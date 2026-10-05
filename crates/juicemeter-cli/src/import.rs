//! Onboarding credentials that other tools (coding agents and the like) manage: `setup-prompt` gives the
//! user a prompt for that tool's agent, `import` reads the agent's answer. The answer says
//! where keys live, never the keys themselves.

use anyhow::{bail, Context};
use juicemeter_core::config::{build, SourceConfig};
use juicemeter_core::secrets::KeyRef;
use juicemeter_core::{Config, Outcome};
use serde::Deserialize;
use std::io::{IsTerminal, Read};

const FORMAT_VERSION: u32 = 1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportFile {
    juicemeter_import: u32,
    sources: Vec<SourceConfig>,
    #[serde(default)]
    unsupported: Vec<String>,
}

pub fn prompt() -> String {
    let kinds = juicemeter_core::providers::kinds().join(", ");
    format!(
        r#"I use juicemeter to track my AI subscription limits and API balances. List the AI
provider credentials you have configured on this machine so juicemeter can read them itself.

Never include secret values (API keys, tokens, passwords). Only say where each one is stored.

Supported providers: {kinds}. For credentials of any other provider, put just the provider
name in "unsupported".

Reply with only this JSON:

{{
  "juicemeter_import": {FORMAT_VERSION},
  "sources": [],
  "unsupported": []
}}

Each entry in "sources" is one of:
  {{"provider": "deepseek", "label": "<short name>", "key": <key location>}}
  {{"provider": "codex", "label": "<short name>", "home": "<CODEX_HOME directory>"}}
  {{"provider": "claude", "label": "<short name>", "home": "<CLAUDE_CONFIG_DIR directory>"}}

A key location is one of:
  {{"env_file": "/absolute/path/.env", "var": "DEEPSEEK_API_KEY"}}
  {{"json_file": "/absolute/path/config.json", "pointer": "/json/pointer/to/the/key"}}
  {{"keychain": "<macOS Keychain service>", "account": "<account>"}}
  {{"env": "VAR_NAME"}}  (only if it is set in my login shell, not just in your process)

Use absolute paths, and labels that tell entries apart, e.g. "Agent DeepSeek".
"#
    )
}

pub async fn run(file: &str, yes: bool, config: &Config) -> anyhow::Result<()> {
    let text = if file == "-" {
        if std::io::stdin().is_terminal() {
            eprintln!("Paste the JSON answer, then press Ctrl-D:");
        }
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        std::fs::read_to_string(file).with_context(|| format!("reading {file}"))?
    };
    // Agents like to wrap JSON in prose or code fences.
    let json = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if a < b => &text[a..=b],
        _ => bail!("no JSON object found in the input"),
    };
    let input: ImportFile = serde_json::from_str(json).context("the answer doesn't match the setup-prompt format")?;
    if input.juicemeter_import != FORMAT_VERSION {
        bail!("format version {} not supported (expected {FORMAT_VERSION})", input.juicemeter_import);
    }

    let configured: Vec<(String, String)> =
        config.sources.iter().filter_map(|s| build(s).ok()).map(|p| (p.kind().to_string(), p.source())).collect();
    let detected: Vec<(String, String)> =
        juicemeter_core::providers::defaults().iter().map(|p| (p.kind().to_string(), p.source())).collect();

    let http = juicemeter_core::http_client();
    let mut accepted: Vec<SourceConfig> = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    for s in input.sources {
        let tag = format!("{} {}", s.provider, s.label.as_deref().unwrap_or(""));
        if s.provider == "command" || s.post.is_some() {
            println!("✗ {tag}: command and POST sources can't be imported; add them to the config by hand");
            continue;
        }
        if matches!(s.key, Some(KeyRef::Command { .. })) {
            println!("✗ {tag}: command-based keys can't be imported; add them to the config by hand");
            continue;
        }
        let p = match build(&s) {
            Ok(p) => p,
            Err(e) => {
                println!("✗ {tag}: {e:#}");
                continue;
            }
        };
        let id = (p.kind().to_string(), p.source());
        if configured.contains(&id) || seen.contains(&id) {
            println!("· {} {}: already configured", p.name(), id.1);
            continue;
        }
        let report = juicemeter_core::fetch_one(p.as_ref(), &http).await;
        let who = report.account.as_ref().map(|a| format!("{} · ", a.display())).unwrap_or_default();
        let replaces = if detected.contains(&id) { " (labels the auto-detected source)" } else { "" };
        match &report.outcome {
            Outcome::Ok { snapshot } => {
                let what = snapshot
                    .balances
                    .first()
                    .map(|b| b.display())
                    .or_else(|| snapshot.windows.first().map(|w| format!("{} {:.0}%", w.label, w.used_percent)))
                    .unwrap_or_default();
                println!("✓ {} · {who}{}  {what}{replaces}", p.name(), id.1);
                seen.push(id);
                accepted.push(s);
            }
            Outcome::NotConfigured { hint } => println!("✗ {} · {}: {hint}", p.name(), id.1),
            Outcome::Error { message } => println!("✗ {} · {who}{}: {message}", p.name(), id.1),
        }
    }
    if !input.unsupported.is_empty() {
        println!("not supported yet: {}", input.unsupported.join(", "));
    }
    if accepted.is_empty() {
        println!("nothing to add");
        return Ok(());
    }

    let path = juicemeter_core::config::config_path().context("no config directory")?;
    if !yes && !crate::ask::confirm(&format!("Add {} source(s) to {}?", accepted.len(), path.display()), false)? {
        println!("nothing changed");
        return Ok(());
    }
    juicemeter_core::edit::add_sources(&accepted)?;
    println!("added. If juicemeter-agent runs here, restart it with `juicemeter-agent install`.");
    Ok(())
}
