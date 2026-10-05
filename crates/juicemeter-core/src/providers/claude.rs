//! Claude (Pro/Max) plan limits via the OAuth usage endpoint that Claude Code's `/usage` uses.
//! Undocumented: the shape may change. Reads Claude Code's stored login; never refreshes it.

use super::{check, Error, Provider};
use crate::model::{Account, Balance, Snapshot, Source, Window};
use crate::secrets::{keychain, tilde};
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const AUTH_HINT: &str = "open Claude Code so it refreshes its login";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// A Claude Code login. `dir` is a `CLAUDE_CONFIG_DIR`; `None` means the default `~/.claude`.
pub struct Claude {
    dir: Option<PathBuf>,
    keychain_service: Option<String>,
    label: Option<String>,
}

impl Claude {
    pub fn new(dir: Option<PathBuf>, keychain_service: Option<String>, label: Option<String>) -> Self {
        Self { dir, keychain_service, label }
    }

    fn dir(&self) -> Option<PathBuf> {
        self.dir.clone().or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
    }

    fn credentials(&self) -> Option<OAuth> {
        let keychain_raw = || match (&self.keychain_service, &self.dir) {
            (Some(svc), _) => keychain(svc, None),
            (None, None) => keychain(KEYCHAIN_SERVICE, None),
            // Unverified guess at Claude Code's naming for non-default config dirs; set
            // `keychain_service` in the source config if it doesn't match.
            (None, Some(d)) => {
                let h: String =
                    Sha256::digest(d.to_string_lossy().as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect();
                keychain(&format!("{KEYCHAIN_SERVICE}-{h}"), None)
            }
        };
        let file_raw = || std::fs::read_to_string(self.dir()?.join(".credentials.json")).ok();
        // Explicit keychain first, then the dir's file, then the derived keychain item.
        let raw = if self.dir.is_some() && self.keychain_service.is_none() {
            file_raw().or_else(keychain_raw)
        } else {
            keychain_raw().or_else(file_raw)
        }?;
        serde_json::from_str::<CredFile>(&raw).ok().map(|c| c.oauth)
    }

    /// Claude Code keeps the account profile in `~/.claude.json`, or inside a custom config dir.
    fn profile_path(&self) -> Option<PathBuf> {
        match &self.dir {
            Some(d) => Some(d.join(".claude.json")),
            None => dirs::home_dir().map(|h| h.join(".claude.json")),
        }
    }
}

#[derive(Deserialize)]
struct Profile {
    #[serde(rename = "oauthAccount")]
    oauth_account: Option<OAuthAccount>,
}

#[derive(Deserialize)]
struct OAuthAccount {
    #[serde(rename = "accountUuid")]
    account_uuid: String,
    #[serde(rename = "emailAddress")]
    email: Option<String>,
    #[serde(rename = "organizationName")]
    org: Option<String>,
}

#[derive(Deserialize)]
struct CredFile {
    #[serde(rename = "claudeAiOauth")]
    oauth: OAuth,
}

#[derive(Deserialize)]
struct OAuth {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    limits: Vec<Limit>,
    five_hour: Option<LegacyWindow>,
    seven_day: Option<LegacyWindow>,
    spend: Option<Spend>,
}

/// Extra usage: money spent past the plan's limits, against an optional monthly cap.
#[derive(Deserialize)]
struct Spend {
    #[serde(default)]
    enabled: bool,
    used: Option<Money>,
    limit: Option<Money>,
    percent: Option<f64>,
}

#[derive(Deserialize)]
struct Money {
    amount_minor: i64,
    exponent: i32,
    currency: String,
}

impl Money {
    fn amount(&self) -> f64 {
        self.amount_minor as f64 / 10f64.powi(self.exponent)
    }
}

/// Extra usage, only when it's switched on: what's been spent, and the cap as a window.
fn spend(s: Option<Spend>) -> (Vec<Window>, Vec<Balance>) {
    let Some(s) = s.filter(|s| s.enabled) else { return (vec![], vec![]) };
    let balances = s
        .used
        .iter()
        .map(|m| Balance { label: "Extra usage spent".into(), amount: m.amount(), currency: m.currency.clone() })
        .collect();
    let windows = match (&s.limit, s.percent) {
        (Some(_), Some(p)) => vec![Window {
            id: "extra_usage".into(),
            label: "Extra usage".into(),
            used_percent: p,
            window_seconds: None,
            resets_at: None,
        }],
        _ => vec![],
    };
    (windows, balances)
}

#[derive(Deserialize)]
struct Limit {
    kind: String,
    percent: f64,
    resets_at: Option<DateTime<Utc>>,
    scope: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct LegacyWindow {
    utilization: f64,
    resets_at: Option<DateTime<Utc>>,
}

fn window(id: &str, label: String, percent: f64, resets_at: Option<DateTime<Utc>>) -> Window {
    let window_seconds = match id {
        "session" => Some(5 * 3600),
        _ if id.starts_with("weekly") => Some(7 * 86400),
        _ => None,
    };
    Window { id: id.into(), label, used_percent: percent, window_seconds, resets_at }
}

fn parse(usage: Usage) -> Vec<Window> {
    if !usage.limits.is_empty() {
        return usage
            .limits
            .into_iter()
            .map(|l| {
                let model = l
                    .scope
                    .as_ref()
                    .and_then(|s| s.pointer("/model/display_name"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                let label = match (l.kind.as_str(), model) {
                    ("session", _) => "Session".to_string(),
                    ("weekly_all", _) => "Weekly".to_string(),
                    (_, Some(m)) => format!("Weekly · {m}"),
                    (k, None) => k.replace('_', " "),
                };
                window(&l.kind, label, l.percent, l.resets_at)
            })
            .collect();
    }
    // Older response shape.
    let mut v = Vec::new();
    if let Some(w) = usage.five_hour {
        v.push(window("session", "Session".into(), w.utilization, w.resets_at));
    }
    if let Some(w) = usage.seven_day {
        v.push(window("weekly_all", "Weekly".into(), w.utilization, w.resets_at));
    }
    v
}

#[async_trait::async_trait]
impl Provider for Claude {
    fn kind(&self) -> &'static str {
        "claude"
    }
    fn name(&self) -> &str {
        "Claude"
    }
    /// Claude's usage endpoint rate-limits eagerly, and its windows are hours to days long.
    fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(10 * 60)
    }

    fn source(&self) -> String {
        self.dir().map(|d| tilde(&d)).unwrap_or_else(|| "~/.claude".into())
    }

    fn account(&self) -> Option<Account> {
        let text = std::fs::read_to_string(self.profile_path()?).ok()?;
        let a = serde_json::from_str::<Profile>(&text).ok()?.oauth_account?;
        Some(Account {
            email: a.email,
            org: a.org,
            label: self.label.clone(),
            ..Account::new("claude", &a.account_uuid)
        })
    }

    async fn fetch(&self, http: &reqwest::Client) -> Result<Snapshot, Error> {
        let creds = self
            .credentials()
            .ok_or_else(|| Error::NotConfigured("log in with Claude Code (`claude`) on this machine".into()))?;
        if creds.expires_at.is_some_and(|ms| ms <= Utc::now().timestamp_millis()) {
            return Err(anyhow::anyhow!("stored login expired — {AUTH_HINT}").into());
        }
        let resp = http
            .get(USAGE_URL)
            .bearer_auth(&creds.access_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await
            .context("request failed")?;
        let mut usage: Usage = check(resp, AUTH_HINT).await?.json().await.context("unexpected response")?;
        let (extra_windows, balances) = spend(usage.spend.take());
        let mut windows = parse(usage);
        windows.extend(extra_windows);
        Ok(Snapshot {
            perks: vec![],
            plan: creds.subscription_type,
            windows,
            balances,
            source: Source::Live,
            as_of: Utc::now(),
            note: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_limits_array() {
        let u: Usage = serde_json::from_str(
            r#"{"limits":[
              {"kind":"session","percent":4,"resets_at":"2026-10-01T17:59:59Z"},
              {"kind":"weekly_scoped","percent":0,"resets_at":null,"scope":{"model":{"display_name":"Fable"}}}]}"#,
        )
        .unwrap();
        let w = parse(u);
        assert_eq!(w[0].label, "Session");
        assert_eq!(w[0].window_seconds, Some(18000));
        assert_eq!(w[1].label, "Weekly · Fable");
    }

    #[test]
    fn extra_usage_only_when_enabled() {
        let off: Usage = serde_json::from_str(
            r#"{"spend":{"used":{"amount_minor":0,"currency":"USD","exponent":2},"limit":null,"percent":0,"enabled":false}}"#,
        )
        .unwrap();
        assert_eq!(spend(off.spend).1.len(), 0);
        let on: Usage = serde_json::from_str(
            r#"{"spend":{"used":{"amount_minor":1250,"currency":"USD","exponent":2},"limit":{"amount_minor":5000,"currency":"USD","exponent":2},"percent":25,"enabled":true}}"#,
        )
        .unwrap();
        let (w, b) = spend(on.spend);
        assert_eq!((b[0].amount, b[0].display()), (12.5, "$12.50".to_string()));
        assert_eq!((w[0].label.as_str(), w[0].used_percent), ("Extra usage", 25.0));
    }

    #[test]
    fn falls_back_to_legacy_shape() {
        let u: Usage = serde_json::from_str(r#"{"five_hour":{"utilization":12.5,"resets_at":null}}"#).unwrap();
        assert_eq!(parse(u)[0].used_percent, 12.5);
    }
}
