//! Providers described in the config instead of code: an HTTP endpoint or a command whose
//! JSON output is mapped onto windows and balances with JSON pointers.
//!
//! ```toml
//! [[source]]
//! provider = "http"
//! name = "Vercel AI Gateway"
//! url = "https://ai-gateway.vercel.sh/v1/credits"
//! key = { env = "AI_GATEWAY_API_KEY" }
//! balance = [{ label = "Credits", amount = "/balance", currency = "USD" }]
//!
//! [[source]]
//! provider = "command"
//! name = "Oh My Pi"
//! command = "omp usage --json"
//! window = [{ label = "Weekly", used = "/weekly/percent", resets_at = "/weekly/resets_at", length = "7d" }]
//! ```
//!
//! A command with no `window` or `balance` mapping must print juicemeter's own format:
//! `{"plan": "pro", "windows": [{"label", "used_percent", "resets_at"?, "window_seconds"?}],
//! "balances": [{"label", "amount", "currency"}]}`.

use super::{check, Error, Provider};
use crate::model::{Account, Balance, Snapshot, Source, Window};
use crate::secrets::KeyRef;
use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// Longest a command may run.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

/// How a key is sent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    /// `Authorization: Bearer <key>`
    #[default]
    Bearer,
    /// The key as the value of this header, e.g. `X-API-Key`.
    Header(String),
}

/// One window read from the response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowMap {
    pub label: String,
    /// Pointer to the share used. Give this or `left`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<String>,
    /// Pointer to the share left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    /// The value is a fraction (0..1) rather than a percentage.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fraction: bool,
    /// Pointer to a total: `used`/`left` are counts out of it, not percentages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
    /// Pointer to the reset time: RFC 3339, or Unix seconds or milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    /// Pointer to the seconds until reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_in: Option<String>,
    /// Window length, e.g. `"5h"` or `"7d"`; enables pace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<String>,
}

/// One balance read from the response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BalanceMap {
    pub label: String,
    /// Pointer to the amount.
    pub amount: String,
    /// Fixed currency code, or a pointer to one if it starts with `/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

/// The response mapping shared by `http` and `command` sources.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mapping {
    pub plan: Option<String>,
    pub windows: Vec<WindowMap>,
    pub balances: Vec<BalanceMap>,
}

impl Mapping {
    fn is_empty(&self) -> bool {
        self.windows.is_empty() && self.balances.is_empty()
    }

    pub fn apply(&self, v: &Value, now: DateTime<Utc>) -> anyhow::Result<(Option<String>, Vec<Window>, Vec<Balance>)> {
        let plan = self.plan.as_deref().and_then(|p| v.pointer(p)).and_then(as_text);
        let mut windows = Vec::new();
        for m in &self.windows {
            let to_pct = |x: f64| -> anyhow::Result<f64> {
                Ok(match &m.of {
                    Some(total) => {
                        let total = number(v, total)?;
                        if total > 0.0 {
                            x / total * 100.0
                        } else {
                            0.0
                        }
                    }
                    None if m.fraction => x * 100.0,
                    None => x,
                })
            };
            let used = match (&m.used, &m.left) {
                (Some(p), _) => to_pct(number(v, p)?)?,
                (None, Some(p)) => 100.0 - to_pct(number(v, p)?)?,
                (None, None) => bail!("window `{}` needs `used` or `left`", m.label),
            };
            let resets_at = match (&m.resets_at, &m.resets_in) {
                (Some(p), _) => v.pointer(p).and_then(timestamp),
                (None, Some(p)) => v.pointer(p).and_then(as_number).map(|s| now + chrono::Duration::seconds(s as i64)),
                _ => None,
            };
            windows.push(Window {
                id: m.label.to_lowercase().replace(' ', "_"),
                label: m.label.clone(),
                used_percent: used.clamp(0.0, 100.0),
                window_seconds: m.length.as_deref().map(duration).transpose()?.map(|d| d.as_secs()),
                resets_at,
            });
        }
        let mut balances = Vec::new();
        for b in &self.balances {
            let currency = match b.currency.as_deref() {
                Some(c) if c.starts_with('/') => v.pointer(c).and_then(as_text).unwrap_or_default(),
                Some(c) => c.to_string(),
                None => String::new(),
            };
            balances.push(Balance { label: b.label.clone(), amount: number(v, &b.amount)?, currency });
        }
        Ok((plan, windows, balances))
    }
}

fn as_number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.trim().parse().ok())
}

fn as_text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned).or_else(|| v.as_f64().map(|n| n.to_string()))
}

fn number(v: &Value, pointer: &str) -> anyhow::Result<f64> {
    let found = v.pointer(pointer).ok_or_else(|| anyhow!("nothing at `{pointer}` in the response"))?;
    as_number(found).ok_or_else(|| anyhow!("`{pointer}` is not a number"))
}

