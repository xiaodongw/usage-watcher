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
        // `send_replace`, emphatically not `send`.
        //
        // `watch::Sender::send` returns `Err` when every receiver has been
        // dropped — and, crucially, *does not store the value*. Nothing here
        // holds a long-lived receiver: `opened()` subscribes, waits for the URL
        // and drops its own, and the status endpoint only ever `borrow`s. So
        // every phase written after that first one — `Done`, `Failed`, all of
        // them — was silently discarded, and the session sat on `Waiting`
        // forever no matter what the login actually did.
        //
        // That is exactly what a completed browser sign-in looked like from the
        // panel: the provider signed in, the poller restarted, and the screen
        // went on saying "Waiting for the browser…" because the only thing it
        // could read still said so. `send_replace` always stores, and still
        // wakes any receiver that happens to exist.
        self.phase.send_replace(phase);
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
    /// Register a running login, replacing any finished attempt at the same
    /// provider.
    ///
    /// Callers must only reach this once they have established that no login
    /// is still in flight — see the reuse check in `api::begin_login`. Racing
    /// two attempts is not merely wasteful: the second cannot bind the loopback
    /// port until the first lets go of it, and if the browser is still pointed
    /// at the first one's URL the code comes back to a listener whose task has
    /// been cancelled. That looks like success in the browser and a login that
    /// never completes everywhere else.
    ///
    /// The supersede path is kept anyway, because "finished" is decided a
    /// moment before this runs and two clicks can still slip between.
    pub async fn install(&self, session: Arc<Session>, task: JoinHandle<()>) {
        let mut inner = self.inner.lock().await;
        if let Some(old) = inner.insert(session.provider.clone(), Entry { session, task }) {
            old.task.abort();
            let _ = old.task.await;
            old.session.set(Phase::Failed {
                message: "superseded by a newer sign-in attempt".into(),
            });
        }
    }

    /// Abandon a login, freeing its loopback port.
    ///
    /// Leaving the sign-in screen has to do this. A task left running holds its
    /// redirect port for the full five-minute timeout — and Codex's port is a
    /// fixed 1455, registered with the provider, so nothing else can be used
    /// instead. The next attempt, from us or from `codex login`, would simply
    /// fail to start.
    ///
    /// Awaits the cancellation rather than firing and forgetting, so that by
    /// the time this returns the socket really is closed and an immediate
    /// retry can bind it.
    pub async fn cancel(&self, provider: &str) -> bool {
        let Some(entry) = self.inner.lock().await.remove(provider) else {
            return false;
        };
        entry.task.abort();
        let _ = entry.task.await;
        entry.session.set(Phase::Failed {
            message: "sign-in cancelled".into(),
        });
        true
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

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> JoinHandle<()> {
        tokio::spawn(std::future::pending())
    }

    #[tokio::test]
    async fn a_session_is_readable_until_it_is_replaced() {
        let logins = Logins::default();
        let (session, _ui) = session("claude", true);
        let id = session.id.clone();
        logins.install(session.clone(), idle()).await;

        // Deliberately still there after it finishes: the UI learns that a
        // sign-in succeeded by polling for exactly this, so dropping the entry
        // on completion would turn the moment of success into a 404.
        session.set(Phase::Done);
        let found = logins.get("claude").await.expect("session went missing");
        assert_eq!(found.id, id);
        assert!(matches!(found.phase(), Phase::Done));
    }

    #[tokio::test]
    async fn cancelling_frees_the_slot_and_says_why() {
        let logins = Logins::default();
        let (session, _ui) = session("codex", false);
        logins.install(session.clone(), idle()).await;

        assert!(logins.cancel("codex").await);
        assert!(logins.get("codex").await.is_none());
        // The screen that cancelled has usually gone, but anything still
        // holding the session must not be left reading "waiting" forever.
        assert!(matches!(session.phase(), Phase::Failed { .. }));

        assert!(!logins.cancel("codex").await, "cancelling twice is not an error");
    }

    #[tokio::test]
    async fn a_phase_is_recorded_even_with_nobody_listening() {
        // The regression that made every browser sign-in look stuck. Nothing
        // holds a receiver between `opened()` returning and the status endpoint
        // being polled, and `watch::Sender::send` discards the value outright
        // when the receiver count is zero — so `Done` and `Failed` were written
        // into the void and the session stayed on `Waiting` forever.
        let (session, _ui) = session("openrouter", false);
        assert!(matches!(session.phase(), Phase::Opening));

        session.set(Phase::Waiting {
            authorize_url: "https://example.test/a".into(),
            needs_code: false,
        });
        assert!(matches!(session.phase(), Phase::Waiting { .. }));

        session.set(Phase::Done);
        assert!(matches!(session.phase(), Phase::Done), "the outcome was dropped");
    }

    #[tokio::test]
    async fn a_code_can_only_be_submitted_once() {
        let (session, ui) = session("claude", true);
        let rx = ui.code_channel().expect("the first take yields the receiver");
        assert!(ui.code_channel().is_none(), "two flows must not share one code");

        session.submit_code("abc".into()).unwrap();
        assert_eq!(rx.await.unwrap(), "abc");
        // The sender is spent; a stale tab posting again gets an error rather
        // than silently doing nothing.
        assert!(session.submit_code("def".into()).is_err());
    }

    #[tokio::test]
    async fn opened_waits_for_the_url_and_then_returns_it() {
        let (session, ui) = session("codex", false);
        let watcher = tokio::spawn({
            let session = session.clone();
            async move { session.opened().await }
        });

        ui.open("https://example.test/authorize").unwrap();

        match watcher.await.unwrap() {
            Phase::Waiting {
                authorize_url,
                needs_code,
            } => {
                assert_eq!(authorize_url, "https://example.test/authorize");
                assert!(!needs_code);
            }
            other => panic!("expected Waiting, got {other:?}"),
        }
    }
}
