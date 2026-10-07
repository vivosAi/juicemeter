mod ask;
mod import;
mod render;
mod setup;

use anyhow::bail;
use clap::{Parser, Subcommand};
use juicemeter_core::config::Show;
use juicemeter_core::fetch::Machines;
use juicemeter_core::{Config, Report};
use std::io::IsTerminal;

#[derive(Parser)]
#[command(name = "juicemeter", version, about = "AI usage meter: plan limits and API balances")]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// `juicemeter --local` etc. work without typing `status`
    #[command(flatten)]
    status: StatusArgs,
}

/// Which machines to read. Default: this machine plus the `hosts` in the config.
#[derive(clap::Args, Default)]
struct Hosts {
    /// Only this machine
    #[arg(long, conflicts_with = "host")]
    local: bool,
    /// Only these juicemeter-agents (`mini`, `host:port`); repeatable
    #[arg(long)]
    host: Vec<String>,
    /// Query this machine's providers directly even if its agent has a cached report
    #[arg(long)]
    fresh: bool,
}

#[derive(clap::Args, Default)]
struct StatusArgs {
    #[command(flatten)]
    hosts: Hosts,
    /// One section per machine instead of merging accounts
    #[arg(long)]
    by_host: bool,
    /// Print JSON: merged entries, or the raw reports with --by-host
    #[arg(long)]
    json: bool,
    /// Comma-separated provider kinds to include, e.g. `claude,codex`
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Show how much of each limit is used (overrides `show` in the config)
    #[arg(long, conflicts_with = "remaining")]
    used: bool,
    /// Show how much of each limit is left (overrides `show` in the config)
    #[arg(long)]
    remaining: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Show usage across your machines, one entry per account (default)
    Status(StatusArgs),
    /// List every account across your machines, once each
    Accounts {
        #[command(flatten)]
        hosts: Hosts,
    },
    /// Name an account, e.g. `juicemeter label codex:3f9a1c2b4d5e "Work ChatGPT"`
    Label {
        account_id: String,
        label: Option<String>,
        /// Remove the label
        #[arg(long, conflicts_with = "label")]
        clear: bool,
    },
    /// Print a prompt for another AI agent to describe credentials it holds
    SetupPrompt,
    /// Add sources from an agent's answer to `setup-prompt` (file, or `-` for stdin)
    Import {
        #[arg(default_value = "-")]
        file: String,
        /// Don't ask for confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Check this machine and your tailnet: agent, firewall, other machines to read
    Setup {
        /// Accept the suggested answers (never runs sudo)
        #[arg(long)]
        yes: bool,
        /// Only report; change nothing
        #[arg(long, conflicts_with = "yes")]
        check: bool,
    },
    /// Print the config and env file locations
    Paths,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Status(cli.status)) {
        Command::Status(StatusArgs { hosts, by_host, json, only, used, remaining }) => {
            let show = match (used, remaining) {
                (true, _) => Show::Used,
                (_, true) => Show::Remaining,
                _ => config.show,
            };
            let mut reports = gather(&config, &hosts).await?;
            for r in &mut reports {
                r.providers.retain(|p| (only.is_empty() || only.contains(&p.provider)) && !config.is_hidden(p));
            }
            let color = std::io::stdout().is_terminal();
            match (by_host, json) {
                (true, true) => println!("{}", serde_json::to_string_pretty(&reports)?),
                (true, false) => render::by_host(&reports, color, show),
                (false, true) => {
                    println!("{}", serde_json::to_string_pretty(&juicemeter_core::merge::merge(&reports))?)
                }
                (false, false) => {
                    render::merged(&juicemeter_core::merge::merge(&reports), reports.len() > 1, color, show)
                }
            }
        }
        Command::Accounts { hosts } => {
            let reports = gather(&config, &hosts).await?;
            render::accounts(&reports, std::io::stdout().is_terminal());
        }
        Command::Label { account_id, label, clear } => {
            let Some((kind, hash)) = account_id.split_once(':') else {
                bail!("account ids look like `codex:3f9a1c2b4d5e` — see `juicemeter accounts`")
            };
            if !juicemeter_core::providers::kinds().contains(&kind) || hash.is_empty() {
                bail!("unknown account id `{account_id}` — see `juicemeter accounts`");
            }
            match (label, clear) {
                (Some(l), _) => {
                    juicemeter_core::edit::set_label(&account_id, Some(&l))?;
                    println!("{account_id} is now \"{l}\"");
                }
                (None, true) => {
                    juicemeter_core::edit::set_label(&account_id, None)?;
                    println!("removed label from {account_id}");
                }
                (None, false) => bail!("give a label, or --clear to remove it"),
            }
        }
        Command::SetupPrompt => print!("{}", import::prompt()),
        Command::Import { file, yes } => import::run(&file, yes, &config).await?,
        Command::Setup { yes, check } => setup::run(&config, yes, check).await?,
        Command::Paths => {
            let show = |p: Option<std::path::PathBuf>| p.map(|p| p.display().to_string()).unwrap_or_else(|| "?".into());
            println!("config    {}", show(juicemeter_core::config::config_path()));
            println!("env file  {}", show(juicemeter_core::secrets::env_file_path()));
        }
    }
    Ok(())
}

/// Collect reports per [`Hosts`], warning about machines that can't be reached.
async fn gather(config: &Config, hosts: &Hosts) -> anyhow::Result<Vec<Report>> {
    let machines = match (hosts.local, hosts.host.is_empty()) {
        (true, _) => Machines::Local,
        (false, true) => Machines::All,
        (false, false) => Machines::Hosts(hosts.host.clone()),
    };
    let g = juicemeter_core::fetch::gather(config, &machines, hosts.fresh).await?;
    for (h, e) in g.unreachable {
        eprintln!("warning: could not reach juicemeter-agent on {h}: {e}");
    }
    Ok(g.reports)
}