/// RFC 3339 text, a `YYYY-MM-DD` date (midnight UTC), or Unix seconds / milliseconds.
fn timestamp(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(s) = v.as_str() {
        if let Ok(t) = DateTime::parse_from_rfc3339(s) {
            return Some(t.with_timezone(&Utc));
        }
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc());
        }
    }
    let n = as_number(v)?;
    let secs = if n > 1e12 { n / 1000.0 } else { n };
    Utc.timestamp_opt(secs as i64, 0).single()
}

/// `"90s"`, `"30m"`, `"5h"`, `"7d"`.
pub fn duration(s: &str) -> anyhow::Result<Duration> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let n: u64 = s[..split].parse().with_context(|| format!("bad length `{s}`"))?;
    let unit = match &s[split..] {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => bail!("bad length `{s}`, use s, m, h or d"),
    };
    Ok(Duration::from_secs(n * unit))
}

/// A usage endpoint described in the config.
pub struct Http {
    pub name: String,
    pub url: String,
    pub post_body: Option<String>,
    pub key: Option<KeyRef>,
    pub auth: Auth,
    pub label: Option<String>,
    pub map: Mapping,
}

#[async_trait::async_trait]
impl Provider for Http {
    fn kind(&self) -> &'static str {
        "http"
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn source(&self) -> String {
        self.url.clone()
    }

    fn account(&self) -> Option<Account> {
        let key = self.key.as_ref().and_then(|k| k.resolve().ok());
        let native = format!("{}|{}", self.url, key.as_deref().unwrap_or(""));
        let host = self.url.split("//").nth(1).and_then(|r| r.split('/').next()).unwrap_or(&self.url).to_string();
        Some(Account {
            hint: Some(key.as_deref().map(crate::secrets::mask).unwrap_or(host)),
            label: self.label.clone(),
            ..Account::new("http", &native)
        })
    }

    async fn fetch(&self, http: &reqwest::Client) -> Result<Snapshot, Error> {
        if self.map.is_empty() {
            return Err(anyhow!("http source `{}` maps no window or balance", self.name).into());
        }
        let mut req = match &self.post_body {
            Some(body) => http.post(&self.url).header("Content-Type", "application/json").body(body.clone()),
            None => http.get(&self.url),
        };
        if let Some(k) = &self.key {
            let key = k.resolve()?;
            req = match &self.auth {
                Auth::Bearer => req.bearer_auth(key),
                Auth::Header(h) => req.header(h.as_str(), key),
            };
        }
        let resp = req.send().await.context("request failed")?;
        let body: Value = check(resp, "check the key").await?.json().await.context("response is not JSON")?;
        let now = Utc::now();
        let (plan, windows, balances) = self.map.apply(&body, now)?;
        Ok(Snapshot { plan, windows, balances, source: Source::Live, as_of: now, note: None, perks: vec![] })
    }
}

/// A command whose JSON output is the usage.
pub struct Command {
    pub name: String,
    pub command: String,
    pub label: Option<String>,
    pub map: Mapping,
}

/// juicemeter's own format, for commands written for it.
#[derive(Deserialize)]
struct Native {
    plan: Option<String>,
    #[serde(default)]
    windows: Vec<NativeWindow>,
    #[serde(default)]
    balances: Vec<Balance>,
}

#[derive(Deserialize)]
struct NativeWindow {
    label: String,
    used_percent: f64,
    resets_at: Option<DateTime<Utc>>,
    window_seconds: Option<u64>,
}

#[async_trait::async_trait]
impl Provider for Command {
    fn kind(&self) -> &'static str {
        "command"
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn source(&self) -> String {
        format!("`{}`", self.command)
    }

    fn account(&self) -> Option<Account> {
        let short: String = self.command.chars().take(28).collect();
        let hint = if short.len() < self.command.len() { format!("{short}…") } else { short };
        Some(Account { hint: Some(hint), label: self.label.clone(), ..Account::new("command", &self.command) })
    }

