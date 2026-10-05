mod service;
mod state;

use anyhow::Context;
use axum::{extract::State, http::StatusCode, routing::get, routing::post, Json, Router};
use clap::{Parser, Subcommand};
use state::Shared;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "juicemeter-agent", version, about = "Serve this machine's AI usage report over Tailscale")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run in the foreground (default)
    Run(RunArgs),
    /// Install as a login service (launchd on macOS, systemd --user on Linux)
    Install,
    /// Remove the login service
    Uninstall,
}

#[derive(clap::Args, Default)]
struct RunArgs {
    #[arg(long, default_value_t = juicemeter_core::DEFAULT_PORT)]
    port: u16,
    /// Address to listen on; repeatable. Default: 127.0.0.1 plus this machine's Tailscale IPv4.
    #[arg(long)]
    listen: Vec<IpAddr>,
    /// Where the last report is persisted
    #[arg(long)]
    cache: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        None => run(RunArgs { port: juicemeter_core::DEFAULT_PORT, ..Default::default() }).await,
        Some(Command::Run(args)) => run(args).await,
        Some(Command::Install) => service::install(),
        Some(Command::Uninstall) => service::uninstall(),
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { eprintln!("{} {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), format_args!($($t)*)) };
}

async fn run(args: RunArgs) -> anyhow::Result<()> {
    let cache = match args.cache {
        Some(p) => p,
        None => dirs::cache_dir().context("no cache dir")?.join("juicemeter").join("report.json"),
    };
    let config = juicemeter_core::Config::load()?;
    let shared = Shared::new(config.providers()?, cache, config);
    shared.spawn_pollers();

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/report", get(report))
        .route("/v1/refresh", post(refresh))
        .with_state(shared);

    if args.listen.is_empty() {
        serve(app.clone(), SocketAddr::new(Ipv4Addr::LOCALHOST.into(), args.port)).await?;
        // Tailscale may come up after us (e.g. at boot), so keep looking for its address.
        tokio::spawn(async move {
            loop {
                if let Some(ip) = tailscale_ipv4() {
                    match serve(app.clone(), SocketAddr::new(ip.into(), args.port)).await {
                        Ok(()) => return,
                        Err(e) => log!("{e:#}"),
                    }
                }
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        });
    } else {
        for ip in &args.listen {
            serve(app.clone(), SocketAddr::new(*ip, args.port)).await?;
        }
    }

    tokio::signal::ctrl_c().await?;
    log!("shutting down");
    Ok(())
}

/// Bind now, serve in the background.
async fn serve(app: Router, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("bind {addr}"))?;
    log!("listening on http://{addr}");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            log!("server on {addr} stopped: {e}");
        }
    });
    Ok(())
}

/// Tailscale assigns IPv4 addresses from the CGNAT range 100.64.0.0/10.
fn tailscale_ipv4() -> Option<Ipv4Addr> {
    if_addrs::get_if_addrs().ok()?.into_iter().find_map(|i| match i.ip() {
        IpAddr::V4(v4) if v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64 => Some(v4),
        _ => None,
    })
}

async fn report(State(s): State<Shared>) -> Json<juicemeter_core::Report> {
    Json(s.report().await)
}

async fn refresh(State(s): State<Shared>) -> Result<Json<juicemeter_core::Report>, (StatusCode, String)> {
    s.refresh_all().await.map_err(|wait| {
        (StatusCode::TOO_MANY_REQUESTS, format!("refreshed recently, try again in {}s", wait.as_secs()))
    })?;
    Ok(Json(s.report().await))
}
