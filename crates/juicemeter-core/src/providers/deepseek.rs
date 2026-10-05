//! DeepSeek API balance via the documented `GET /user/balance` endpoint.

use super::{check, Error, Provider};
use crate::model::{Account, Balance, Snapshot, Source};
use crate::secrets::KeyRef;
use anyhow::Context;
use chrono::Utc;
use serde::Deserialize;

const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";
pub const KEY_VAR: &str = "DEEPSEEK_API_KEY";

/// A DeepSeek API key. `key: None` means the default lookup (env var, env file, Keychain).
pub struct DeepSeek {
    key: Option<KeyRef>,
    label: Option<String>,
}

impl DeepSeek {
    pub fn new(key: Option<KeyRef>, label: Option<String>) -> Self {
        Self { key, label }
    }

    /// The key and where it was found.
    fn resolve(&self) -> Result<(String, String), Error> {
        match &self.key {
            Some(k) => Ok((k.resolve()?, k.describe())),
            None => crate::secrets::api_key(KEY_VAR, "deepseek").ok_or_else(|| {
                Error::NotConfigured(format!(
                    "set {KEY_VAR} in the environment or the juicemeter env file, \
                     or run `security add-generic-password -s juicemeter -a deepseek -w`"
                ))
            }),
        }
    }
}

#[derive(Deserialize)]
struct BalanceResponse {
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<BalanceInfo>,
}

#[derive(Deserialize)]
struct BalanceInfo {
    currency: String,
    total_balance: String,
}

#[async_trait::async_trait]
impl Provider for DeepSeek {
    fn kind(&self) -> &'static str {
        "deepseek"
    }
    fn name(&self) -> &str {
        "DeepSeek"
    }
    fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(30 * 60)
    }

    fn source(&self) -> String {
        match (&self.key, self.resolve()) {
            (Some(k), _) => k.describe(),
            (None, Ok((_, origin))) => origin,
            (None, Err(_)) => "default".into(),
        }
    }

    fn account(&self) -> Option<Account> {
        let (key, _) = self.resolve().ok()?;
        Some(Account {
            hint: Some(crate::secrets::mask(&key)),
            label: self.label.clone(),
            ..Account::new("deepseek", &key)
        })
    }

    async fn fetch(&self, http: &reqwest::Client) -> Result<Snapshot, Error> {
        let (key, _) = self.resolve()?;
        let resp = http.get(BALANCE_URL).bearer_auth(key).send().await.context("request failed")?;
        let b: BalanceResponse =
            check(resp, "check the DeepSeek API key").await?.json().await.context("unexpected response")?;
        let balances = b
            .balance_infos
            .into_iter()
            .filter_map(|i| {
                Some(Balance { label: "Balance".into(), amount: i.total_balance.parse().ok()?, currency: i.currency })
            })
            .collect();
        Ok(Snapshot {
            perks: vec![],
            plan: None,
            windows: vec![],
            balances,
            source: Source::Live,
            as_of: Utc::now(),
            note: (!b.is_available).then(|| "balance insufficient for API calls".into()),
        })
    }
}
