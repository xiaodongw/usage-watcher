//! The running set of pollers, kept in step with the config file.
//!
//! Providers used to be read once at startup, which was fine when the only way
//! to add one was to edit TOML and restart. Now the config screen adds and
//! removes them while the daemon is serving, so the set of poll tasks has to be
//! reconciled rather than built.
//!
//! Reconciliation is deliberately restart-based: when a provider's settings
//! change, its task is stopped and a new one started, rather than the running
//! task being told to reload. That is simpler to reason about, and it has a
//! property the UI depends on — [`poll::run`] polls *before* its first sleep,
//! so a restart doubles as "fetch this one right now". Signing in and then
//! waiting five minutes for the tile to appear would make the login feel
//! broken.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use uw_core::collect::Poller;
use uw_core::config::ProviderConfig;
use uw_core::providers::Any;
use uw_core::Config;

use crate::hub::Hub;
use crate::poll::{self, Schedule};

struct Task {
    /// What the config said when this task was started. A task whose settings
    /// no longer match is restarted; comparing is what keeps `sync` from
    /// tearing down every provider on every unrelated change.
    settings: ProviderConfig,
    stop: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

pub struct Supervisor {
    hub: Arc<Hub>,
    http: uw_core::reqwest::Client,
    tasks: Mutex<HashMap<String, Task>>,
}

impl Supervisor {
    pub fn new(hub: Arc<Hub>, http: uw_core::reqwest::Client) -> Self {
        Supervisor {
            hub,
            http,
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Bring the running tasks in line with `cfg`.
    ///
    /// Idempotent, so callers can run it after any change without working out
    /// what actually changed.
    pub async fn sync(&self, cfg: &Config) {
        let mut tasks = self.tasks.lock().await;

        // Gone from config, or switched off: stop the task and take the tile
        // off the panel. A removed provider must not leave a ghost behind.
        let stale: Vec<String> = tasks
            .keys()
            .filter(|id| !cfg.is_enabled(id))
            .cloned()
            .collect();
        for id in stale {
            Self::stop(&mut tasks, &id).await;
            self.hub.forget(&id).await;
        }

        for adapter in Any::all() {
            let id = adapter.id();
            if !cfg.is_enabled(id) {
                continue;
            }
            let Some(settings) = cfg.providers.get(id) else {
                continue;
            };
            // Already running with exactly these settings — leave it alone
            // rather than resetting its backoff and re-polling.
            if tasks.get(id).is_some_and(|t| &t.settings == settings) {
                continue;
            }
            Self::stop(&mut tasks, id).await;
            if let Some(task) = self.spawn(adapter, cfg) {
                tasks.insert(id.to_string(), task);
            }
        }
    }

    /// Restart one provider even if its settings are unchanged.
    ///
    /// What a completed login calls: the credential moved but the config did
    /// not, and the running task is sitting on a backoff sleep holding an
    /// error it no longer has.
    pub async fn restart(&self, id: &str, cfg: &Config) {
        let mut tasks = self.tasks.lock().await;
        Self::stop(&mut tasks, id).await;
        if !cfg.is_enabled(id) {
            return;
        }
        if let Some(adapter) = Any::by_id(id) {
            if let Some(task) = self.spawn(adapter, cfg) {
                tasks.insert(id.to_string(), task);
            }
        }
    }

    pub async fn shutdown(&self) {
        let mut tasks = self.tasks.lock().await;
        let ids: Vec<String> = tasks.keys().cloned().collect();
        for id in ids {
            Self::stop(&mut tasks, &id).await;
        }
    }

    pub async fn running(&self) -> usize {
        self.tasks.lock().await.len()
    }

    fn spawn(&self, adapter: Any, cfg: &Config) -> Option<Task> {
        let id = adapter.id();
        let settings = cfg.providers.get(id)?.clone();
        let poller = Poller::new(adapter, cfg)?;

        let (active, idle) = cfg.intervals(id, adapter.poll_intervals());
        tracing::info!(provider = id, "polling every {active}s active / {idle}s idle");

        let (stop, stop_rx) = watch::channel(false);
        let handle = tokio::spawn(poll::run(
            poller,
            self.hub.clone(),
            self.http.clone(),
            Schedule::from_secs((active, idle)),
            stop_rx,
        ));
        Some(Task {
            settings,
            stop,
            handle,
        })
    }

    /// Stop a task and wait for it to notice.
    ///
    /// Awaited rather than fired and forgotten: a task that is mid-poll would
    /// otherwise write its result into the hub *after* the provider was
    /// removed, putting the tile back on a panel the user just cleared.
    async fn stop(tasks: &mut HashMap<String, Task>, id: &str) {
        let Some(task) = tasks.remove(id) else {
            return;
        };
        let _ = task.stop.send(true);
        let _ = task.handle.await;
        tracing::debug!(provider = id, "poller stopped");
    }
}
