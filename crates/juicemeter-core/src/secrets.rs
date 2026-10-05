//! Credential lookup. Built-in API key sources resolve in order:
//! 1. process environment variable
//! 2. env file (`$JUICEMETER_ENV_FILE`, default `~/.config/juicemeter/juicemeter.env`)
//! 3. macOS Keychain item with service `juicemeter` and the given account name
//!
//! Extra sources point at a key explicitly with a [`KeyRef`].

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn env_file_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("JUICEMETER_ENV_FILE") {
        return Some(PathBuf::from(p));
    }
    Some(crate::config::config_dir()?.join("juicemeter.env"))
}

/// Resolve an API key from env var `var`, the env file, then Keychain account `account`.
/// Returns the key and a description of where it was found.
pub fn api_key(var: &str, account: &str) -> Option<(String, String)> {
    if let Some(k) = non_empty(std::env::var(var).ok()) {
        return Some((k, format!("env {var}")));
    }
    if let Some(path) = env_file_path() {
        if let Some(k) = env_file_lookup(&path, var) {
            return Some((k, format!("{}:{var}", tilde(&path))));
        }
    }
    keychain("juicemeter", Some(account)).map(|k| (k, format!("keychain juicemeter/{account}")))
}

/// Where an extra source's API key lives. Never the key itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyRef {
    /// `{ env_file = "~/.someagent/.env", var = "DEEPSEEK_API_KEY" }`
    EnvFile { env_file: String, var: String },
    /// `{ env = "DEEPSEEK_API_KEY_WORK" }`
    Env { env: String },
    /// `{ keychain = "juicemeter", account = "deepseek-work" }`
    Keychain { keychain: String, account: Option<String> },
    /// `{ json_file = "~/.tool/config.json", pointer = "/providers/deepseek/api_key" }`
    JsonFile { json_file: String, pointer: String },
    /// `{ command = "op read op://Private/DeepSeek/credential" }` — hand-written config only.
    Command { command: String },
}

impl KeyRef {
    pub fn resolve(&self) -> anyhow::Result<String> {
        let key = match self {
            KeyRef::Env { env } => std::env::var(env).ok(),
            KeyRef::EnvFile { env_file, var } => env_file_lookup(&expand(env_file), var),
            KeyRef::Keychain { keychain: service, account } => keychain(service, account.as_deref()),
            KeyRef::JsonFile { json_file, pointer } => {
                let text =
                    std::fs::read_to_string(expand(json_file)).with_context(|| format!("reading {json_file}"))?;
                let v: serde_json::Value =
                    serde_json::from_str(&text).with_context(|| format!("parsing {json_file}"))?;
                v.pointer(pointer).and_then(|v| v.as_str()).map(str::to_owned)
            }
            KeyRef::Command { command } => {
                let out = Command::new("/bin/sh").args(["-c", command]).output()?;
                if !out.status.success() {
                    bail!("`{command}` exited with {}", out.status);
                }
                Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
            }
        };
        non_empty(key).with_context(|| format!("no key found at {}", self.describe()))
    }

    pub fn describe(&self) -> String {
        match self {
            KeyRef::Env { env } => format!("env {env}"),
            KeyRef::EnvFile { env_file, var } => format!("{env_file}:{var}"),
            KeyRef::Keychain { keychain, account } => match account {
                Some(a) => format!("keychain {keychain}/{a}"),
                None => format!("keychain {keychain}"),
            },
            KeyRef::JsonFile { json_file, pointer } => format!("{json_file}#{pointer}"),
            KeyRef::Command { command } => format!("command `{command}`"),
        }
    }
}

fn env_file_lookup(path: &Path, var: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (k, v) = line.split_once('=')?;
        if k.trim() != var {
            return None;
        }
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(v);
        non_empty(Some(v.to_string()))
    })
}

/// Read a generic password from the macOS Keychain. Returns None elsewhere or if missing.
pub fn keychain(service: &str, account: Option<&str>) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let mut cmd = Command::new("/usr/bin/security");
    cmd.args(["find-generic-password", "-s", service]);
    if let Some(a) = account {
        cmd.args(["-a", a]);
    }
    let out = cmd.arg("-w").output().ok()?;
    if !out.status.success() {
        return None;
    }
    non_empty(Some(String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

/// Expand a leading `~/`.
pub fn expand(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// Shorten a path under the home directory to `~/…` for display.
pub fn tilde(path: &Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// `sk-…a1b2`
pub fn mask(key: &str) -> String {
    let prefix: String = key.chars().take_while(|c| *c != '-').take(4).collect();
    let tail: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    if key.contains('-') && prefix.len() < key.len() {
        format!("{prefix}-…{tail}")
    } else {
        format!("…{tail}")
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_file_variants() {
        let dir = std::env::temp_dir().join(format!("juicemeter-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("juicemeter.env");
        std::fs::write(&f, "# c\nexport A_KEY=\"q1\"\nB_KEY='q2'\nC_KEY = plain \nEMPTY=\n").unwrap();
        assert_eq!(env_file_lookup(&f, "A_KEY").as_deref(), Some("q1"));
        assert_eq!(env_file_lookup(&f, "B_KEY").as_deref(), Some("q2"));
        assert_eq!(env_file_lookup(&f, "C_KEY").as_deref(), Some("plain"));
        assert_eq!(env_file_lookup(&f, "EMPTY"), None);
        assert_eq!(env_file_lookup(&f, "MISSING"), None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn key_refs_parse_from_toml() {
        let parse = |s: &str| toml::from_str::<std::collections::HashMap<String, KeyRef>>(s).unwrap()["k"].clone();
        assert!(matches!(parse(r#"k = { env = "X" }"#), KeyRef::Env { .. }));
        assert!(matches!(parse(r#"k = { env_file = "~/.e", var = "X" }"#), KeyRef::EnvFile { .. }));
        assert!(matches!(parse(r#"k = { keychain = "s" }"#), KeyRef::Keychain { account: None, .. }));
        assert!(matches!(parse(r#"k = { json_file = "f", pointer = "/a" }"#), KeyRef::JsonFile { .. }));
        assert!(matches!(parse(r#"k = { command = "echo" }"#), KeyRef::Command { .. }));
    }

    #[test]
    fn masks_keys() {
        assert_eq!(mask("sk-abcdef1234"), "sk-…1234");
        assert_eq!(mask("abcdef1234"), "…1234");
    }
}
