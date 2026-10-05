use crate::log;
use chrono::Local;
use juicemeter_core::{Config, Outcome, Provider, ProviderReport, Report};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, RwLock};

/// Manual refreshes closer together than this are refused, to stay polite to the providers.
const MIN_REFRESH_GAP: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
pub struct Shared(Arc<Inner>);

struct Inner {
    providers: Vec<Box<dyn Provider>>,
    reports: RwLock<Vec<Option<ProviderReport>>>,
    http: juicemeter_core::reqwest::Client,
    cache: PathBuf,
    config: Config,
    wake: Vec<Notify>,
    /// Per provider: don't call it again before this (it asked us to slow down).
    cooldown: Vec<Mutex<Option<Instant>>>,
    last_manual: Mutex<Option<Instant>>,
}

impl Shared {
    pub fn new(providers: Vec<Box<dyn Provider>>, cache: PathBuf, config: Config) -> Self {
        let mut reports: Vec<Option<ProviderReport>> = vec![None; providers.len()];
        if let Some(saved) = load(&cache) {
            for r in saved.providers {
                if let Some(i) = providers.iter().position(|p| p.kind() == r.provider && p.source() == r.source) {
                    reports[i] = Some(r);
                }
            }
            log!("loaded cached report from {}", cache.display());
        }
        let wake = providers.iter().map(|_| Notify::new()).collect();
        let cooldown = providers.iter().map(|_| Mutex::new(None)).collect();
        Self(Arc::new(Inner {
            providers,
            reports: RwLock::new(reports),
            http: juicemeter_core::http_client(),
            cache,
            config,
            wake,
            cooldown,
            last_manual: Mutex::new(None),
        }))
    }

    pub async fn report(&self) -> Report {
        // Names from `[labels]` belong to whoever is looking and are applied there, fresh;
        // applying them here too made a deleted name stick until the agent restarted.
        let mut r = Report::new(self.0.reports.read().await.iter().flatten().cloned().collect());
        if !self.0.config.share_email {
            r.redact_emails();
        }
        r
    }

    pub fn spawn_pollers(&self) {
        for i in 0..self.0.providers.len() {
            let s = self.clone();
            tokio::spawn(async move { s.poll(i).await });
        }
    }

    async fn poll(&self, i: usize) {
        let base = self.0.config.interval(self.0.providers[i].as_ref());
        let mut delay = base;
        loop {
            delay = match self.fetch(i).await {
                Fetched::Ok => base,
                Fetched::Failed => (delay * 2).min(MAX_BACKOFF).max(base),
                Fetched::Wait(d) => d.max(base),
            };
            // A wake means a manual refresh just fetched: restart the timer, don't fetch again.
            while tokio::time::timeout(delay, self.0.wake[i].notified()).await.is_ok() {}
        }
    }

    /// Fetch provider `i` and store the result, unless it asked us to wait.
    async fn fetch(&self, i: usize) -> Fetched {
        let p = &self.0.providers[i];
        if let Some(until) = *self.0.cooldown[i].lock().await {
            if let Some(left) = until.checked_duration_since(Instant::now()) {
                return Fetched::Wait(left);
            }
        }
        let (new, wait) = juicemeter_core::fetch_one_paced(p.as_ref(), &self.0.http).await;
        *self.0.cooldown[i].lock().await = wait.map(|d| Instant::now() + d);
        let failed = matches!(new.outcome, Outcome::Error { .. });
        if let Outcome::Error { message } = &new.outcome {
            log!("{} {}: {message}", p.kind(), p.source());
        }
        {
            let mut reports = self.0.reports.write().await;
            reports[i] = Some(merge(reports[i].take(), new));
        }
        self.persist().await;
        match (wait, failed) {
            (Some(d), _) => Fetched::Wait(d),
            (None, true) => Fetched::Failed,
            (None, false) => Fetched::Ok,
        }
    }

    /// Refresh everything now. Err carries how long until a refresh is allowed again.
    pub async fn refresh_all(&self) -> Result<(), Duration> {
        {
            let mut last = self.0.last_manual.lock().await;
            if let Some(t) = *last {
                if t.elapsed() < MIN_REFRESH_GAP {
                    return Err(MIN_REFRESH_GAP - t.elapsed());
                }
            }
            *last = Some(Instant::now());
        }
        futures::future::join_all((0..self.0.providers.len()).map(|i| self.fetch(i))).await;
        self.0.wake.iter().for_each(Notify::notify_waiters);
        Ok(())
    }

    async fn persist(&self) {
        let report = self.report().await;
        let path = &self.0.cache;
        let result = async {
            if let Some(dir) = path.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            let tmp = path.with_extension("json.tmp");
            tokio::fs::write(&tmp, serde_json::to_vec_pretty(&report)?).await?;
            tokio::fs::rename(&tmp, path).await?;
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = result {
            log!("could not write cache {}: {e:#}", path.display());
        }
    }
}

enum Fetched {
    Ok,
    Failed,
    /// The provider asked for this much quiet.
    Wait(Duration),
}

/// A failed refresh shouldn't wipe good data: keep the last snapshot (its `as_of` shows its
/// age) and attach the error as a note.
fn merge(prev: Option<ProviderReport>, new: ProviderReport) -> ProviderReport {
    match (prev, &new.outcome) {
        (Some(mut prev), Outcome::Error { message }) => match &mut prev.outcome {
            Outcome::Ok { snapshot } => {
                snapshot.note = Some(format!("refresh failed at {}: {message}", Local::now().format("%H:%M")));
                prev
            }
            _ => new,
        },
        _ => new,
    }
}

fn load(path: &PathBuf) -> Option<Report> {
    let r: Report = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (r.schema_version == juicemeter_core::model::SCHEMA_VERSION).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use juicemeter_core::{Snapshot, Source};

    fn report(outcome: Outcome) -> ProviderReport {
        ProviderReport { provider: "x".into(), name: "X".into(), source: "s".into(), account: None, outcome }
    }

    #[test]
    fn error_keeps_last_good_snapshot() {
        let snap = Snapshot {
            perks: vec![],
            plan: None,
            windows: vec![],
            balances: vec![],
            source: Source::Live,
            as_of: chrono::Utc::now(),
            note: None,
        };
        let merged =
            merge(Some(report(Outcome::Ok { snapshot: snap })), report(Outcome::Error { message: "HTTP 429".into() }));
        let Outcome::Ok { snapshot } = merged.outcome else { panic!("expected ok") };
        assert!(snapshot.note.unwrap().contains("HTTP 429"));
    }

    #[test]
    fn not_configured_replaces_previous() {
        let merged = merge(
            Some(report(Outcome::Error { message: "x".into() })),
            report(Outcome::NotConfigured { hint: "h".into() }),
        );
        assert!(matches!(merged.outcome, Outcome::NotConfigured { .. }));
    }
}
