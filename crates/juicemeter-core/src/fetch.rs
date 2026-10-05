//! Read reports from this machine and from other machines' agents.

use crate::model::{Report, SCHEMA_VERSION};
use crate::{Config, DEFAULT_PORT};
use anyhow::{bail, Context};
use std::time::Duration;

/// Which machines to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Machines {
    /// This machine plus the `hosts` in the config.
    All,
    /// Only this machine.
    Local,
    /// Only these agents (`name` or `name:port`).
    Hosts(Vec<String>),
}

pub struct Gathered {
    pub reports: Vec<Report>,
    /// Agents that couldn't be read, with the reason.
    pub unreachable: Vec<(String, String)>,
}

/// Collect reports. This machine's comes from its agent's cache when one is running (unless
/// `fresh`), so repeated calls don't hit the providers. Labels from the config are applied.
pub async fn gather(config: &Config, machines: &Machines, fresh: bool) -> anyhow::Result<Gathered> {
    let targets: Vec<String> = match machines {
        Machines::All => config.hosts.clone(),
        Machines::Local => vec![],
        Machines::Hosts(h) => h.clone(),
    };
    let local = async {
        if matches!(machines, Machines::Hosts(_)) {
            return anyhow::Ok(None);
        }
        if !fresh {
            if let Ok(r) = local_agent().await {
                return Ok(Some(r));
            }
        }
        Ok(Some(crate::collect(&config.providers()?).await))
    };
    let (local, remote) = futures::join!(local, read_agents(&targets));
    let mut g = Gathered { reports: local?.into_iter().collect(), unreachable: vec![] };
    for (r, h) in remote.into_iter().zip(targets) {
        match r {
            Ok(r) => g.reports.push(r),
            Err(e) => g.unreachable.push((h, format!("{e:#}"))),
        }
    }
    for r in &mut g.reports {
        r.apply_labels(&config.labels);
    }
    Ok(g)
}

/// Ask this machine's agent and the configured hosts' agents to check their providers now.
/// Agents refuse refreshes closer than 30 seconds apart; those are ignored.
pub async fn refresh(config: &Config) {
    let http = crate::http_client();
    let mut urls = vec![format!("http://127.0.0.1:{DEFAULT_PORT}/v1/refresh")];
    urls.extend(config.hosts.iter().map(|h| agent_url(h, "refresh")));
    futures::future::join_all(urls.iter().map(|u| http.post(u).send())).await;
}

fn agent_url(host: &str, path: &str) -> String {
    if host.contains(':') {
        format!("http://{host}/v1/{path}")
    } else {
        format!("http://{host}:{DEFAULT_PORT}/v1/{path}")
    }
}

async fn local_agent() -> anyhow::Result<Report> {
    let http = reqwest::Client::builder().timeout(Duration::from_millis(500)).build()?;
    let r: Report = http
        .get(agent_url(&format!("127.0.0.1:{DEFAULT_PORT}"), "report"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    anyhow::ensure!(r.schema_version == SCHEMA_VERSION, "local agent is outdated");
    Ok(r)
}

async fn read_agents(hosts: &[String]) -> Vec<anyhow::Result<Report>> {
    let http = crate::http_client();
    futures::future::join_all(hosts.iter().map(|h| {
        let http = &http;
        async move {
            let r: Report = http
                .get(agent_url(h, "report"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await
                .context("unexpected report format (agent on an older version?)")?;
            if r.schema_version != SCHEMA_VERSION {
                bail!(
                    "agent speaks report schema {} but this build expects {SCHEMA_VERSION}; update both",
                    r.schema_version
                );
            }
            anyhow::Ok(r)
        }
    }))
    .await
}
