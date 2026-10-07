//! `~/.config/juicemeter/config.toml`:
//!
//! ```toml
//! hosts = ["studio-mac"]              # other machines running juicemeter-agent
//! auto_detect = true                  # use the default source of each provider
//! share_email = true                  # include account emails in served reports
//! show = "remaining"                  # or "used": what bars and percentages show
//! mode = "watch"                      # menu bar: watch, lowest, use_it, minimal, everything
//! bar_value = "both"                  # menu bar items: both, percent, time
//! bar_names = "full"                  # or "short": "Cl 79%" instead of "Claude 79%"
//! pins = ["claude:1a2b3c4d5e6f"]      # accounts the menu bar shows when nothing needs attention;
//!                                     # [] for none, unset for your Claude account
//!
//! [intervals]                         # how often the agent checks each provider
//! claude = "10m"
//! deepseek = "1h"
//!
//! [[source]]                          # extra credentials on this machine
//! provider = "codex"
//! home = "~/.codex-work"
//! label = "Work ChatGPT"
//!
//! [[source]]
//! provider = "deepseek"
//! key = { env_file = "~/.someagent/.env", var = "DEEPSEEK_API_KEY" }
//!
//! hidden = ["antigravity:0a1b2c3d4e5f"]  # accounts this machine doesn't show
//! disable = ["antigravity"]           # providers this machine doesn't check at all
//!
//! [labels]                            # names for accounts, by account id
//! "codex:3f9a1c2b4d5e" = "Personal ChatGPT"
//! ```

use crate::providers::Provider;
use crate::secrets::KeyRef;
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default = "yes")]
    pub auto_detect: bool,
    #[serde(default = "yes")]
    pub share_email: bool,
    #[serde(default)]
    pub show: Show,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub bar_value: BarValue,
    #[serde(default)]
    pub bar_names: BarNames,
    /// Accounts the menu bar shows. `None` (unset) means a sensible default; `[]` means none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pins: Option<Vec<String>>,
    /// Older single-account form of `pins`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    /// Poll interval per provider kind, e.g. `claude = "10m"`.
    #[serde(default)]
    pub intervals: BTreeMap<String, Interval>,
    #[serde(default, rename = "source")]
    pub sources: Vec<SourceConfig>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Accounts this machine doesn't show (ids, see `juicemeter accounts`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
    /// Providers this machine doesn't check at all, e.g. `["antigravity"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disable: Vec<String>,
}

/// Glass half full or half empty.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Show {
    #[default]
    Remaining,
    Used,
}

/// What each menu bar item shows after its name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarValue {
    /// `79% (4d)`
    #[default]
    Both,
    /// `79%`
    Percent,
    /// `4d`: time until it refills
    Time,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarNames {
    #[default]
    Full,
    /// Two letters, unless the label is already four characters or fewer.
    Short,
}

/// What the menu bar app shows at a glance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The pinned account, unless something is running out or going unused.
    #[default]
    Watch,
    /// Whatever limit has the least left.
    Lowest,
    /// The biggest allowance about to go unused, else the pinned account.
    UseIt,
    /// Icon only.
    Minimal,
    /// A short number for every account.
    Everything,
}

impl Mode {
    pub const ALL: [Mode; 5] = [Mode::Watch, Mode::Lowest, Mode::UseIt, Mode::Minimal, Mode::Everything];

    pub fn key(self) -> &'static str {
        match self {
            Mode::Watch => "watch",
            Mode::Lowest => "lowest",
            Mode::UseIt => "use_it",
            Mode::Minimal => "minimal",
            Mode::Everything => "everything",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Mode::Watch => "Watch: pinned account, alerts take over",
            Mode::Lowest => "Lowest: whatever is closest to running out",
            Mode::UseIt => "Use it: allowance about to go unused",
            Mode::Minimal => "Minimal: icon only",
            Mode::Everything => "Everything: every account",
        }
    }
}

/// A duration written as `"90s"`, `"10m"` or `"1h"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Interval(pub Duration);

/// Checking more often than this risks being rate limited for no benefit.
pub const MIN_INTERVAL: Duration = Duration::from_secs(60);

impl TryFrom<String> for Interval {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        let s = s.trim();
        let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        let n: u64 = s[..split].parse().map_err(|_| format!("bad interval `{s}`, expected e.g. \"10m\""))?;
        let secs = match &s[split..] {
            "s" => n,
            "m" => n * 60,
            "h" => n * 3600,
            _ => return Err(format!("bad interval `{s}`, use s, m or h, e.g. \"10m\"")),
        };
        let d = Duration::from_secs(secs);
        if d < MIN_INTERVAL {
            return Err(format!("interval `{s}` is below the 1m minimum"));
        }
        Ok(Interval(d))
    }
}

