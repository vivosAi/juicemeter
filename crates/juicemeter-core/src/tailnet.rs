//! What Tailscale knows: this machine, the other devices on the tailnet, and which of them
//! run a juicemeter agent. Read from the `tailscale` command line; nothing is changed.

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    /// What to put in `hosts`: the MagicDNS name, or the Tailscale IPv4 without MagicDNS.
    pub address: String,
    /// The name the device shows in Tailscale, e.g. "Studio Mac".
    pub display: String,
    pub os: String,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tailnet {
    pub this: Device,
    pub peers: Vec<Device>,
}

/// A device and whether a juicemeter agent answered on it.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    #[serde(flatten)]
    pub device: Device,
    pub agent: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Status {
    backend_state: String,
    #[serde(rename = "Self")]
    this: Option<Node>,
    #[serde(default)]
    peer: HashMap<String, Node>,
    current_tailnet: Option<CurrentTailnet>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CurrentTailnet {
    #[serde(rename = "MagicDNSEnabled", default)]
    magic_dns_enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Node {
    host_name: String,
    #[serde(rename = "DNSName", default)]
    dns_name: String,
    #[serde(rename = "TailscaleIPs", default)]
    tailscale_ips: Vec<String>,
    #[serde(rename = "OS", default)]
    os: String,
    #[serde(default)]
    online: bool,
}

impl Node {
    fn device(self, magic_dns: bool) -> Device {
        let short = self.dns_name.split('.').next().unwrap_or_default().to_string();
        let ipv4 = self.tailscale_ips.iter().find(|ip| ip.contains('.')).cloned();
        let address = match (magic_dns && !short.is_empty(), ipv4) {
            (true, _) | (false, None) => short,
            (false, Some(ip)) => ip,
        };
        Device { address, display: self.host_name, os: self.os, online: self.online }
    }
}

/// Where the `tailscale` command lives; the macOS app keeps it inside its bundle.
pub fn tailscale_bin() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("tailscale")).collect())
        .unwrap_or_default();
    candidates.extend(
        [
            "/opt/homebrew/bin/tailscale",
            "/usr/local/bin/tailscale",
            "/usr/bin/tailscale",
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ]
        .map(PathBuf::from),
    );
    candidates.into_iter().find(|p| p.is_file())
}

/// This machine and its tailnet. Errors say what to do: install Tailscale, or log in.
pub fn status() -> anyhow::Result<Tailnet> {
    let Some(bin) = tailscale_bin() else {
        bail!("Tailscale isn't installed: get it from https://tailscale.com/download");
    };
    let out =
        std::process::Command::new(&bin).args(["status", "--json"]).output().context("could not run tailscale")?;
    let status: Status =
        serde_json::from_slice(&out.stdout).context("unexpected output from `tailscale status --json`")?;
    if status.backend_state != "Running" {
        bail!(
            "Tailscale is {}: run `tailscale up` (or open the Tailscale app) and log in",
            status.backend_state.to_lowercase()
        );
    }
    let magic_dns = status.current_tailnet.is_some_and(|t| t.magic_dns_enabled);
    let this = status.this.context("Tailscale didn't report this machine")?.device(magic_dns);
    let mut peers: Vec<Device> = status.peer.into_values().map(|n| n.device(magic_dns)).collect();
    peers.sort_by(|a, b| (!a.online, &a.address).cmp(&(!b.online, &b.address)));
    Ok(Tailnet { this, peers })
}

/// Ask every online peer whether a juicemeter agent answers on it.
pub async fn find_agents(peers: &[Device]) -> Vec<Found> {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().expect("http client");
    let probes = peers.iter().map(|d| {
        let http = &http;
        async move {
            let agent = d.online
                && http
                    .get(format!("http://{}:{}/healthz", d.address, crate::DEFAULT_PORT))
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success());
            Found { device: d.clone(), agent }
        }
    });
    futures::future::join_all(probes).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "BackendState": "Running",
        "CurrentTailnet": {"MagicDNSSuffix": "tail0.ts.net", "MagicDNSEnabled": true},
        "Self": {"HostName": "Studio Mac", "DNSName": "studio-mac.tail0.ts.net.", "TailscaleIPs": ["100.1.2.3", "fd7a::1"], "OS": "macOS", "Online": true},
        "Peer": {
            "a": {"HostName": "phone", "DNSName": "phone.tail0.ts.net.", "TailscaleIPs": ["100.1.2.4"], "OS": "android", "Online": false},
            "b": {"HostName": "Mac mini", "DNSName": "mac-mini.tail0.ts.net.", "TailscaleIPs": ["100.1.2.5"], "OS": "macOS", "Online": true}
        }
    }"#;

    #[test]
    fn reads_devices_and_prefers_magic_dns_names() {
        let s: Status = serde_json::from_str(SAMPLE).unwrap();
        let magic = s.current_tailnet.as_ref().unwrap().magic_dns_enabled;
        assert!(magic);
        let this = s.this.unwrap().device(magic);
        assert_eq!((this.address.as_str(), this.display.as_str()), ("studio-mac", "Studio Mac"));
        let mini = s.peer.into_values().find(|n| n.host_name == "Mac mini").unwrap();
        assert_eq!(mini.device(false).address, "100.1.2.5", "without MagicDNS, use the IP");
    }
}
