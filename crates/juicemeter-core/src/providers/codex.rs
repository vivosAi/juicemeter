//! Codex / ChatGPT plan limits. Live data comes from the ChatGPT backend endpoint the Codex
//! CLI uses (undocumented); if that fails we fall back to the newest `rate_limits` event in
//! Codex's session logs, which is only as fresh as the last Codex turn.

use super::{check, Error, Provider};
use crate::model::{Account, Balance, Perk, Snapshot, Source, Window};
use crate::secrets::tilde;
use anyhow::{anyhow, Context};
use base64::Engine;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const AUTH_HINT: &str = "run `codex` so it refreshes its login";
/// How many recent session files to scan for a rate_limits event.
const MAX_LOG_FILES: usize = 10;
/// Older log data says nothing useful about current limits (the longest window is a week).
const MAX_LOG_AGE_DAYS: i64 = 7;

/// How long free-reset expiry dates are reused before asking Codex again.
const PERKS_TTL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// A Codex login. `home` is a `CODEX_HOME`.
pub struct Codex {
    home: PathBuf,
    label: Option<String>,
    /// Free resets with expiry dates, from the last app-server answer.
    perks: std::sync::Mutex<Option<(std::time::Instant, Vec<Perk>)>>,
}

impl Codex {
    pub fn new(home: PathBuf, label: Option<String>) -> Self {
        Self { home, label, perks: std::sync::Mutex::new(None) }
    }

    /// Free resets for a live snapshot. The usage endpoint gives only a count; expiry dates
    /// come from codex app-server, asked at most every few hours.
    async fn perks(&self, available: u32) -> Vec<Perk> {
        if available == 0 {
            return vec![];
        }
        let cached = self.perks.lock().unwrap().clone();
        if let Some((at, perks)) = cached {
            if at.elapsed() < PERKS_TTL && perks.first().is_some_and(|p| p.count == available) {
                return perks;
            }
        }
        match app_server::read(&self.home).await {
            Ok(s) if !s.perks.is_empty() => {
                *self.perks.lock().unwrap() = Some((std::time::Instant::now(), s.perks.clone()));
                s.perks
            }
            _ => vec![Perk { label: "Free resets".into(), count: available, expires_at: None }],
        }
    }

    fn tokens(&self) -> Option<Tokens> {
        let text = std::fs::read_to_string(self.home.join("auth.json")).ok()?;
        serde_json::from_str::<AuthFile>(&text).ok()?.tokens
    }
}

pub fn default_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".codex")))
}

#[derive(Deserialize)]
struct AuthFile {
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    account_id: Option<String>,
    id_token: Option<String>,
}

#[derive(Deserialize)]
struct IdClaims {
    email: Option<String>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: AuthClaims,
}

#[derive(Deserialize, Default)]
struct AuthClaims {
    chatgpt_user_id: Option<String>,
    chatgpt_account_id: Option<String>,
}

/// Read the claims of the login's ID token. The signature is not checked: the token is only
/// used to label the account, never to authorize anything.
fn id_claims(jwt: &str) -> Option<IdClaims> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

// --- live endpoint ---

#[derive(Deserialize)]
struct LiveUsage {
    plan_type: Option<String>,
    rate_limit: Option<LiveRateLimit>,
    credits: Option<Credits>,
    rate_limit_reset_credits: Option<ResetCounts>,
}

#[derive(Deserialize)]
struct ResetCounts {
    available_count: Option<u32>,
}

#[derive(Deserialize)]
struct LiveRateLimit {
    primary_window: Option<LiveWindow>,
    secondary_window: Option<LiveWindow>,
}

#[derive(Deserialize)]
struct LiveWindow {
    used_percent: f64,
    limit_window_seconds: Option<u64>,
    reset_at: Option<i64>,
}

#[derive(Deserialize)]
struct Credits {
    #[serde(default)]
    has_credits: bool,
    balance: Option<String>,
}

// --- session log event ---

