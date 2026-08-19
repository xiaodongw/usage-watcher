//! Provider adapters.
//!
//! Each adapter knows two things and nothing else: how to obtain a credential
//! (both own-grant and delegated), and how to turn one HTTP response into a
//! [`crate::model::Provider`].

pub mod claude;
pub mod codex;
pub mod opencode;
pub mod openrouter;

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

use crate::auth::{AuthMode, Credential, TokenSource};
use crate::model::Provider;
use crate::Config;

/// Which auth mechanism a provider should use, from config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AuthPreference {
    /// Borrow the vendor CLI's credential, read-only. Safe, zero setup, but
    /// only works where that CLI is installed.
    #[default]
    Delegated,
    /// Our own OAuth grant. Required anywhere the CLI does not exist.
    Own,
    /// A long-lived token pasted in by hand — for Claude, what
    /// `claude setup-token` prints (a one-year OAuth token, and the *supported*
    /// path for environments without an interactive browser). No refresh, and
    /// nothing to reverse-engineer.
    Token,
}

pub trait Adapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;

    /// OAuth parameters for [`AuthPreference::Own`].
    ///
    /// Fallible because not every provider has one: opencode mints its keys in
    /// a web console, and the error is where the user is told what to do
    /// instead of logging in.
    fn oauth_config(&self) -> Result<crate::auth::OAuthConfig>;

    /// What this provider uses when config says nothing.
    ///
    /// Delegated suits anything with a vendor CLI to borrow from, which is why
    /// it is the global default; a provider with no CLI overrides it rather
    /// than showing an error tile out of the box.
    fn default_auth(&self) -> AuthPreference {
        AuthPreference::Delegated
    }

    /// Where the vendor CLI keeps its credential.
    fn delegated_path(&self) -> Option<PathBuf>;

    /// Capture provider facts that exist outside the token response, once, at
    /// login. Anything written into `cred.extra` here survives refreshes.
    ///
    /// Called only after an own-grant login or an adopt — delegated mode reads
    /// the same facts straight out of the vendor CLI's file. Failure is not
    /// fatal: a missing plan name degrades one label, and refusing to store a
    /// perfectly good token over it would be far worse.
    fn enrich(
        &self,
        _http: &reqwest::Client,
        _cred: &mut Credential,
    ) -> impl std::future::Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// One poll: credential in, normalized provider out.
    fn fetch(
        &self,
        http: &reqwest::Client,
        cred: &Credential,
        kind: crate::model::AuthKind,
    ) -> impl std::future::Future<Output = Result<Provider>> + Send;

    fn auth_mode(&self, pref: AuthPreference) -> Result<AuthMode> {
        match pref {
            AuthPreference::Own => Ok(AuthMode::OwnGrant(self.oauth_config()?)),
            // Stored under a distinct key so a pasted token and an OAuth grant
            // can coexist and you can switch between them without re-entering
            // either one.
            AuthPreference::Token => Ok(AuthMode::ApiKey {
                keyring_entry: format!("{}-token", self.id()),
            }),
            AuthPreference::Delegated => match self.delegated_path() {
                Some(cred_path) => Ok(AuthMode::Delegated { cred_path }),
                None => bail!(
                    "`{}` has no delegated credential source — set auth = \"own\"",
                    self.id()
                ),
            },
        }
    }

    fn token_source(&self, pref: AuthPreference) -> Result<TokenSource> {
        Ok(TokenSource::new(self.id(), self.auth_mode(pref)?))
    }
}

/// Every adapter, as one dispatchable value.
///
/// [`Adapter`] is deliberately not object-safe — `fetch` returns an RPIT future
/// so that a poll allocates nothing — which means `Vec<dyn Adapter>` is out.
/// This enum is the price: one arm per provider, and in exchange there is
/// exactly one place that lists what providers exist.
#[derive(Debug, Clone, Copy)]
pub enum Any {
    Claude(claude::Claude),
    Codex(codex::Codex),
    OpenRouter(openrouter::OpenRouter),
    Opencode(opencode::Opencode),
}

