//! Starting the collector in this process.
//!
//! The packaging requirement this exists for: unzip, double-click one file,
//! see your usage. Telling someone to also start a second process — and to keep
//! it running, and to restart it after a reboot — is where a widget stops being
//! something people use.
//!
//! It is the *same* daemon, though, not a stripped-down copy. `uwd` is a
//! library that this links; the standalone binary is a `main` in front of the
//! same code. So a login started from the panel and a login started from `uw`
//! do the same thing, and the widget still works pointed at a daemon somewhere
//! else — which is the arrangement this project was built for, with the
//! credentials inside WSL and the UI on Windows.
//!
//! Desktop only. On Android and iOS there is no keychain integration and no
//! loopback redirect to catch an OAuth callback on, so the phone builds stay
//! viewers of a daemon elsewhere.

use std::time::Duration;

use tauri::Manager;
use uw_core::Config;

/// Where the webview should point itself.
pub struct DaemonUrl(pub String);

/// Held so the daemon lives as long as the app does.
///
/// Never read: dropping it would not stop the collector either, since the
/// tasks are detached onto the runtime. Keeping the handle in managed state is
/// what makes the lifetime explicit rather than accidental, and it is where a
/// clean shutdown hook would attach if one is ever wanted.
#[allow(dead_code)]
pub struct Embedded(pub Option<uwd::Running>);

/// How long to wait for an already-running daemon to answer `/health`.
///
/// Short: it is loopback, and the only thing at stake is whether we bind an
/// ephemeral port instead. A slow answer here would delay the first paint.
const PROBE_TIMEOUT: Duration = Duration::from_millis(750);

/// Find a daemon, starting one if there is not already one to use.
///
/// Three outcomes, in order of preference:
///
/// 1. **We bind the configured port.** The normal case, and the one that makes
///    the app self-contained.
/// 2. **Something is already answering there.** A `uwd` the user started, or a
///    second copy of this app. Use it rather than fighting over the config file
///    and polling every provider twice.
/// 3. **The port is taken by something that is not a daemon.** Bind an
///    ephemeral port instead. The webview is told the address, so an unlucky
///    port collision costs nothing the user can see.
pub async fn ensure_running(app: &tauri::AppHandle) {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            // A corrupt config file must not stop the app from starting — the
            // panel is the only place the user can fix it from.
            tracing::error!("could not read the config: {e:#}");
            Config::fresh()
        }
    };
    let configured = cfg.daemon.bind.clone();

    let (url, running) = match uwd::start(cfg.clone(), None).await {
        Ok(r) => (format!("http://{}", r.addr), Some(r)),
        Err(e) => {
            tracing::info!("could not bind {configured}: {e:#}");
            match probe(&configured).await {
                Some(url) => {
                    tracing::info!("using the daemon already running at {url}");
                    (url, None)
                }
                None => match uwd::start(cfg, Some("127.0.0.1:0".into())).await {
                    Ok(r) => {
                        tracing::info!("bound an ephemeral port instead");
                        (format!("http://{}", r.addr), Some(r))
                    }
                    Err(e) => {
                        // The panel still loads; it shows "cannot reach uwd"
                        // and offers the settings screen, which is the only
                        // useful thing left to do.
                        tracing::error!("could not start a daemon at all: {e:#}");
                        (format!("http://{configured}"), None)
                    }
                },
            }
        }
    };

    app.manage(DaemonUrl(url));
    app.manage(Embedded(running));
}

/// Is a usage-watcher daemon answering on this address?
///
/// `/health` is unauthenticated precisely so this works: a token-protected
/// daemon must still be recognisable as one, or we would start a second.
async fn probe(bind: &str) -> Option<String> {
    let url = format!("http://{bind}");
    let client = uw_core::reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .ok()?;
    let res = client.get(format!("{url}/health")).send().await.ok()?;
    res.status().is_success().then_some(url)
}

/// Where the webview should send its requests.
#[tauri::command]
pub fn daemon_url(url: tauri::State<'_, DaemonUrl>) -> String {
    url.0.clone()
}
