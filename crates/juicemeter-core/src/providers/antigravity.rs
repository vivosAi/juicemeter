//! Antigravity (Google's agent IDE, Gemini Code Assist under the hood): per-model weekly
//! quota from the Cloud Code endpoints the IDE itself uses (undocumented). Reads the login
//! Antigravity keeps in the Keychain; never refreshes it, so the IDE stays in charge of it.

use super::{check, Error, Provider};
use crate::model::{Account, Snapshot, Source, Window};
use anyhow::{anyhow, Context};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;

const BASE: &str = "https://daily-cloudcode-pa.googleapis.com";
const KEYCHAIN_SERVICE: &str = "gemini";
const KEYCHAIN_ACCOUNT: &str = "antigravity";
const AUTH_HINT: &str = "open Antigravity so it renews its login";
/// Quota buckets reset weekly; the endpoint doesn't say how long the window is.
const WINDOW_SECS: u64 = 7 * 86400;

pub struct Antigravity {
    pub label: Option<String>,
}

#[derive(Deserialize)]
struct Stored {
    token: Option<Token>,
    #[serde(flatten)]
    flat: Token,
}

#[derive(Deserialize, Default)]
struct Token {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expiry: Option<String>,
}

struct Login {
    access_token: String,
    expiry: Option<DateTime<Utc>>,
    /// Stable for the life of this sign-in; used only, hashed, to tell logins apart.
    refresh_token: Option<String>,
}

/// The login Antigravity stored. The item may be wrapped as `go-keyring-base64:<base64 JSON>`.
fn stored_login() -> Option<Login> {
    let raw = crate::secrets::keychain(KEYCHAIN_SERVICE, Some(KEYCHAIN_ACCOUNT))?;
    parse_stored(&raw)
}

fn parse_stored(raw: &str) -> Option<Login> {
    let json = match raw.trim().strip_prefix("go-keyring-base64:") {
        Some(b64) => String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?).ok()?,
        None => raw.trim().to_string(),
    };
    let s: Stored = serde_json::from_str(&json).ok()?;
    let t = s.token.unwrap_or(s.flat);
    let expiry = t.expiry.as_deref().and_then(|e| DateTime::parse_from_rfc3339(e).ok()).map(|d| d.with_timezone(&Utc));
    Some(Login { access_token: t.access_token?, expiry, refresh_token: t.refresh_token })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadCodeAssist {
    cloudaicompanion_project: Option<String>,
    current_tier: Option<Tier>,
}

#[derive(Deserialize)]
struct Tier {
    id: Option<String>,
}

#[derive(Deserialize)]
struct Quota {
    #[serde(default)]
    buckets: Vec<Bucket>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    model_id: Option<String>,
    remaining_fraction: Option<f64>,
    reset_time: Option<DateTime<Utc>>,
}

/// One window per model family, at its most-used model: Claude models, Gemini models.
fn windows(buckets: Vec<Bucket>) -> Vec<Window> {
    let mut out: Vec<Window> = Vec::new();
    for b in buckets {
        let (Some(model), Some(left), Some(reset)) = (b.model_id, b.remaining_fraction, b.reset_time) else { continue };
        let family = if model.starts_with("claude") {
            "Claude models"
        } else if model.starts_with("gemini") {
            "Gemini models"
        } else {
            continue;
        };
        let used = ((1.0 - left) * 100.0).clamp(0.0, 100.0);
        match out.iter_mut().find(|w| w.label == family) {
            Some(w) if used > w.used_percent => {
                w.used_percent = used;
                w.resets_at = Some(reset);
            }
            Some(_) => {}
            None => out.push(Window {
                id: family.to_lowercase().replace(' ', "_"),
                label: family.into(),
                used_percent: used,
                window_seconds: Some(WINDOW_SECS),
                resets_at: Some(reset),
            }),
        }
    }
    // Gemini first: it's the IDE's own.
    out.sort_by_key(|w| w.label != "Gemini models");
    out
}