#[derive(Deserialize)]
struct LogLine {
    timestamp: DateTime<Utc>,
    payload: LogPayload,
}

#[derive(Deserialize)]
struct LogPayload {
    rate_limits: Option<LogRateLimits>,
}

#[derive(Deserialize)]
struct LogRateLimits {
    plan_type: Option<String>,
    primary: Option<LogWindow>,
    secondary: Option<LogWindow>,
    credits: Option<Credits>,
}

#[derive(Deserialize)]
struct LogWindow {
    used_percent: f64,
    window_minutes: Option<u64>,
    resets_at: Option<i64>,
}

fn window(slot: &str, used: f64, secs: Option<u64>, reset: Option<i64>) -> Window {
    let label = match secs {
        Some(18000) => "5-hour".to_string(),
        Some(604800) => "Weekly".to_string(),
        Some(s) if s % 86400 == 0 => format!("{}-day", s / 86400),
        Some(s) => format!("{}-hour", s / 3600),
        None => slot.to_string(),
    };
    Window {
        id: slot.into(),
        label,
        used_percent: used,
        window_seconds: secs,
        resets_at: reset.and_then(|t| Utc.timestamp_opt(t, 0).single()),
    }
}

fn credits(c: Option<Credits>) -> Vec<Balance> {
    c.filter(|c| c.has_credits)
        .and_then(|c| c.balance?.parse::<f64>().ok())
        .map(|amount| Balance { label: "Credits".into(), amount, currency: "credits".into() })
        .into_iter()
        .collect()
}

async fn fetch_live(http: &reqwest::Client, tokens: &Tokens) -> anyhow::Result<(Snapshot, u32)> {
    let mut req = http.get(USAGE_URL).bearer_auth(&tokens.access_token);
    if let Some(acc) = &tokens.account_id {
        req = req.header("ChatGPT-Account-Id", acc);
    }
    let resp = req.send().await.context("request failed")?;
    let u: LiveUsage = check(resp, AUTH_HINT).await?.json().await.context("unexpected response")?;
    let mut windows = Vec::new();
    if let Some(rl) = u.rate_limit {
        for (slot, w) in [("primary", rl.primary_window), ("secondary", rl.secondary_window)] {
            if let Some(w) = w {
                windows.push(window(slot, w.used_percent, w.limit_window_seconds, w.reset_at));
            }
        }
    }
    let resets = u.rate_limit_reset_credits.and_then(|r| r.available_count).unwrap_or(0);
    let snapshot = Snapshot {
        perks: vec![],
        plan: u.plan_type,
        windows,
        balances: credits(u.credits),
        source: Source::Live,
        as_of: Utc::now(),
        note: None,
    };
    Ok((snapshot, resets))
}

fn newest_session_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<(std::time::SystemTime, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "jsonl") {
                if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                    out.push((m, p));
                }
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.into_iter().take(MAX_LOG_FILES).map(|(_, p)| p).collect()
}

fn latest_from_logs(home: &Path) -> anyhow::Result<Snapshot> {
    for file in newest_session_files(&home.join("sessions")) {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let found = text
            .lines()
            .rev()
            .filter(|l| l.contains("\"rate_limits\""))
            .filter_map(|l| serde_json::from_str::<LogLine>(l).ok())
            .find_map(|l| Some((l.timestamp, l.payload.rate_limits?)));
        if let Some((ts, rl)) = found {
            let windows = [("primary", rl.primary), ("secondary", rl.secondary)]
                .into_iter()
                .filter_map(|(slot, w)| {
                    let w = w?;
                    Some(window(slot, w.used_percent, w.window_minutes.map(|m| m * 60), w.resets_at))
                })
                .collect();
            return Ok(Snapshot {
                perks: vec![],
                plan: rl.plan_type,
                windows,
                balances: credits(rl.credits),
                source: Source::LocalLog,
                as_of: ts,
                note: None,
            });
        }
    }
    Err(anyhow!("no rate-limit events in Codex session logs"))
}

