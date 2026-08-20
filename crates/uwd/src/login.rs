//! Browser logins driven by a viewer that is not this process.
//!
//! `uw auth login` can block on stdin because a terminal has nothing else to
//! do. The daemon cannot: it is serving other viewers, and the human is over in
//! a webview — possibly on a different machine, since the whole point of the
//! daemon/viewer split is that credentials stay where the vendor CLIs are.
//!
//! So a login becomes a small state machine the UI can watch:
//!
//! ```text
//!   POST /providers/{id}/login  ──►  Opening
//!                                      │  the OAuth client hands us the URL
//!                                      ▼
//!                                    Waiting { authorize_url, needs_code }
//!                                      │  browser redirects to our loopback
//!                                      │  ...or the user pastes a code
//!                                      ▼
//!                                    Done | Failed
//! ```
//!
//! `needs_code` is the one branch the UI has to care about, and it is not a
//! preference: Claude registers no loopback redirect at all, so its consent
//! page shows a code and there is nothing to listen for. Every other provider
//! redirects and the paste box would only be a box to get wrong.

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use std::sync::Arc;

use anyhow::{bail, Result};
use serde::Serialize;
use tokio::sync::{oneshot, watch, Mutex};
use tokio::task::JoinHandle;
use ts_rs::TS;
use uw_core::auth::LoginUi;

/// How long a started login stays open before it is abandoned.
///
/// Matches the OAuth client's own wait. Sessions are also replaced whenever a
/// new login starts for the same provider, which is what actually reclaims
/// them in practice — this is the backstop for a user who walks away.
pub const SESSION_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// Where a login has got to. Polled by `GET`, and pushed over SSE.
#[derive(TS, Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
#[ts(export, export_to = "../../../widget/src/types/")]
pub enum Phase {
    /// Started, but the authorize URL is not built yet. Sub-second.
    Opening,
    Waiting {
        authorize_url: String,
        /// The provider will show a code rather than redirect; the UI must
        /// offer a field and post it back.
        needs_code: bool,
    },
    Done,
    Failed {
        message: String,
    },
}

impl Phase {
    pub fn is_final(&self) -> bool {
        matches!(self, Phase::Done | Phase::Failed { .. })
    }
}

/// One login in flight.
pub struct Session {
    /// Random, and required on the code submission. Without it a stale tab
    /// could post the code from an abandoned login into a fresh one — which
    /// would fail the PKCE check in a way nobody could diagnose.
    pub id: String,
    pub provider: String,
    phase: watch::Sender<Phase>,
    code_tx: StdMutex<Option<oneshot::Sender<String>>>,
}

impl Session {
    pub fn phase(&self) -> Phase {
        self.phase.borrow().clone()
    }

    pub fn set(&self, phase: Phase) {
        // Fails only when nobody is watching, which is the normal case for a
        // login nobody has polled yet.
        let _ = self.phase.send(phase);
    }

    pub fn watch(&self) -> watch::Receiver<Phase> {
        self.phase.subscribe()
    }

    /// Hand over the code the user pasted. Once only.
    pub fn submit_code(&self, code: String) -> Result<()> {
        let tx = self
            .code_tx
            .lock()
            .expect("login session mutex poisoned")
            .take();
        match tx {
            Some(tx) => {
                let _ = tx.send(code);
                Ok(())
            }
            None => bail!("this login is not waiting for a code"),
        }
    }

    /// Block until the login leaves [`Phase::Opening`], so the request that
    /// started it can answer with a URL instead of "check back later".
    pub async fn opened(&self) -> Phase {
        let mut rx = self.watch();
        loop {
            {
                let phase = rx.borrow_and_update().clone();
                if !matches!(phase, Phase::Opening) {
                    return phase;
                }
            }
            if rx.changed().await.is_err() {
                return Phase::Failed {
                    message: "the login task ended before it produced a URL".into(),
                };
            }
        }
    }
}

/// The [`LoginUi`] the daemon presents: publish the URL, wait for a code.
pub struct HttpLoginUi {
    session: Arc<Session>,
    needs_code: bool,
    code_rx: StdMutex<Option<oneshot::Receiver<String>>>,
}