impl From<Interval> for String {
    fn from(i: Interval) -> String {
        let s = i.0.as_secs();
        match s {
            _ if s.is_multiple_of(3600) => format!("{}h", s / 3600),
            _ if s.is_multiple_of(60) => format!("{}m", s / 60),
            _ => format!("{s}s"),
        }
    }
}

fn yes() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hosts: vec![],
            auto_detect: true,
            share_email: true,
            show: Show::default(),
            mode: Mode::default(),
            bar_value: BarValue::default(),
            bar_names: BarNames::default(),
            pins: None,
            pin: None,
            intervals: BTreeMap::new(),
            sources: vec![],
            labels: BTreeMap::new(),
            hidden: vec![],
            disable: vec![],
        }
    }
}

/// One set of credentials on this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Tool config directory: `CODEX_HOME` for codex, `CLAUDE_CONFIG_DIR` for claude.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// API key location, for API-key providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<KeyRef>,
    /// claude only: Keychain service holding this config dir's login, if not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keychain_service: Option<String>,

    // http and command sources (see providers::custom)
    /// Display name, e.g. "Vercel AI Gateway".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// JSON body; sends a POST instead of a GET.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<crate::providers::custom::Auth>,
    /// Shell command whose stdout is JSON. Hand-written config only, never imported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Pointer to the plan name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub window: Vec<crate::providers::custom::WindowMap>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub balance: Vec<crate::providers::custom::BalanceMap>,
}

impl SourceConfig {
    /// An entry for `provider` with nothing else set.
    pub fn new(provider: &str) -> Self {
        Self {
            provider: provider.into(),
            label: None,
            home: None,
            key: None,
            keychain_service: None,
            name: None,
            url: None,
            post: None,
            auth: None,
            command: None,
            plan: None,
            window: vec![],
            balance: vec![],
        }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;
    Some(base.join("juicemeter"))
}

pub fn config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("JUICEMETER_CONFIG") {
        return Some(PathBuf::from(p));
    }
    Some(config_dir()?.join("config.toml"))
}

