//! One poll of one provider, shared by the CLI and the daemon.
//!
//! Both need exactly the same sequence — resolve the credential, refresh it if
//! stale, call the adapter — and both need a failure to render as a visible
//! tile rather than a missing one. Keeping that in one place is what stops the
//! two front ends from disagreeing about what "Claude is down" looks like.

use anyhow::{anyhow, Result};
use chrono::Utc;

use crate::auth::TokenSource;
use crate::model::{AuthKind, Provider, Status};
use crate::providers::{Any, AuthPreference};
use crate::Config;

/// A provider wired up and ready to poll repeatedly.
///
/// The [`TokenSource`] is held for the life of the poller, not rebuilt per
/// poll: it owns the credential cache and the single-flight refresh mutex, so
/// recreating it would mean re-reading the keychain every minute and losing the
/// guarantee that only one refresh is ever in flight.
pub struct Poller {
    adapter: Any,
    pref: AuthPreference,
    /// A provider can be misconfigured rather than merely failing — delegated
    /// mode with no vendor CLI installed, say. That error is kept rather than
    /// returned from the constructor, so the provider still gets a tile saying
    /// what is wrong instead of silently disappearing.
    source: std::result::Result<TokenSource, String>,
}

impl Poller {
    /// `None` when the provider is switched off in config — a disabled
    /// provider should vanish, not appear as an error.
    pub fn new(adapter: Any, cfg: &Config) -> Option<Self> {
        if !cfg.is_enabled(adapter.id()) {
            return None;
        }
        let pref = cfg.auth_pref(adapter.id());
        let source = adapter.token_source(pref).map_err(|e| format!("{e:#}"));
        Some(Poller {
            adapter,
            pref,
            source,
        })
    }

    pub fn id(&self) -> &'static str {
        self.adapter.id()
    }

    pub fn label(&self) -> &'static str {
        self.adapter.label()
    }

    pub fn auth_kind(&self) -> AuthKind {
        match self.pref {
            AuthPreference::Own => AuthKind::OwnGrant,
            AuthPreference::Delegated => AuthKind::Delegated,
            AuthPreference::Token => AuthKind::ApiKey,
        }
    }

    pub async fn poll(&self, http: &reqwest::Client) -> Result<Provider> {
        let source = self.source.as_ref().map_err(|e| anyhow!("{e}"))?;
        let cred = source.access_token().await?;
        self.adapter.fetch(http, &cred, source.kind()).await
    }

    /// Poll, turning any failure into a visible tile.
    pub async fn poll_or_tile(&self, http: &reqwest::Client) -> Provider {
        match self.poll(http).await {
            Ok(p) => p,
            Err(e) => self.error_tile(format!("{e:#}")),
        }
    }

    /// The tile to show when [`Self::poll`] failed.
    pub fn error_tile(&self, message: String) -> Provider {
        error_tile(self.id(), self.label(), self.auth_kind(), message)
    }
}

/// A provider that failed, rendered as a tile.
///
/// Never carries meters: a stale number beside an error reads as current, which
/// is worse than showing no number at all.
pub fn error_tile(id: &str, label: &str, auth: AuthKind, message: String) -> Provider {
    Provider {
        id: id.to_string(),
        label: label.to_string(),
        plan: None,
        status: Status::Error { message },
        auth,
        updated_at: Utc::now(),
        meters: Vec::new(),
    }
}

/// Build a poller for every enabled provider.
pub fn pollers(cfg: &Config) -> Vec<Poller> {
    Any::all()
        .into_iter()
        .filter_map(|a| Poller::new(a, cfg))
        .collect()
}

/// Poll every enabled provider once, concurrently.
///
/// This is the whole of `uw status`. The daemon does not use it — it polls each
/// provider on its own schedule rather than in lockstep.
pub async fn poll_all(cfg: &Config, http: &reqwest::Client) -> Vec<Provider> {
    let pollers = pollers(cfg);
    futures_util::future::join_all(pollers.iter().map(|p| p.poll_or_tile(http))).await
}
