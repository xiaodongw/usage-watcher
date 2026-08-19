//! Provider adapters.
//!
//! Each adapter knows two things and nothing else: how to obtain a credential
//! (both own-grant and delegated), and how to turn one HTTP response into a
//! [`crate::model::Provider`].

pub mod claude;
pub mod codex;

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

use crate::auth::{AuthMode, Credential, TokenSource};
use crate::model::Provider;

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
    fn oauth_config(&self) -> crate::auth::OAuthConfig;

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
            AuthPreference::Own => Ok(AuthMode::OwnGrant(self.oauth_config())),
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

/// Read a vendor CLI's credential file. Dispatches on provider id because each
/// CLI uses a different on-disk shape.
///
/// Always opened read-only, and the caller must never refresh the result.
pub fn read_delegated(provider: &str, path: &Path) -> Result<Credential> {
    match provider {
        "claude" => claude::read_delegated(path),
        "codex" => codex::read_delegated(path),
        other => bail!("`{other}` does not support delegated auth"),
    }
}

/// Run a provider's [`Adapter::enrich`] hook. Dispatches on provider id for
/// the same reason [`read_delegated`] does: callers hold a string, not a type.
pub async fn enrich(
    provider: &str,
    http: &reqwest::Client,
    cred: &mut Credential,
) -> Result<()> {
    match provider {
        "claude" => claude::Claude.enrich(http, cred).await,
        "codex" => codex::Codex.enrich(http, cred).await,
        _ => Ok(()),
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
