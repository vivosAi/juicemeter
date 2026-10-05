use crate::model::{Account, Snapshot};

#[cfg(feature = "antigravity")]
pub mod antigravity;
#[cfg(feature = "claude")]
pub mod claude;
#[cfg(feature = "codex")]
pub mod codex;
pub mod custom;
#[cfg(feature = "deepseek")]
pub mod deepseek;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No credentials found on this machine; the hint tells the user how to add them.
    #[error("not configured: {0}")]
    NotConfigured(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// One set of credentials for one provider. Implementations must only *read* credentials —
/// never refresh or rotate them, since that would log the owning tool (Claude Code, Codex, …) out.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Provider kind, e.g. `"claude"`.
    fn kind(&self) -> &'static str;
    fn name(&self) -> &str;
    /// Where the credentials live, e.g. `"~/.codex"`. Used for display and de-duplication.
    fn source(&self) -> String;
    /// Who the credentials belong to, from local data only (no network).
    fn account(&self) -> Option<Account>;
    /// How often a long-running agent should refresh this source.
    fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(5 * 60)
    }
    async fn fetch(&self, http: &reqwest::Client) -> Result<Snapshot, Error>;
}

/// Provider kinds compiled into this build.
#[allow(clippy::vec_init_then_push, unused_mut)]
pub fn kinds() -> Vec<&'static str> {
    let mut v = Vec::new();
    #[cfg(feature = "claude")]
    v.push("claude");
    #[cfg(feature = "codex")]
    v.push("codex");
    #[cfg(feature = "deepseek")]
    v.push("deepseek");
    #[cfg(feature = "antigravity")]
    v.push("antigravity");
    v.extend(["http", "command"]);
    v
}

/// The default credential source of every compiled-in provider.
#[allow(clippy::vec_init_then_push, unused_mut)]
pub fn defaults() -> Vec<Box<dyn Provider>> {
    let mut v: Vec<Box<dyn Provider>> = Vec::new();
    #[cfg(feature = "claude")]
    v.push(Box::new(claude::Claude::new(None, None, None)));
    #[cfg(feature = "codex")]
    if let Some(home) = codex::default_home() {
        v.push(Box::new(codex::Codex::new(home, None)));
    }
    #[cfg(feature = "deepseek")]
    v.push(Box::new(deepseek::DeepSeek::new(None, None)));
    #[cfg(feature = "antigravity")]
    v.push(Box::new(antigravity::Antigravity { label: None }));
    v
}

/// The provider asked us to slow down. Long-running callers should wait `retry_after`.
#[derive(Debug, thiserror::Error)]
#[error("HTTP 429: rate limited by the provider{}", match .retry_after {
    Some(d) => format!(", retrying in {}m", d.as_secs().div_ceil(60)),
    None => String::new(),
})]
pub struct RateLimited {
    pub retry_after: Option<std::time::Duration>,
}

/// Turn a non-2xx response into a readable error, flagging auth failures.
pub(crate) async fn check(resp: reqwest::Response, auth_hint: &str) -> anyhow::Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    if status.as_u16() == 429 {
        return Err(RateLimited { retry_after: retry_after(resp.headers()) }.into());
    }
    let body: String = resp.text().await.unwrap_or_default().chars().take(200).collect();
    match status.as_u16() {
        401 | 403 => anyhow::bail!("HTTP {status}: credentials rejected, {auth_hint}"),
        _ => anyhow::bail!("HTTP {status}: {body}"),
    }
}

/// `Retry-After` in seconds (the HTTP-date form is rare for APIs and ignored).
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
    let secs: u64 = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim().parse().ok()?;
    Some(std::time::Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

    #[test]
    fn reads_retry_after_seconds() {
        let mut h = HeaderMap::new();
        assert_eq!(retry_after(&h), None);
        h.insert(RETRY_AFTER, HeaderValue::from_static("1800"));
        assert_eq!(retry_after(&h), Some(std::time::Duration::from_secs(1800)));
        h.insert(RETRY_AFTER, HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"));
        assert_eq!(retry_after(&h), None);
    }

    #[test]
    fn rate_limit_message_and_downcast() {
        let e: anyhow::Error = RateLimited { retry_after: Some(std::time::Duration::from_secs(90)) }.into();
        assert_eq!(e.to_string(), "HTTP 429: rate limited by the provider, retrying in 2m");
        assert!(e.downcast_ref::<RateLimited>().is_some());
    }
}