    async fn fetch(&self, _http: &reqwest::Client) -> Result<Snapshot, Error> {
        let run = tokio::process::Command::new("/bin/sh")
            .args(["-c", &self.command])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(COMMAND_TIMEOUT, run)
            .await
            .map_err(|_| anyhow!("`{}` took longer than {}s", self.command, COMMAND_TIMEOUT.as_secs()))?
            .with_context(|| format!("could not run `{}`", self.command))?;
        if !out.status.success() {
            let err: String = String::from_utf8_lossy(&out.stderr).chars().take(200).collect();
            return Err(anyhow!("`{}` exited with {}: {}", self.command, out.status, err.trim()).into());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let now = Utc::now();
        let snapshot = |plan, windows, balances| Snapshot {
            plan,
            windows,
            balances,
            source: Source::Live,
            as_of: now,
            note: None,
            perks: vec![],
        };
        if self.map.is_empty() {
            let n: Native = serde_json::from_str(&text).context("output is not juicemeter's format")?;
            let windows = n
                .windows
                .into_iter()
                .map(|w| Window {
                    id: w.label.to_lowercase().replace(' ', "_"),
                    label: w.label,
                    used_percent: w.used_percent.clamp(0.0, 100.0),
                    window_seconds: w.window_seconds,
                    resets_at: w.resets_at,
                })
                .collect();
            return Ok(snapshot(n.plan, windows, n.balances));
        }
        let v: Value = serde_json::from_str(&text).context("output is not JSON")?;
        let (plan, windows, balances) = self.map.apply(&v, now)?;
        Ok(snapshot(plan, windows, balances))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn win(label: &str) -> WindowMap {
        WindowMap {
            label: label.into(),
            used: None,
            left: None,
            fraction: false,
            of: None,
            resets_at: None,
            resets_in: None,
            length: None,
        }
    }

    #[test]
    fn maps_windows_and_balances() {
        let v = json!({
            "plan": "pro",
            "limits": {"weekly": {"usage": 0.25, "reset": 1790000000}, "session": {"left": "60", "in": 3600}},
            "balance_infos": [{"total_balance": "4.89", "currency": "USD"}]
        });
        let map = Mapping {
            plan: Some("/plan".into()),
            windows: vec![
                WindowMap {
                    used: Some("/limits/weekly/usage".into()),
                    fraction: true,
                    resets_at: Some("/limits/weekly/reset".into()),
                    length: Some("7d".into()),
                    ..win("Weekly")
                },
                WindowMap {
                    left: Some("/limits/session/left".into()),
                    resets_in: Some("/limits/session/in".into()),
                    ..win("Session")
                },
            ],
            balances: vec![BalanceMap {
                label: "Balance".into(),
                amount: "/balance_infos/0/total_balance".into(),
                currency: Some("/balance_infos/0/currency".into()),
            }],
        };
        let now = Utc::now();
        let (plan, w, b) = map.apply(&v, now).unwrap();
        assert_eq!(plan.as_deref(), Some("pro"));
        assert_eq!((w[0].used_percent, w[0].window_seconds), (25.0, Some(604800)));
        assert_eq!(w[0].resets_at.unwrap().timestamp(), 1790000000);
        assert_eq!(w[1].used_percent, 40.0);
        assert!((w[1].resets_at.unwrap() - now).num_seconds() >= 3599);
        assert_eq!((b[0].amount, b[0].display()), (4.89, "$4.89".to_string()));
    }

    #[test]
    fn missing_fields_are_clear_errors() {
        let map = Mapping { windows: vec![WindowMap { used: Some("/nope".into()), ..win("W") }], ..Default::default() };
        let e = map.apply(&json!({}), Utc::now()).unwrap_err().to_string();
        assert!(e.contains("/nope"), "{e}");
    }

    #[test]
    fn timestamps_in_seconds_millis_and_text() {
        assert_eq!(timestamp(&json!(1790000000)).unwrap().timestamp(), 1790000000);
        assert_eq!(timestamp(&json!(1790000000123u64)).unwrap().timestamp(), 1790000000);
        assert_eq!(timestamp(&json!("2026-10-06T03:59:59Z")).unwrap().timestamp(), 1791259199);
        assert_eq!(timestamp(&json!("2026-10-06")).unwrap().timestamp(), 1791244800);
    }

    #[test]
    fn counts_out_of_a_total() {
        // MiniMax-style: the "usage" count is what's left, out of a total.
        let v = json!({"m": [{"usage_count": 1459, "total_count": 1500}]});
        let map = Mapping {
            windows: vec![WindowMap {
                left: Some("/m/0/usage_count".into()),
                of: Some("/m/0/total_count".into()),
                ..win("Plan")
            }],
            ..Default::default()
        };
        let (_, w, _) = map.apply(&v, Utc::now()).unwrap();
        assert!((w[0].used_percent - 2.733).abs() < 0.01, "{}", w[0].used_percent);
    }

    #[tokio::test]
    async fn command_in_native_format() {
        let c = Command {
            name: "Test".into(),
            command: r#"echo '{"plan":"x","windows":[{"label":"Day","used_percent":30}],"balances":[{"label":"B","amount":2,"currency":"EUR"}]}'"#.into(),
            label: None,
            map: Mapping::default(),
        };
        let s = c.fetch(&reqwest::Client::new()).await.unwrap();
        assert_eq!((s.windows[0].used_percent, s.balances[0].display()), (30.0, "€2.00".to_string()));
    }

    #[tokio::test]
    async fn failing_command_reports_its_error() {
        let c =
            Command { name: "T".into(), command: "echo boom >&2; exit 3".into(), label: None, map: Mapping::default() };
        let e = c.fetch(&reqwest::Client::new()).await.unwrap_err().to_string();
        assert!(e.contains("boom"), "{e}");
    }
}
