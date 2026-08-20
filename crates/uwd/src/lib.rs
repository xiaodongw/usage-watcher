//! `uwd` — the collector daemon, as a library.
//!
//! It owns every credential and every poll; the widget, the CLI and any phone
//! view are viewers over `/snapshot` and `/events`. That split is what makes a
//! desktop widget possible under WSL2, where the credentials and vendor CLIs
//! live inside Linux but the UI wants to render on Windows.
//!
//! A library rather than only a binary because the desktop app embeds it. A
//! user who downloads a zip and double-clicks one file should not then have to
//! be told about a second process — so the Tauri shell starts this in-process
//! and talks to it over loopback, exactly as an external `uwd` would be talked
//! to. One code path, whichever way it was launched.

pub mod api;
pub mod http;
pub mod hub;
pub mod login;
pub mod poll;
pub mod supervisor;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{oneshot, RwLock};
use tokio::task::JoinHandle;
use uw_core::Config;

use crate::hub::Hub;
use crate::login::Logins;
use crate::supervisor::Supervisor;

/// Everything a request handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<Hub>,
    pub http: uw_core::reqwest::Client,
    pub supervisor: Arc<Supervisor>,
    pub logins: Arc<Logins>,
    /// The live config, and the only writer of the config file.
    ///
    /// Held here rather than re-read per request so that two clicks arriving
    /// together cannot each load, modify and save — which loses one of them.
    pub config: Arc<RwLock<Config>>,
    /// When set, every route except `/health` requires it as a bearer token.
    ///
    /// Snapshotted at startup on purpose. Changing the token or the bind
    /// address means restarting the listener, so honouring an edit here while
    /// the old socket stayed open would only be half a change.
    pub token: Option<String>,
}

/// A daemon that is listening.
pub struct Running {
    pub addr: SocketAddr,
    pub state: AppState,
    shutdown: oneshot::Sender<()>,
    server: JoinHandle<()>,
}

impl Running {
    /// Serve until the process is asked to stop, then wind down cleanly.
    pub async fn run_until_ctrl_c(self) -> Result<()> {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
        self.stop().await
    }

    /// Stop serving, stop polling, and abandon any login in flight.
    pub async fn stop(self) -> Result<()> {
        let _ = self.shutdown.send(());
        let _ = self.server.await;
        self.state.logins.abort_all().await;
        self.state.supervisor.shutdown().await;
        Ok(())
    }
}

/// Bind, start polling, and serve. Returns as soon as the socket is listening.
///
/// `bind_override` beats `cfg.daemon.bind`, which is how the embedded daemon
/// asks for an ephemeral port when the usual one is taken.
pub async fn start(cfg: Config, bind_override: Option<String>) -> Result<Running> {
    // The override is applied to a copy, never written back into `cfg`. The
    // provider API saves the config whenever something is added or removed, so
    // folding a `--bind` flag or an ephemeral fallback port into the loaded
    // config would quietly make that one run's address permanent — and the next
    // plain `uwd` would come up somewhere nobody asked for.
    let daemon = match bind_override {
        Some(bind) => uw_core::config::DaemonConfig {
            bind,
            ..cfg.daemon.clone()
        },
        None => cfg.daemon.clone(),
    };
    // Checked before anything else starts: refusing to listen is only useful if
    // it happens before we have fetched anyone's usage.
    let addr = daemon.check()?;

    let hub = Arc::new(Hub::new(daemon.history));
    let httpc = uw_core::http_client();
    let supervisor = Arc::new(Supervisor::new(hub.clone(), httpc.clone()));

    let state = AppState {
        hub: hub.clone(),
        http: httpc,
        supervisor: supervisor.clone(),
        logins: Arc::new(Logins::default()),
        token: daemon.token.clone(),
        config: Arc::new(RwLock::new(cfg)),
    };

    // Bind before polling anything. The embedded daemon in the desktop app
    // calls this speculatively — if the usual port is taken it expects an
    // `Err` and tries something else — and pollers started before a failed
    // bind would carry on running, unreachable, polling every provider twice
    // once the retry succeeded.
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("could not bind {addr}"))?;
    // The bound address, not the requested one: port 0 means "any free port",
    // and the caller has no other way to learn which one it got.
    let addr = listener.local_addr()?;
    tracing::info!("listening on http://{addr}");

    {
        let cfg = state.config.read().await.clone();
        supervisor.sync(&cfg).await;
    }
    // Not an error. An empty provider list is what a fresh install looks like,
    // and the panel's job in that state is to offer an "Add provider" button —
    // which it cannot do if the daemon refused to start.
    match supervisor.running().await {
        0 => tracing::info!("no providers configured yet"),
        n => tracing::info!("{n} provider(s) polling"),
    }

    if state.token.is_some() {
        tracing::info!("bearer token required");
    }

    let app = http::router(state.clone());
    let (shutdown, rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
        if let Err(e) = served {
            tracing::error!("server stopped: {e}");
        }
    });

    Ok(Running {
        addr,
        state,
        shutdown,
        server,
    })
}