#[async_trait::async_trait]
impl Provider for Codex {
    fn kind(&self) -> &'static str {
        "codex"
    }
    fn name(&self) -> &str {
        "Codex"
    }
    fn source(&self) -> String {
        tilde(&self.home)
    }

    fn account(&self) -> Option<Account> {
        let t = self.tokens()?;
        let claims = t.id_token.as_deref().and_then(id_claims);
        let (email, user, workspace) = match claims {
            Some(c) => (c.email, c.auth.chatgpt_user_id, c.auth.chatgpt_account_id.or(t.account_id)),
            None => (None, None, t.account_id),
        };
        // Limits are per user within a workspace, so both parts identify the account.
        let native = format!("{}/{}", user.as_deref().unwrap_or(""), workspace.as_deref().unwrap_or(""));
        if native == "/" {
            return None;
        }
        Some(Account { email, label: self.label.clone(), ..Account::new("codex", &native) })
    }

    async fn fetch(&self, http: &reqwest::Client) -> Result<Snapshot, Error> {
        let home = &self.home;
        // 1. The usage endpoint, with the stored login.
        let live_err = match self.tokens() {
            Some(t) => match fetch_live(http, &t).await {
                Ok((mut s, resets)) => {
                    s.perks = self.perks(resets).await;
                    return Ok(s);
                }
                Err(e) => Some(e),
            },
            None => None,
        };
        // 2. Codex itself, asked over its app-server protocol. Covers logins kept outside
        //    auth.json (e.g. in the system keyring) too.
        let rpc_err = match app_server::read(home).await {
            Ok(s) => return Ok(s),
            Err(e) => e,
        };
        let Some(live_err) = live_err else {
            // Without any login, old logs may belong to a previous account: don't guess.
            return Err(Error::NotConfigured("log in with `codex login` (ChatGPT account)".into()));
        };
        // 3. The newest session log, if recent.
        match latest_from_logs(home) {
            Ok(mut s) if Utc::now() - s.as_of < chrono::Duration::days(MAX_LOG_AGE_DAYS) => {
                s.note = Some(format!("live fetch failed ({live_err:#}); showing last session log"));
                Ok(s)
            }
            _ => Err(anyhow!("{live_err:#}; codex app-server: {rpc_err:#}").into()),
        }
    }
}

