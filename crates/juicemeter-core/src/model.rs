use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Bump on breaking changes to the JSON shape.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub host: String,
    pub generated_at: DateTime<Utc>,
    pub providers: Vec<ProviderReport>,
}

impl Report {
    pub fn new(providers: Vec<ProviderReport>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            host: gethostname::gethostname().to_string_lossy().into_owned(),
            generated_at: Utc::now(),
            providers,
        }
    }

    /// Apply user labels (keyed by account id), overriding any label the source supplied.
    pub fn apply_labels(&mut self, labels: &BTreeMap<String, String>) {
        for a in self.providers.iter_mut().filter_map(|p| p.account.as_mut()) {
            if let Some(l) = labels.get(&a.id) {
                a.label = Some(l.clone());
            }
        }
    }

    pub fn redact_emails(&mut self) {
        for a in self.providers.iter_mut().filter_map(|p| p.account.as_mut()) {
            a.email = None;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderReport {
    /// Provider kind, e.g. `"codex"`.
    pub provider: String,
    pub name: String,
    /// Where this machine keeps the credentials, e.g. `"~/.codex"`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// Who a source belongs to. `id` is stable and safe to share; the rest is for display.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// `<provider>:<hash of the provider's own account id or API key>`
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// Masked API key, e.g. `sk-…a1b2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Account {
    pub fn new(provider: &str, native_id: &str) -> Self {
        let digest = Sha256::digest(native_id.as_bytes());
        let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
        Self { id: format!("{provider}:{hex}"), ..Default::default() }
    }

    /// Best human-readable name: label, then email, then key hint, then id.
    pub fn display(&self) -> &str {
        self.label.as_deref().or(self.email.as_deref()).or(self.hint.as_deref()).unwrap_or(&self.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Ok { snapshot: Snapshot },
    NotConfigured { hint: String },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    pub balances: Vec<Balance>,
    pub source: Source,
    /// When the data was true. Equals fetch time for live data; older for log fallbacks.
    pub as_of: DateTime<Utc>,
    /// Caveats worth showing the user (e.g. why a fallback source was used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Things you have that expire if unused, e.g. free limit resets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub perks: Vec<Perk>,
}

/// Something worth using before it expires, like Codex's free limit resets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Perk {
    pub label: String,
    pub count: u32,
    /// When the next one expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// A rate-limit window such as "5-hour session" or "weekly".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub id: String,
    pub label: String,
    pub used_percent: f64,
    pub window_seconds: Option<u64>,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Balance {
    pub label: String,
    pub amount: f64,
    pub currency: String,
}

impl Balance {
    /// `$4.93`, `€12.00`, `¥80.00`; other currencies as `4.93 XYZ`.
    pub fn display(&self) -> String {
        match self.currency.as_str() {
            "USD" => format!("${:.2}", self.amount),
            "EUR" => format!("€{:.2}", self.amount),
            "GBP" => format!("£{:.2}", self.amount),
            "CNY" | "JPY" => format!("¥{:.2}", self.amount),
            c => format!("{:.2} {c}", self.amount),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Live,
    LocalLog,
}