impl LoginUi for HttpLoginUi {
    /// Emphatically does *not* open a browser. The daemon may have no display
    /// at all — under WSL, over SSH, in a container — and the viewer that asked
    /// for this login is the thing sitting in front of a human.
    fn open(&self, url: &str) -> Result<()> {
        self.session.set(Phase::Waiting {
            authorize_url: url.to_string(),
            needs_code: self.needs_code,
        });
        Ok(())
    }

    fn read_code(&self) -> Result<String> {
        // Unreachable: `code_channel` below is implemented, and the OAuth
        // client prefers it. Blocking here would park a runtime worker on a
        // human for up to five minutes.
        bail!("this login delivers its code over HTTP, not stdin")
    }

    /// The pasted code, raced against the loopback listener.
    ///
    /// Claude's flow needs both arms live at once: the browser may redirect
    /// straight to us, or — with the daemon in WSL and the browser on Windows —
    /// it may fail to reach us and show a code instead. Whichever arrives
    /// first wins.
    fn paste_channel(&self) -> Option<oneshot::Receiver<String>> {
        self.take_code()
    }

    /// The pasted code as the *only* route, for a provider that never
    /// redirects back.
    fn code_channel(&self) -> Option<oneshot::Receiver<String>> {
        self.take_code()
    }
}

impl HttpLoginUi {
    /// One receiver behind both hooks. The OAuth client calls exactly one of
    /// them per login, decided by the redirect mode, so they cannot compete.
    fn take_code(&self) -> Option<oneshot::Receiver<String>> {
        self.code_rx
            .lock()
            .expect("login session mutex poisoned")
            .take()
    }
}

/// Create a session and the UI that drives it.
pub fn session(provider: &str, needs_code: bool) -> (Arc<Session>, HttpLoginUi) {
    let (code_tx, code_rx) = oneshot::channel();
    let (phase, _) = watch::channel(Phase::Opening);

    let session = Arc::new(Session {
        id: uw_core::auth::random_id(),
        provider: provider.to_string(),
        phase,
        code_tx: StdMutex::new(Some(code_tx)),
    });

    let ui = HttpLoginUi {
        session: session.clone(),
        needs_code,
        code_rx: StdMutex::new(Some(code_rx)),
    };
    (session, ui)
}

/// Every login in flight, at most one per provider.
#[derive(Default)]
pub struct Logins {
    inner: Mutex<HashMap<String, Entry>>,
}

struct Entry {
    session: Arc<Session>,
    task: JoinHandle<()>,
}

// A finished session is deliberately *not* removed from the map. The UI learns
// that a login succeeded by polling this very endpoint, so dropping the entry
// on completion would turn the moment of success into a 404. Entries are
// reclaimed when the next login for that provider replaces them, which also
// avoids a race between the task finishing and `install` recording it.

impl Logins {
    /// Register a running login, replacing and cancelling any previous attempt
    /// at the same provider.
    ///
    /// Replacing rather than refusing: a user who closed the browser tab and
    /// clicked "Sign in" again is asking for a new attempt, not an error, and
    /// the old task would otherwise sit on its loopback port for five minutes
    /// and block the new one from binding it. Codex's fixed port 1455 makes
    /// that failure certain rather than merely likely.
    pub async fn install(&self, session: Arc<Session>, task: JoinHandle<()>) {
        let mut inner = self.inner.lock().await;
        if let Some(old) = inner.insert(session.provider.clone(), Entry { session, task }) {
            old.task.abort();
            old.session.set(Phase::Failed {
                message: "superseded by a newer sign-in attempt".into(),
            });
        }
    }

    pub async fn get(&self, provider: &str) -> Option<Arc<Session>> {
        self.inner.lock().await.get(provider).map(|e| e.session.clone())
    }

    pub async fn abort_all(&self) {
        for (_, entry) in self.inner.lock().await.drain() {
            entry.task.abort();
        }
    }
}
