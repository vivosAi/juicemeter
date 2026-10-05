//! Combine reports from several machines into one entry per account.

use crate::model::{Outcome, ProviderReport, Report};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Merged {
    /// The best copy: a successful fetch over an error, then the most recent data.
    #[serde(flatten)]
    pub report: ProviderReport,
    /// Host the chosen copy came from.
    pub host: String,
    /// Every machine that holds this account, as (host, source).
    pub seen_on: Vec<(String, String)>,
}

/// One entry per account across all reports. Sources without an identifiable account stay
/// separate. A provider that is only "not configured" everywhere appears once.
pub fn merge(reports: &[Report]) -> Vec<Merged> {
    let mut out: Vec<Merged> = Vec::new();
    for r in reports {
        for p in &r.providers {
            let seen = (r.host.clone(), p.source.clone());
            let existing = match (&p.account, &p.outcome) {
                (Some(a), _) => out.iter().position(|m| m.report.account.as_ref().is_some_and(|b| b.id == a.id)),
                (None, Outcome::NotConfigured { .. }) => out.iter().position(|m| {
                    m.report.provider == p.provider && matches!(m.report.outcome, Outcome::NotConfigured { .. })
                }),
                (None, _) => None,
            };
            match existing {
                Some(i) => {
                    let m = &mut out[i];
                    m.seen_on.push(seen);
                    if better(p, &m.report) {
                        let label = m.report.account.as_ref().and_then(|a| a.label.clone());
                        m.report = p.clone();
                        m.host = r.host.clone();
                        // Keep a label if only another copy had one.
                        if let (Some(a), Some(l)) = (m.report.account.as_mut(), label) {
                            a.label.get_or_insert(l);
                        }
                    }
                }
                None => out.push(Merged { report: p.clone(), host: r.host.clone(), seen_on: vec![seen] }),
            }
        }
    }
    // Drop "not configured" placeholders for providers that have a real entry.
    let not_configured = |m: &Merged| matches!(m.report.outcome, Outcome::NotConfigured { .. });
    let real: Vec<String> = out.iter().filter(|m| !not_configured(m)).map(|m| m.report.provider.clone()).collect();
    out.retain(|m| !not_configured(m) || !real.contains(&m.report.provider));
    // Group by provider, keeping first-seen order within each.
    out.sort_by(|a, b| a.report.provider.cmp(&b.report.provider));
    out
}

fn better(new: &ProviderReport, old: &ProviderReport) -> bool {
    match (&new.outcome, &old.outcome) {
        (Outcome::Ok { snapshot: n }, Outcome::Ok { snapshot: o }) => n.as_of > o.as_of,
        (Outcome::Ok { .. }, _) => true,
        (_, Outcome::Ok { .. }) => false,
        (Outcome::Error { .. }, Outcome::NotConfigured { .. }) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Account, Snapshot, Source};
    use chrono::{Duration, Utc};

    fn ok(provider: &str, account: &str, age_min: i64) -> ProviderReport {
        ProviderReport {
            provider: provider.into(),
            name: provider.into(),
            source: "~/.x".into(),
            account: Some(Account { id: account.into(), ..Default::default() }),
            outcome: Outcome::Ok {
                snapshot: Snapshot {
                    perks: vec![],
                    plan: None,
                    windows: vec![],
                    balances: vec![],
                    source: Source::Live,
                    as_of: Utc::now() - Duration::minutes(age_min),
                    note: None,
                },
            },
        }
    }

    fn report(host: &str, providers: Vec<ProviderReport>) -> Report {
        Report { host: host.into(), ..Report::new(providers) }
    }

    #[test]
    fn same_account_merges_and_freshest_wins() {
        let merged = merge(&[
            report("laptop", vec![ok("codex", "codex:a", 10)]),
            report("mini", vec![ok("codex", "codex:a", 1), ok("codex", "codex:b", 1)]),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].host, "mini");
        assert_eq!(merged[0].seen_on.len(), 2);
    }

    #[test]
    fn success_beats_error_and_not_configured_collapses() {
        let mut err = ok("claude", "claude:a", 0);
        err.outcome = Outcome::Error { message: "429".into() };
        let nc = ProviderReport {
            provider: "deepseek".into(),
            name: "DeepSeek".into(),
            source: "default".into(),
            account: None,
            outcome: Outcome::NotConfigured { hint: "h".into() },
        };
        let merged = merge(&[
            report("laptop", vec![err, ok("deepseek", "deepseek:k", 0)]),
            report("mini", vec![ok("claude", "claude:a", 30), nc]),
        ]);
        assert!(matches!(merged[0].report.outcome, Outcome::Ok { .. }));
        assert_eq!(merged[0].host, "mini");
        assert_eq!(merged.len(), 2, "not-configured deepseek on mini is dropped");
    }
}
