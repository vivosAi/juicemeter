//! `juicemeter setup`: check this machine and the tailnet, fix what can be fixed.
//! Asks before every change; never runs sudo without an explicit yes.

use crate::ask::confirm;
use juicemeter_core::tailnet::{self, Found};
use juicemeter_core::Config;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct Opts {
    /// Accept the defaults (never sudo).
    yes: bool,
    /// Only report.
    check: bool,
}

impl Opts {
    fn ask(&self, question: &str, default: bool) -> anyhow::Result<bool> {
        if self.check {
            return Ok(false);
        }
        if self.yes {
            return Ok(default);
        }
        confirm(question, default)
    }
}

fn ok(msg: &str) {
    println!("✓ {msg}");
}
fn warn(msg: &str) {
    println!("⚠ {msg}");
}
fn bad(msg: &str) {
    println!("✗ {msg}");
}
fn info(msg: &str) {
    println!("  {msg}");
}

pub async fn run(config: &Config, yes: bool, check: bool) -> anyhow::Result<()> {
    let opts = Opts { yes, check };

    // 1. Tailscale
    let net = match tailnet::status() {
        Ok(n) => {
            ok(&format!("Tailscale is running; this machine is \"{}\" ({})", n.this.address, n.this.display));
            Some(n)
        }
        Err(e) => {
            bad(&format!("{e:#}"));
            info("juicemeter works on one machine without it; Tailscale is how machines see each other.");
            None
        }
    };

    // 2. The agent on this machine
    let running = local_agent_answers().await;
    if running {
        ok("juicemeter-agent is running here");
    } else {
        match agent_bin() {
            Some(bin) => {
                warn("juicemeter-agent isn't running, so other machines can't read this one");
                if opts.ask("Install it as a login service and start it now?", true)? {
                    let status = Command::new(&bin).arg("install").status()?;
                    if status.success() {
                        ok("juicemeter-agent installed and started");
                    } else {
                        bad("installing the agent failed (see above)");
                    }
                } else {
                    info(&format!("To do it later: {} install", bin.display()));
                }
            }
            None => {
                warn("juicemeter-agent isn't installed");
                info("Install it next to juicemeter (see the README), then run `juicemeter setup` again.");
            }
        }
    }

    // 3. The firewall (macOS)
    if cfg!(target_os = "macos") {
        check_macos_firewall(&opts)?;
    }

    // 4. Other machines
    if let Some(net) = net {
        find_machines(config, &net, &opts).await?;
    }

    if !opts.check {
        println!("\nDone. `juicemeter` shows every machine you read; `juicemeter setup --check` re-runs these checks.");
    }
    Ok(())
}

async fn local_agent_answers() -> bool {
    let url = format!("http://127.0.0.1:{}/healthz", juicemeter_core::DEFAULT_PORT);
    let http =
        juicemeter_core::reqwest::Client::builder().timeout(Duration::from_secs(2)).build().expect("http client");
    http.get(url).send().await.is_ok_and(|r| r.status().is_success())
}

/// `juicemeter-agent`, next to this binary or on the PATH.
fn agent_bin() -> Option<PathBuf> {
    let sibling = std::env::current_exe().ok()?.with_file_name("juicemeter-agent");
    if sibling.is_file() {
        return Some(sibling);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join("juicemeter-agent")).find(|p| p.is_file())
}

/// The binary launchd starts, which is what the firewall rule must name.
fn installed_agent_path() -> Option<PathBuf> {
    let plist = dirs::home_dir()?.join("Library/LaunchAgents/dev.juicemeter.agent.plist");
    let text = std::fs::read_to_string(plist).ok()?;
    let start = text.find("<array><string>")? + "<array><string>".len();
    let end = text[start..].find("</string>")? + start;
    Some(PathBuf::from(&text[start..end]))
}

fn check_macos_firewall(opts: &Opts) -> anyhow::Result<()> {
    const FW: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";
    let out = |args: &[&str]| -> String {
        Command::new(FW)
            .args(args)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    if !out(&["--getglobalstate"]).contains("enabled") {
        ok("macOS firewall is off; nothing to allow");
        return Ok(());
    }
    let Some(agent) = installed_agent_path().or_else(agent_bin) else { return Ok(()) };
    let agent_str = agent.display().to_string();
    if allowed_in(&out(&["--listapps"]), &agent_str) {
        ok("macOS firewall lets other machines reach the agent");
        return Ok(());
    }
    warn("macOS firewall is on and doesn't allow the agent: other machines get \"connection reset\"");
    info("This allows only juicemeter-agent; it still listens on localhost and Tailscale only.");
    let cmds = [vec!["--add", agent_str.as_str()], vec!["--unblockapp", agent_str.as_str()]];
    for c in &cmds {
        info(&format!("sudo {FW} {}", c.join(" ")));
    }
    // sudo is never assumed, even with --yes.
    if !opts.check && confirm("Run these now? (macOS will ask for your password)", false)? {
        for c in &cmds {
            Command::new("sudo").arg(FW).args(c).status()?;
        }
        if allowed_in(&out(&["--listapps"]), &agent_str) {
            ok("agent allowed through the firewall");
        } else {
            bad("the firewall still doesn't list the agent as allowed");
        }
    }
    Ok(())
}

/// Whether `--listapps` output lists `path` with incoming connections allowed.
fn allowed_in(listing: &str, path: &str) -> bool {
    let mut lines = listing.lines();
    while let Some(line) = lines.next() {
        if line.trim_end().ends_with(path) {
            return lines.next().is_some_and(|l| l.contains("Allow incoming"));
        }
    }
    false
}

async fn find_machines(config: &Config, net: &tailnet::Tailnet, opts: &Opts) -> anyhow::Result<()> {
    println!("\nLooking for other machines on your tailnet…");
    let found: Vec<Found> = tailnet::find_agents(&net.peers).await;
    for f in &found {
        let d = &f.device;
        let what = match (d.online, f.agent) {
            (_, true) => "juicemeter agent found",
            (true, false) => "no agent",
            (false, _) => "offline",
        };
        let mark = if f.agent { "✓" } else { "·" };
        let known = if config.hosts.contains(&d.address) { ", already read" } else { "" };
        println!("  {mark} {:<24} {:<8} {what}{known}", d.address, d.os);
    }
    let new: Vec<String> = found
        .iter()
        .filter(|f| f.agent && !config.hosts.contains(&f.device.address))
        .map(|f| f.device.address.clone())
        .collect();
    if new.is_empty() {
        if !found.iter().any(|f| f.agent) {
            info("No other agents answered. Install juicemeter on your other machines and run setup there too.");
        }
        return Ok(());
    }
    if opts.ask(&format!("Read {} from this machine?", new.join(", ")), true)? {
        let mut hosts = config.hosts.clone();
        hosts.extend(new);
        juicemeter_core::edit::set_list("hosts", &hosts, &[])?;
        ok("added; `juicemeter` now shows them too");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_firewall_app_list() {
        let listing = "Total number of apps = 2 \n1 : /usr/libexec/dhcp6d \n             (Allow incoming connections)\n2 : /Users/me/.cargo/bin/juicemeter-agent \n             (Block incoming connections)\n";
        assert!(super::allowed_in(listing, "/usr/libexec/dhcp6d"));
        assert!(!super::allowed_in(listing, "/Users/me/.cargo/bin/juicemeter-agent"));
        assert!(!super::allowed_in(listing, "/missing"));
    }
}
