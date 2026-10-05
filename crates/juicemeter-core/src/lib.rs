//! Collect AI subscription usage (rate-limit windows, balances) from local credentials.
//!
//! Each provider implements [`Provider`]; [`collect`] runs them concurrently and returns a
//! [`Report`] whose JSON form is the stable integration surface for CLIs, agents and UIs.

pub mod config;
pub mod edit;
pub mod fetch;
pub mod merge;
pub mod model;
pub mod pace;
pub mod providers;
pub mod secrets;
pub mod tailnet;

pub use config::Config;
pub use model::{Account, Balance, Outcome, Perk, ProviderReport, Report, Snapshot, Source, Window};
pub use providers::{Error, Provider, RateLimited};
pub use reqwest;

/// Port the agent listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 47878;

use std::time::Duration;

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("juicemeter/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}

/// Longest a provider's `Retry-After` is honoured for.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(2 * 3600);

/// Fetch one provider. Never fails: problems are reported in the outcome.
pub async fn fetch_one(p: &dyn Provider, http: &reqwest::Client) -> ProviderReport {
    fetch_one_paced(p, http).await.0
}

/// Like [`fetch_one`], plus how long the provider asked us to wait before trying again.
pub async fn fetch_one_paced(p: &dyn Provider, http: &reqwest::Client) -> (ProviderReport, Option<Duration>) {
    let mut wait = None;
    let outcome = match p.fetch(http).await {
        Ok(snapshot) => Outcome::Ok { snapshot },
        Err(Error::NotConfigured(hint)) => Outcome::NotConfigured { hint },
        Err(Error::Other(e)) => {
            wait = e.downcast_ref::<RateLimited>().and_then(|r| r.retry_after).map(|d| d.min(MAX_RETRY_AFTER));
            Outcome::Error { message: format!("{e:#}") }
        }
    };
    let report = ProviderReport {
        provider: p.kind().into(),
        name: p.name().into(),
        source: p.source(),
        account: p.account(),
        outcome,
    };
    (report, wait)
}

/// Fetch every provider concurrently.
pub async fn collect(providers: &[Box<dyn Provider>]) -> Report {
    let http = http_client();
    let reports = futures::future::join_all(providers.iter().map(|p| fetch_one(p.as_ref(), &http))).await;
    Report::new(reports)
}
