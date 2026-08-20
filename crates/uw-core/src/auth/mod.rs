//! Authentication.
//!
//! Three mechanisms sit behind one interface so the UI shows a single
//! "Connect" button per provider and never branches on which one is in play:
//!
//! - [`AuthMode::OwnGrant`]  — our own OAuth grant, our own refresh token. The
//!   only mode that works somewhere the vendor CLI does not exist (Android).
//! - [`AuthMode::Delegated`] — borrow a CLI's credential strictly read-only.
//!   Never refreshed: both Claude and Codex rotate refresh tokens, so
//!   refreshing a borrowed one logs the user out of the real CLI.
//! - [`AuthMode::ApiKey`]    — a pasted key.

pub mod loopback;
pub mod oauth;
pub mod pkce;
pub mod store;

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

pub use oauth::{Credential, Flow, LoginUi, OAuthClient, OAuthConfig, RedirectMode, TokenBody};
pub use pkce::random_id;
pub use store::TokenStore;

use crate::model::AuthKind;

#[derive(Debug, Clone)]
pub enum AuthMode {
    OwnGrant(OAuthConfig),
    Delegated { cred_path: PathBuf },
    ApiKey { keyring_entry: String },
}

impl AuthMode {
    pub fn kind(&self) -> AuthKind {
        match self {
            AuthMode::OwnGrant(_) => AuthKind::OwnGrant,
            AuthMode::Delegated { .. } => AuthKind::Delegated,
            AuthMode::ApiKey { .. } => AuthKind::ApiKey,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            AuthMode::OwnGrant(_) => "own",
            AuthMode::Delegated { .. } => "delegated",
            AuthMode::ApiKey { .. } => "api-key",
        }
    }
}

/// Holds a provider's credential and keeps it fresh.
///
/// The mutex makes refresh single-flight: several pollers can call
/// [`Self::access_token`] concurrently, but only one network refresh happens
/// and the losers observe the already-refreshed credential.
pub struct TokenSource {
    provider: String,
    mode: AuthMode,
    cached: Arc<Mutex<Option<Credential>>>,
}

impl TokenSource {
    pub fn new(provider: impl Into<String>, mode: AuthMode) -> Self {
        TokenSource {
            provider: provider.into(),
            mode,
            cached: Arc::new(Mutex::new(None)),
        }
    }

    pub fn mode(&self) -> &AuthMode {
        &self.mode
    }

    pub fn kind(&self) -> AuthKind {
        self.mode.kind()
    }

    /// Interactive login. Only meaningful for `OwnGrant`.
    /// Returns the new credential rather than only storing it, so the caller
    /// can run the adapter's `enrich` hook against it and store the result.
    pub async fn login(&self, ui: &dyn LoginUi) -> Result<Credential> {
        let AuthMode::OwnGrant(cfg) = &self.mode else {
            bail!(
                "`{}` is configured for {} auth, which has no login step. \
                 Set auth = \"own\" for this provider first.",
                self.provider,
                self.mode.name()
            );
        };

        let cred = OAuthClient::new(cfg.clone()).login(ui).await?;
        self.store(cred.clone()).await?;
        Ok(cred)
    }

    /// Persist a credential and refresh the in-process cache.
    pub async fn store(&self, cred: Credential) -> Result<()> {
        TokenStore::save(&self.provider, &cred)?;
        *self.cached.lock().await = Some(cred);
        Ok(())
    }

    /// The store entry this mode reads from.
    ///
    /// A pasted token lives under its own key so it can coexist with an OAuth
    /// grant for the same provider. Logout has to honour that: deleting
    /// `<provider>` while the provider is in token mode reported success and
    /// removed nothing.
    fn entry(&self) -> &str {
        match &self.mode {
            AuthMode::ApiKey { keyring_entry } => keyring_entry,
            _ => &self.provider,
        }
    }

    pub async fn logout(&self) -> Result<()> {
        TokenStore::delete(self.entry())?;
        *self.cached.lock().await = None;
        Ok(())
    }

    /// A usable access token, refreshed if needed.
    pub async fn access_token(&self) -> Result<Credential> {
        match &self.mode {
            AuthMode::OwnGrant(cfg) => self.own_grant_token(cfg).await,
            AuthMode::Delegated { cred_path } => self.delegated_token(cred_path).await,
            AuthMode::ApiKey { keyring_entry } => match TokenStore::load(keyring_entry)? {
                Some(c) => Ok(c),
                None => bail!(
                    "no API key stored for `{}` — paste one from the panel, or run \
                     `uw auth token {}`",
                    self.provider,
                    self.provider
                ),
            },
        }
    }

    async fn own_grant_token(&self, cfg: &OAuthConfig) -> Result<Credential> {
        let mut guard = self.cached.lock().await;

        if guard.is_none() {
            *guard = TokenStore::load(&self.provider)?;
        }

        let Some(cred) = guard.clone() else {
            // Worded for both front ends. This lands in a provider tile as
            // often as it lands in a terminal, and "run `uw auth login`" on a
            // Windows machine with no CLI installed is a dead end.
            bail!(
                "not signed in to `{}` — sign in from the panel, or run \
                 `uw auth login {}`",
                self.provider,
                self.provider
            );
        };

        if !cred.is_expired() {
            return Ok(cred);
        }

        let Some(rt) = cred.refresh_token.clone() else {
            bail!(
                "`{}` credential expired and carries no refresh token — \
                 sign in again from the panel, or run `uw auth login {}`",
                self.provider,
                self.provider
            );
        };

        let mut fresh = OAuthClient::new(cfg.clone()).refresh(&rt).await?;

        // Some providers drop `extra` on refresh; carry forward what we learned
        // at login (Codex's account_id) rather than losing it.
        for (k, v) in &cred.extra {
            fresh.extra.entry(k.clone()).or_insert_with(|| v.clone());
        }

        // Persist BEFORE handing the token out. If the provider rotated the
        // refresh token and we crashed after using it but before saving, the
        // stored one would already be dead and the account locked out.
        TokenStore::save(&self.provider, &fresh)?;
        *guard = Some(fresh.clone());
        Ok(fresh)
    }

    /// Read a vendor CLI's credential file. Strictly read-only, never refreshed.
    async fn delegated_token(&self, path: &Path) -> Result<Credential> {
        let cred = crate::providers::read_delegated(&self.provider, path)?;
        if cred.is_expired() {
            bail!(
                "the {} CLI's token has expired. usage-watcher will not refresh a \
                 borrowed token (that would sign you out of the CLI) — run the CLI \
                 once to refresh it, or switch this provider to auth = \"own\".",
                self.provider
            );
        }
        Ok(cred)
    }
}
