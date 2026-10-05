//! Install the agent as a per-user login service.

use anyhow::{bail, Context};
use std::path::PathBuf;
use std::process::Command;

#[cfg(target_os = "macos")]
const LABEL: &str = "dev.juicemeter.agent";

fn exe() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe()?.canonicalize()?;
    if exe.components().any(|c| c.as_os_str() == "target") {
        eprintln!(
            "warning: installing a binary from a build directory ({}); \
             `cargo install --path crates/juicemeter-agent` first gives a stable path",
            exe.display()
        );
    }
    Ok(exe)
}

#[cfg(target_os = "macos")]
fn home() -> anyhow::Result<PathBuf> {
    dirs::home_dir().context("no home directory")
}

fn run(cmd: &mut Command) -> anyhow::Result<()> {
    let status = cmd.status().with_context(|| format!("running {cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?} failed with {status}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn plist_path() -> anyhow::Result<PathBuf> {
    Ok(home()?.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn domain() -> anyhow::Result<String> {
    let out = Command::new("/usr/bin/id").arg("-u").output()?;
    Ok(format!("gui/{}", String::from_utf8_lossy(&out.stdout).trim()))
}

#[cfg(target_os = "macos")]
pub fn install() -> anyhow::Result<()> {
    let exe = exe()?;
    let log = home()?.join("Library/Logs/juicemeter-agent.log");
    let plist = plist_path()?;
    std::fs::create_dir_all(plist.parent().unwrap())?;
    std::fs::write(
        &plist,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{exe}</string><string>run</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
            exe = exe.display(),
            log = log.display()
        ),
    )?;
    let domain = domain()?;
    // Replace a previous install if present; on a first install there is
    // nothing to boot out, so launchctl's "No such process" is just noise.
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LABEL}")])
        .stderr(std::process::Stdio::null())
        .status();
    run(Command::new("launchctl").args(["bootstrap", &domain]).arg(&plist))?;
    println!("installed {}\nlogs: {}", plist.display(), log.display());
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> anyhow::Result<()> {
    let _ = Command::new("launchctl").args(["bootout", &format!("{}/{LABEL}", domain()?)]).status();
    let plist = plist_path()?;
    if plist.exists() {
        std::fs::remove_file(&plist)?;
    }
    println!("uninstalled");
    Ok(())
}

#[cfg(target_os = "linux")]
fn unit_path() -> anyhow::Result<PathBuf> {
    Ok(dirs::config_dir().context("no config dir")?.join("systemd/user/juicemeter-agent.service"))
}

#[cfg(target_os = "linux")]
pub fn install() -> anyhow::Result<()> {
    let unit = unit_path()?;
    std::fs::create_dir_all(unit.parent().unwrap())?;
    std::fs::write(
        &unit,
        format!(
            "[Unit]\nDescription=juicemeter agent\nAfter=network-online.target\n\n\
             [Service]\nExecStart={} run\nRestart=always\nRestartSec=10\n\n\
             [Install]\nWantedBy=default.target\n",
            exe()?.display()
        ),
    )?;
    run(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
    run(Command::new("systemctl").args(["--user", "enable", "--now", "juicemeter-agent"]))?;
    println!("installed {}\nlogs: journalctl --user -u juicemeter-agent", unit.display());
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn uninstall() -> anyhow::Result<()> {
    let _ = Command::new("systemctl").args(["--user", "disable", "--now", "juicemeter-agent"]).status();
    let unit = unit_path()?;
    if unit.exists() {
        std::fs::remove_file(&unit)?;
    }
    println!("uninstalled");
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn install() -> anyhow::Result<()> {
    bail!("service install is only supported on macOS and Linux")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn uninstall() -> anyhow::Result<()> {
    bail!("service install is only supported on macOS and Linux")
}
