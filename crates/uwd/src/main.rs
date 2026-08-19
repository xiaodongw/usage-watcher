//! `uwd` — the collector daemon.
//!
//! It owns every credential and every poll; the widget, the CLI and any future
//! phone view are read-only viewers over `/snapshot` and `/events`. That split
//! is what makes a desktop widget possible under WSL2, where the credentials
//! and vendor CLIs live inside Linux but the UI wants to render on Windows.

mod http;
mod hub;
mod poll;

use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::watch;

use uw_core::Config;

use crate::hub::Hub;
use crate::poll::Schedule;

#[derive(Parser)]
#[command(name = "uwd", about = "Collect AI subscription usage and serve it to viewers")]
struct Cli {
    /// Override `[daemon] bind` from the config file.
    #[arg(long)]
    bind: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("UWD_LOG")
                .unwrap_or_else(|_| "uwd=info,uw_core=warn".into()),
        )
        .init();

    let cli = Cli::parse();
    let mut cfg = Config::load()?;
    if let Some(bind) = cli.bind {
        cfg.daemon.bind = bind;
    }

    // Checked before anything else starts: refusing to listen is only useful
    // if it happens before we have fetched anyone's usage.
    let addr = cfg.daemon.check()?;

    let pollers = uw_core::collect::pollers(&cfg);
    if pollers.is_empty() {
        anyhow::bail!(
            "no providers are enabled — check `providers.*.enabled` in {}",
            Config::path()?.display()
        );
    }

    let hub = Arc::new(Hub::new(cfg.daemon.history));
    let httpc = uw_core::http_client();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut tasks = Vec::new();
    for poller in pollers {
        let intervals = uw_core::providers::Any::by_id(poller.id())
            .map(|a| a.poll_intervals())
            .unwrap_or((60, 300));
        let (active, idle) = cfg.intervals(poller.id(), intervals);
        tracing::info!(
            provider = poller.id(),
            "polling every {active}s active / {idle}s idle"
        );
        tasks.push(tokio::spawn(poll::run(
            poller,
            hub.clone(),
            httpc.clone(),
            Schedule::from_secs((active, idle)),
            shutdown_rx.clone(),
        )));
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("could not bind {addr}"))?;
    tracing::info!("listening on http://{addr}");
    if cfg.daemon.token.is_some() {
        tracing::info!("bearer token required");
    }

    let app = http::router(http::AppState {
        hub: hub.clone(),
        token: cfg.daemon.token.clone(),
    });

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;

    // Stop the pollers and let them finish the sleep they are sitting in,
    // rather than leaving tasks running against a closed hub.
    let _ = shutdown_tx.send(true);
    for t in tasks {
        let _ = t.await;
    }
    Ok(())
}