impl Config {
    /// Load the config file; a missing file means defaults.
    pub fn load() -> anyhow::Result<Self> {
        let Some(path) = config_path() else { return Ok(Self::default()) };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("invalid config {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Pinned accounts, accepting the older `pin = "…"` form.
    pub fn pinned(&self) -> Option<Vec<String>> {
        self.pins.clone().or_else(|| self.pin.clone().map(|p| vec![p]))
    }

    /// How often to check a provider: the configured interval, else the provider's default.
    pub fn interval(&self, p: &dyn Provider) -> Duration {
        self.intervals.get(p.kind()).map(|i| i.0).unwrap_or_else(|| p.poll_interval())
    }

    /// Providers for this machine: the default source of each provider (if `auto_detect`),
    /// plus configured sources. A configured source replaces a default with the same location.
    pub fn providers(&self) -> anyhow::Result<Vec<Box<dyn Provider>>> {
        let mut out: Vec<Box<dyn Provider>> = if self.auto_detect { crate::providers::defaults() } else { Vec::new() };
        for s in &self.sources {
            let p = build(s)?;
            match out.iter().position(|o| o.kind() == p.kind() && o.source() == p.source()) {
                Some(i) => out[i] = p,
                None => out.push(p),
            }
        }
        out.retain(|p| !self.disable.iter().any(|d| d == p.kind()));
        Ok(out)
    }

    /// Whether this report is hidden here: its account is, or it's a "not configured" hint
    /// for a provider whose accounts are hidden.
    pub fn is_hidden(&self, r: &crate::ProviderReport) -> bool {
        match &r.account {
            Some(a) => self.hidden.contains(&a.id),
            None => {
                matches!(r.outcome, crate::Outcome::NotConfigured { .. })
                    && self
                        .hidden
                        .iter()
                        .any(|h| h.strip_prefix(r.provider.as_str()).is_some_and(|rest| rest.starts_with(':')))
            }
        }
    }
}

/// Construct a provider from a source entry.
pub fn build(s: &SourceConfig) -> anyhow::Result<Box<dyn Provider>> {
    let home = s.home.as_deref().map(crate::secrets::expand);
    let needs = |what: &str| anyhow::anyhow!("{} source needs `{what}`", s.provider);
    Ok(match s.provider.as_str() {
        #[cfg(feature = "claude")]
        "claude" => {
            if s.key.is_some() {
                bail!("claude sources take `home`, not `key`");
            }
            Box::new(crate::providers::claude::Claude::new(home, s.keychain_service.clone(), s.label.clone()))
        }
        #[cfg(feature = "codex")]
        "codex" => match home {
            Some(h) => Box::new(crate::providers::codex::Codex::new(h, s.label.clone())),
            None => return Err(needs("home")),
        },
        #[cfg(feature = "deepseek")]
        "deepseek" => match &s.key {
            Some(k) => Box::new(crate::providers::deepseek::DeepSeek::new(Some(k.clone()), s.label.clone())),
            None => return Err(needs("key")),
        },
        #[cfg(feature = "antigravity")]
        "antigravity" => Box::new(crate::providers::antigravity::Antigravity { label: s.label.clone() }),
        "http" | "command" => {
            use crate::providers::custom::{Command, Http, Mapping};
            let name = s.name.clone().ok_or_else(|| needs("name"))?;
            let map = Mapping { plan: s.plan.clone(), windows: s.window.clone(), balances: s.balance.clone() };
            if s.provider == "http" {
                Box::new(Http {
                    name,
                    url: s.url.clone().ok_or_else(|| needs("url"))?,
                    post_body: s.post.clone(),
                    key: s.key.clone(),
                    auth: s.auth.clone().unwrap_or_default(),
                    label: s.label.clone(),
                    map,
                })
            } else {
                Box::new(Command {
                    name,
                    command: s.command.clone().ok_or_else(|| needs("command"))?,
                    label: s.label.clone(),
                    map,
                })
            }
        }
        other => bail!("unknown provider `{other}` (supported: {})", crate::providers::kinds().join(", ")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let c: Config = toml::from_str(
            r#"
            hosts = ["mini"]
            share_email = false
            show = "used"
            pins = []
            [[source]]
            provider = "codex"
            home = "/tmp/codex-work"
            label = "Work"
            [[source]]
            provider = "deepseek"
            key = { env = "DS2" }
            [intervals]
            claude = "10m"
            [labels]
            "codex:abc" = "Personal"
            "#,
        )
        .unwrap();
        assert!(c.auto_detect && !c.share_email);
        assert_eq!(c.sources.len(), 2);
        assert_eq!(c.labels["codex:abc"], "Personal");
        assert_eq!(c.show, Show::Used);
        assert_eq!(c.pinned(), Some(vec![]));
        let legacy: Config = toml::from_str("pin = \"claude:a\"").unwrap();
        assert_eq!(legacy.pinned(), Some(vec!["claude:a".to_string()]));
        assert_eq!(c.intervals["claude"].0, Duration::from_secs(600));
    }

    #[test]
    fn intervals_parse_and_enforce_minimum() {
        let parse = |s: &str| Interval::try_from(s.to_string());
        assert_eq!(parse("90s").unwrap().0, Duration::from_secs(90));
        assert_eq!(parse("1h").unwrap().0, Duration::from_secs(3600));
        assert!(parse("30s").is_err());
        assert!(parse("10").is_err());
        assert!(parse("m").is_err());
        assert_eq!(String::from(parse("120m").unwrap()), "2h");
    }

    #[test]
    fn rejects_unknown_fields_and_providers() {
        assert!(toml::from_str::<Config>("[[source]]\nprovider = \"codex\"\nhom = \"x\"").is_err());
        let s = SourceConfig { provider: "nope".into(), ..SourceConfig::new("nope") };
        assert!(build(&s).is_err());
    }

    #[test]
    fn disabled_providers_are_not_checked() {
        let c: Config = toml::from_str("disable = [\"deepseek\"]").unwrap();
        let kinds: Vec<String> = c.providers().unwrap().iter().map(|p| p.kind().to_string()).collect();
        assert!(!kinds.contains(&"deepseek".to_string()) && kinds.contains(&"claude".to_string()), "{kinds:?}");
    }

    #[test]
    fn parses_http_and_command_sources() {
        let c: Config = toml::from_str(
            r#"
            [[source]]
            provider = "http"
            name = "Gateway"
            url = "https://example.com/credits"
            key = { env = "K" }
            auth = { header = "X-API-Key" }
            balance = [{ label = "Credits", amount = "/balance", currency = "USD" }]

            [[source]]
            provider = "command"
            name = "Pi"
            command = "omp usage --json"
            window = [{ label = "Weekly", used = "/w/pct", length = "7d" }]
            "#,
        )
        .unwrap();
        let ps: Vec<_> = c.sources.iter().map(|s| build(s).unwrap()).collect();
        assert_eq!((ps[0].kind(), ps[0].name()), ("http", "Gateway"));
        assert_eq!((ps[1].kind(), ps[1].source()), ("command", "`omp usage --json`".to_string()));
        assert!(build(&SourceConfig::new("http")).is_err(), "http needs a name and url");
    }

    #[test]
    fn configured_source_replaces_default_at_same_location() {
        let home = crate::providers::codex::default_home().unwrap();
        let c = Config {
            sources: vec![SourceConfig {
                label: Some("Mine".into()),
                home: Some(home.display().to_string()),
                ..SourceConfig::new("codex")
            }],
            ..Default::default()
        };
        let ps = c.providers().unwrap();
        assert_eq!(ps.iter().filter(|p| p.kind() == "codex").count(), 1);
    }
}