/// Codex's own JSON-RPC server (`codex app-server`), which answers `account/rateLimits/read`
/// from the login Codex manages. Read-only sandbox, no approvals, no prompts.
mod app_server {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    const TIMEOUT: Duration = Duration::from_secs(20);

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Limits {
        plan_type: Option<String>,
        primary: Option<RpcWindow>,
        secondary: Option<RpcWindow>,
        credits: Option<RpcCredits>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RpcWindow {
        used_percent: f64,
        window_duration_mins: Option<u64>,
        resets_at: Option<i64>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RpcCredits {
        #[serde(default)]
        has_credits: bool,
        balance: Option<String>,
    }

    /// Where `codex` may be installed. Agents run without the user's shell PATH, so check the
    /// usual places, newest Node versions first; old copies that lack app-server are skipped.
    fn candidates() -> Vec<PathBuf> {
        let mut v = Vec::new();
        if let Some(p) = std::env::var_os("JUICEMETER_CODEX_BIN") {
            v.push(PathBuf::from(p));
        }
        if let Some(h) = dirs::home_dir() {
            let mut nvm: Vec<PathBuf> = std::fs::read_dir(h.join(".nvm/versions/node"))
                .map(|d| d.flatten().map(|e| e.path().join("bin/codex")).collect())
                .unwrap_or_default();
            nvm.sort();
            v.extend(nvm.into_iter().rev());
            for rel in [".npm-global/bin/codex", ".local/bin/codex", ".bun/bin/codex", ".volta/bin/codex"] {
                v.push(h.join(rel));
            }
        }
        v.extend(["/opt/homebrew/bin/codex", "/usr/local/bin/codex"].map(PathBuf::from));
        if let Some(path) = std::env::var_os("PATH") {
            v.extend(std::env::split_paths(&path).map(|d| d.join("codex")));
        }
        v.retain(|p| p.is_file());
        v.dedup();
        v
    }

    pub async fn read(home: &Path) -> anyhow::Result<Snapshot> {
        let found = candidates();
        if found.is_empty() {
            anyhow::bail!("codex not found");
        }
        let mut last = anyhow!("no codex answered");
        for bin in found {
            match tokio::time::timeout(TIMEOUT, ask(&bin, home)).await {
                Ok(Ok(s)) => return Ok(s),
                Ok(Err(e)) => last = e.context(bin.display().to_string()),
                Err(_) => last = anyhow!("{} took longer than {}s", bin.display(), TIMEOUT.as_secs()),
            }
        }
        Err(last)
    }

    async fn ask(bin: &Path, home: &Path) -> anyhow::Result<Snapshot> {
        let mut child = tokio::process::Command::new(bin)
            .args(["-s", "read-only", "-a", "never", "app-server"])
            .env("CODEX_HOME", home)
            // Node-based installs find `node` next to the script.
            .env("PATH", path_with(bin))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("could not start")?;
        let mut stdin = child.stdin.take().context("no stdin")?;
        let mut lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();

        let send = |v: serde_json::Value| format!("{v}\n");
        stdin
            .write_all(
                send(serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "juicemeter", "version": env!("CARGO_PKG_VERSION")}}}))
                .as_bytes(),
            )
            .await?;
        reply(&mut lines, 1).await?;
        stdin
            .write_all(send(serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}})).as_bytes())
            .await?;
        stdin
            .write_all(
                send(serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "account/rateLimits/read"})).as_bytes(),
            )
            .await?;
        let result = reply(&mut lines, 2).await?;
        let _ = child.kill().await;

        let limits: Limits =
            serde_json::from_value(result.get("rateLimits").cloned().context("no rateLimits in reply")?)
                .context("unexpected rateLimits")?;
        let mut snapshot = parse(limits);
        snapshot.perks = reset_credits(&result);
        Ok(snapshot)
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ResetCredit {
        status: Option<String>,
        expires_at: Option<i64>,
    }

    /// Free full resets OpenAI hands out; each expires if unused.
    pub(super) fn reset_credits(result: &serde_json::Value) -> Vec<Perk> {
        let credits: Vec<ResetCredit> = result
            .pointer("/rateLimitResetCredits/credits")
            .and_then(|c| serde_json::from_value(c.clone()).ok())
            .unwrap_or_default();
        let now = Utc::now().timestamp();
        let available: Vec<i64> = credits
            .iter()
            .filter(|c| c.status.as_deref() == Some("available"))
            .map(|c| c.expires_at.unwrap_or(i64::MAX))
            .filter(|&e| e > now)
            .collect();
        if available.is_empty() {
            return vec![];
        }
        let next = available.iter().min().copied().filter(|&e| e != i64::MAX);
        vec![Perk {
            label: "Free resets".into(),
            count: available.len() as u32,
            expires_at: next.and_then(|e| Utc.timestamp_opt(e, 0).single()),
        }]
    }

    fn path_with(bin: &Path) -> std::ffi::OsString {
        let mut dirs: Vec<PathBuf> = bin.parent().map(Path::to_path_buf).into_iter().collect();
        if let Some(p) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&p));
        }
        std::env::join_paths(dirs).unwrap_or_default()
    }

    /// The `result` of the reply with this id; notifications in between are skipped.
    async fn reply<R: tokio::io::AsyncBufRead + Unpin>(
        lines: &mut tokio::io::Lines<R>,
        id: u64,
    ) -> anyhow::Result<serde_json::Value> {
        while let Some(line) = lines.next_line().await? {
            let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            if msg.get("id").and_then(|v| v.as_u64()) != Some(id) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                anyhow::bail!("codex replied with an error: {err}");
            }
            return msg.get("result").cloned().context("reply has no result");
        }
        anyhow::bail!("codex exited without replying")
    }

    fn parse(l: Limits) -> Snapshot {
        let windows = [("primary", l.primary), ("secondary", l.secondary)]
            .into_iter()
            .filter_map(|(slot, w)| {
                let w = w?;
                Some(window(slot, w.used_percent, w.window_duration_mins.map(|m| m * 60), w.resets_at))
            })
            .collect();
        let credits = l.credits.map(|c| Credits { has_credits: c.has_credits, balance: c.balance });
        Snapshot {
            perks: vec![],
            plan: l.plan_type,
            windows,
            balances: super::credits(credits),
            source: Source::Live,
            as_of: Utc::now(),
            note: Some("via codex app-server".into()),
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn parses_rate_limits_reply() {
            let l: super::Limits = serde_json::from_str(
                r#"{"limitId":"codex","primary":{"usedPercent":12,"windowDurationMins":300,"resetsAt":1790995970},
                    "secondary":{"usedPercent":40,"windowDurationMins":10080,"resetsAt":1791582770},
                    "credits":{"hasCredits":false,"unlimited":false,"balance":"0"},"planType":"plus"}"#,
            )
            .unwrap();
            let s = super::parse(l);
            assert_eq!(s.plan.as_deref(), Some("plus"));
            assert_eq!((s.windows[0].label.as_str(), s.windows[0].used_percent), ("5-hour", 12.0));
            assert_eq!((s.windows[1].label.as_str(), s.windows[1].window_seconds), ("Weekly", Some(604800)));
            assert!(s.balances.is_empty());
        }

        #[test]
        fn counts_available_resets_and_finds_the_next_expiry() {
            let soon = chrono::Utc::now().timestamp() + 86400;
            let later = soon + 5 * 86400;
            let past = soon - 10 * 86400;
            let v = serde_json::json!({"rateLimitResetCredits": {"credits": [
                {"status": "available", "expiresAt": later},
                {"status": "available", "expiresAt": soon},
                {"status": "used", "expiresAt": soon},
                {"status": "available", "expiresAt": past}]}});
            let p = super::reset_credits(&v);
            assert_eq!((p[0].count, p[0].expires_at.unwrap().timestamp()), (2, soon));
            assert!(super::reset_credits(&serde_json::json!({})).is_empty());
        }

        /// Talks to the real Codex on this machine: `cargo test -- --ignored live_app_server`.
        #[tokio::test]
        #[ignore]
        async fn live_app_server() {
            let s = super::read(&crate::providers::codex::default_home().unwrap()).await.unwrap();
            assert!(!s.windows.is_empty());
            println!("{:?} {:?}", s.plan, s.windows.iter().map(|w| (&w.label, w.used_percent)).collect::<Vec<_>>());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_windows_by_length() {
        assert_eq!(window("primary", 5.0, Some(18000), None).label, "5-hour");
        assert_eq!(window("secondary", 5.0, Some(604800), None).label, "Weekly");
        assert_eq!(window("x", 5.0, None, None).label, "x");
    }

    #[test]
    fn reads_rate_limits_from_logs() {
        let home = std::env::temp_dir().join(format!("juicemeter-codex-{}", std::process::id()));
        let day = home.join("sessions/2026/09/06");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout.jsonl"),
            concat!(
                r#"{"timestamp":"2026-09-06T15:00:00Z","payload":{"rate_limits":{"plan_type":"plus","primary":{"used_percent":40.0,"window_minutes":300,"resets_at":1788718681}}}}"#, "\n",
                r#"{"timestamp":"2026-09-06T15:05:00Z","payload":{"type":"other"}}"#, "\n",
            ),
        )
        .unwrap();
        let s = latest_from_logs(&home).unwrap();
        assert_eq!(s.source, Source::LocalLog);
        assert_eq!(s.windows[0].used_percent, 40.0);
        assert_eq!(s.windows[0].window_seconds, Some(18000));
        std::fs::remove_dir_all(home).ok();
    }
}