async fn post<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    token: &str,
    path: &str,
    body: serde_json::Value,
) -> anyhow::Result<T> {
    let resp = http
        .post(format!("{BASE}/v1internal:{path}"))
        .bearer_auth(token)
        .header("User-Agent", "antigravity")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("{path} request failed"))?;
    check(resp, AUTH_HINT).await?.json().await.with_context(|| format!("unexpected {path} response"))
}

#[async_trait::async_trait]
impl Provider for Antigravity {
    fn kind(&self) -> &'static str {
        "antigravity"
    }
    fn name(&self) -> &str {
        "Antigravity"
    }
    fn source(&self) -> String {
        format!("keychain {KEYCHAIN_SERVICE}/{KEYCHAIN_ACCOUNT}")
    }
    fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(10 * 60)
    }

    /// Nothing stored locally names the Google account, so the sign-in itself identifies it.
    /// The same account signed in on two machines shows twice; two accounts never merge.
    fn account(&self) -> Option<Account> {
        let login = stored_login()?;
        let native = login.refresh_token.unwrap_or(login.access_token);
        Some(Account { label: self.label.clone(), ..Account::new("antigravity", &native) })
    }

    async fn fetch(&self, http: &reqwest::Client) -> Result<Snapshot, Error> {
        let login =
            stored_login().ok_or_else(|| Error::NotConfigured("sign in to Antigravity on this machine".into()))?;
        let token = login.access_token;
        if login.expiry.is_some_and(|e| e <= Utc::now()) {
            return Err(anyhow!("stored login expired, {AUTH_HINT}").into());
        }
        let lca: LoadCodeAssist = post(http, &token, "loadCodeAssist", serde_json::json!({})).await?;
        let body = match &lca.cloudaicompanion_project {
            Some(p) => serde_json::json!({ "project": p }),
            None => serde_json::json!({}),
        };
        let quota: Quota = post(http, &token, "retrieveUserQuota", body).await?;
        let plan = lca.current_tier.and_then(|t| t.id).map(|id| id.trim_end_matches("-tier").to_string());
        Ok(Snapshot {
            perks: vec![],
            plan,
            windows: windows(quota.buckets),
            balances: vec![],
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
    fn reads_wrapped_and_plain_tokens() {
        let json = r#"{"access_token":"ya29.x","expiry":"2026-10-03T01:34:43.34493+02:00","token_type":"Bearer"}"#;
        let wrapped = format!("go-keyring-base64:{}", base64::engine::general_purpose::STANDARD.encode(json));
        for raw in [json.to_string(), wrapped] {
            let l = parse_stored(&raw).unwrap();
            assert_eq!(l.access_token, "ya29.x");
            assert_eq!(l.expiry.unwrap().to_rfc3339(), "2026-10-02T23:34:43.344930+00:00");
        }
    }

    #[test]
    fn groups_buckets_by_family_at_the_most_used_model() {
        let q: Quota = serde_json::from_str(
            r#"{"buckets":[
              {"tokenType":"WTUS","modelId":"chat_20706","remainingFraction":1},
              {"resetTime":"2026-10-09T22:36:07Z","modelId":"claude-sonnet-4-6","remainingFraction":0.8},
              {"resetTime":"2026-10-09T22:36:07Z","modelId":"claude-opus-4-6-thinking","remainingFraction":0.5},
              {"resetTime":"2026-10-09T22:36:07Z","modelId":"gemini-2.5-pro","remainingFraction":1}]}"#,
        )
        .unwrap();
        let w = windows(q.buckets);
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].label.as_str(), w[0].used_percent), ("Gemini models", 0.0));
        assert_eq!((w[1].label.as_str(), w[1].used_percent), ("Claude models", 50.0));
        assert_eq!(w[1].window_seconds, Some(604800));
    }
}