/// The one place the arms are enumerated. Every forwarding method below is the
/// same shape, and writing them out four times each was how the third provider
/// ended up missing from two of them.
macro_rules! dispatch {
    ($self:expr, |$a:ident| $body:expr) => {
        match $self {
            Any::Claude($a) => $body,
            Any::Codex($a) => $body,
            Any::Opencode($a) => $body,
            Any::OpenRouter($a) => $body,
        }
    };
}

impl Any {
    /// Every provider that exists, in display order.
    ///
    /// This is what `uw` prints. The daemon keys its snapshot by id instead, so
    /// tiles hold still between polls — and the two agree because this order is
    /// alphabetical by id as well.
    pub fn all() -> Vec<Any> {
        vec![
            Any::Claude(claude::Claude),
            Any::Codex(codex::Codex),
            Any::Opencode(opencode::Opencode),
            Any::OpenRouter(openrouter::OpenRouter),
        ]
    }

    pub fn id(&self) -> &'static str {
        dispatch!(self, |a| a.id())
    }

    pub fn label(&self) -> &'static str {
        dispatch!(self, |a| a.label())
    }

    pub fn delegated_path(&self) -> Option<PathBuf> {
        dispatch!(self, |a| a.delegated_path())
    }

    pub fn default_auth(&self) -> AuthPreference {
        dispatch!(self, |a| a.default_auth())
    }

    pub fn token_source(&self, pref: AuthPreference) -> Result<TokenSource> {
        dispatch!(self, |a| a.token_source(pref))
    }

    pub async fn fetch(
        &self,
        http: &reqwest::Client,
        cred: &Credential,
        kind: crate::model::AuthKind,
    ) -> Result<Provider> {
        match self {
            Any::Claude(a) => a.fetch(http, cred, kind).await,
            Any::Codex(a) => a.fetch(http, cred, kind).await,
            Any::OpenRouter(a) => a.fetch(http, cred, kind).await,
            Any::Opencode(a) => a.fetch(http, cred, kind).await,
        }
    }

    pub async fn enrich(&self, http: &reqwest::Client, cred: &mut Credential) -> Result<()> {
        match self {
            Any::Claude(a) => a.enrich(http, cred).await,
            Any::Codex(a) => a.enrich(http, cred).await,
            Any::OpenRouter(a) => a.enrich(http, cred).await,
            Any::Opencode(a) => a.enrich(http, cred).await,
        }
    }

    /// The auth mode to use for this provider: whatever config says, or the
    /// adapter's own default. Reading it here rather than from [`crate::Config`]
    /// keeps callers from having to know which default belongs to which
    /// provider — get that wrong and a provider silently stops reporting.
    pub fn auth_pref(&self, cfg: &Config) -> AuthPreference {
        cfg.configured_auth(self.id())
            .unwrap_or_else(|| self.default_auth())
    }

    /// Default poll intervals in seconds, `(active, idle)`, before config
    /// overrides and the 30-second floor.
    ///
    /// "Active" means the provider is currently consuming something. Claude's
    /// 5-hour window moves fast enough to be worth a minute; Codex only
    /// publishes a 7-day bucket, so polling it that often would tell us
    /// nothing new. OpenRouter is slowest of all: a prepaid balance only moves
    /// when you spend, and the account wallet is not a per-request counter.
    pub fn poll_intervals(&self) -> (u64, u64) {
        match self {
            Any::Claude(_) => (60, 300),
            Any::Codex(_) => (120, 600),
            Any::Opencode(_) => (120, 600),
            Any::OpenRouter(_) => (300, 900),
        }
    }

    /// What `uw auth adopt` should leave the provider set to, or `None` where
    /// there is no vendor credential to adopt.
    ///
    /// The two answers are not the same thing. Claude and Codex hand over a
    /// rotating OAuth grant, which we then own and refresh — that is
    /// [`AuthPreference::Own`], and it obliges the user to re-run the vendor
    /// login. opencode hands over a static API key, which is a copy, not a
    /// transfer: nothing rotates and the CLI is unaffected.
    pub fn adopt_as(&self) -> Option<AuthPreference> {
        match self {
            Any::Claude(_) | Any::Codex(_) => Some(AuthPreference::Own),
            Any::Opencode(_) => Some(AuthPreference::Token),
            Any::OpenRouter(_) => None,
        }
    }

    /// The vendor command to re-run after an adopt, for providers where our
    /// copy and theirs would otherwise share one rotating refresh token.
    pub fn relogin_hint(&self) -> Option<&'static str> {
        match self {
            Any::Claude(_) => Some("claude auth login"),
            Any::Codex(_) => Some("codex login"),
            // Static keys. Copying one changes nothing for the CLI.
            Any::Opencode(_) | Any::OpenRouter(_) => None,
        }
    }

    /// Read the vendor credential *including* anything [`read_delegated`]
    /// deliberately withholds, for `uw auth adopt`. See the note on
    /// [`claude::read_full_credential`].
    pub fn read_full_credential(&self) -> Result<(PathBuf, Credential)> {
        let Some(path) = self.delegated_path() else {
            bail!("`{}` has no vendor credential file to adopt", self.id());
        };
        let cred = match self {
            Any::Claude(_) => claude::read_full_credential(&path)?,
            Any::Codex(_) => codex::read_full_credential(&path)?,
            Any::Opencode(_) => opencode::read_full_credential(&path)?,
            Any::OpenRouter(_) => bail!("`openrouter` has no vendor credential to adopt"),
        };
        Ok((path, cred))
    }

    /// `None` for an id we do not have an adapter for.
    pub fn by_id(id: &str) -> Option<Any> {
        Any::all().into_iter().find(|a| a.id() == id)
    }

    /// Every provider id, for CLI help and error messages.
    pub fn known_ids() -> String {
        Any::all()
            .iter()
            .map(|a| a.id())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Read a vendor CLI's credential file. Dispatches on provider id because each
/// CLI uses a different on-disk shape.
///
/// Always opened read-only, and the caller must never refresh the result.
pub fn read_delegated(provider: &str, path: &Path) -> Result<Credential> {
    match provider {
        "claude" => claude::read_delegated(path),
        "codex" => codex::read_delegated(path),
        "opencode" => opencode::read_delegated(path),
        other => bail!("`{other}` does not support delegated auth"),
    }
}

/// Run a provider's [`Adapter::enrich`] hook. Dispatches on provider id for
/// the same reason [`read_delegated`] does: callers hold a string, not a type.
pub async fn enrich(provider: &str, http: &reqwest::Client, cred: &mut Credential) -> Result<()> {
    match Any::by_id(provider) {
        Some(a) => a.enrich(http, cred).await,
        None => Ok(()),
    }
}

/// Pull the `exp` claim out of a JWT access token.
///
/// Codex's `auth.json` records only `last_refresh`, not an expiry, so the token
/// itself is the only reliable source. Signature is not verified — we are
/// reading our own token's lifetime, not authorizing anything.
pub(crate) fn jwt_expiry(token: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let exp = claims.get("exp")?.as_i64()?;
    chrono::DateTime::from_timestamp(exp, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    #[test]
    fn reads_jwt_expiry() {
        let claims = serde_json::json!({ "exp": 1787284072_i64, "sub": "x" });
        let tok = format!(
            "hdr.{}.sig",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        );
        assert_eq!(jwt_expiry(&tok).unwrap().timestamp(), 1787284072);
    }

    #[test]
    fn bad_jwt_is_none() {
        assert!(jwt_expiry("nope").is_none());
        assert!(jwt_expiry("a.b.c").is_none());
    }

    #[test]
    fn delegated_is_the_default_preference() {
        assert_eq!(AuthPreference::default(), AuthPreference::Delegated);
    }
}
