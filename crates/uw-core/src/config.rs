//! `~/.config/usage-watcher/config.toml`.
//!
//! Holds the per-provider auth toggle and nothing secret — credentials live in
//! the OS keychain.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::providers::AuthPreference;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
}

/// `uwd` settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Loopback by default. Anything else demands a `token` — see
    /// [`DaemonConfig::check`].
    #[serde(default = "bind_default")]
    pub bind: String,
    /// Bearer token required on every request except `/health`. Optional on
    /// loopback, mandatory anywhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// How many snapshots to keep in memory for burn-rate. At the default
    /// intervals this is roughly a day.
    #[serde(default = "history_default")]
    pub history: usize,
}

fn bind_default() -> String {
    "127.0.0.1:7878".to_string()
}

fn history_default() -> usize {
    1500
}

impl Default for DaemonConfig {
    fn default() -> Self {
        DaemonConfig {
            bind: bind_default(),
            token: None,
            history: history_default(),
        }
    }
}

impl DaemonConfig {
    /// Refuse to expose usage data to a network without a token.
    ///
    /// The snapshot carries no secrets, but it does say when you are near your
    /// limits, and an unauthenticated listener on a LAN is not something to
    /// enable by accident. Tailscale addresses count as "not loopback".
    pub fn check(&self) -> Result<std::net::SocketAddr> {
        let addr: std::net::SocketAddr = self
            .bind
            .parse()
            .with_context(|| format!("`{}` is not a valid host:port", self.bind))?;

        if !addr.ip().is_loopback() && self.token.is_none() {
            anyhow::bail!(
                "refusing to bind {addr}: a non-loopback address needs \
                 `[daemon] token = \"...\"` in {}",
                Config::path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "the config file".into())
            );
        }
        Ok(addr)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub auth: AuthPreference,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    /// Seconds between polls while the provider is in use. Falls back to the
    /// adapter's own default; floored at 30s so the watcher can never become a
    /// meaningful share of your own usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_active: Option<u64>,
    /// Seconds between polls while nothing is being consumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_idle: Option<u64>,
}

fn enabled_default() -> bool {
    true
}

/// Hand-written rather than derived: `#[derive(Default)]` would give
/// `enabled: false`, since `serde(default = ...)` only applies when
/// deserializing. That silently disabled any provider whose config entry was
/// created by `set_auth_pref`.
impl Default for ProviderConfig {
    fn default() -> Self {
        ProviderConfig {
            auth: AuthPreference::default(),
            enabled: enabled_default(),
            interval_active: None,
            interval_idle: None,
        }
    }
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        Ok(dirs::config_dir()
            .context("no config directory on this platform")?
            .join("usage-watcher")
            .join("config.toml"))
    }

    /// Load config, falling back to defaults when the file does not exist yet.
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Config::default());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("{} is not valid TOML", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("could not create {}", dir.display()))?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)
            .with_context(|| format!("could not write {}", path.display()))
    }

    /// What the config file says, or `None` if it says nothing.
    ///
    /// Deliberately not defaulted here: the fallback belongs to the adapter,
    /// which knows whether it has a vendor CLI to borrow from. Use
    /// [`crate::providers::Any::auth_pref`] unless you truly want the raw value.
    pub fn configured_auth(&self, provider: &str) -> Option<AuthPreference> {
        self.providers.get(provider).map(|p| p.auth)
    }

    pub fn set_auth_pref(&mut self, provider: &str, pref: AuthPreference) {
        self.providers.entry(provider.to_string()).or_default().auth = pref;
    }

    /// Per-provider poll intervals in seconds, config overriding the adapter's
    /// defaults, with the 30-second floor applied to both.
    pub fn intervals(&self, provider: &str, defaults: (u64, u64)) -> (u64, u64) {
        const FLOOR: u64 = 30;
        let c = self.providers.get(provider);
        let active = c.and_then(|p| p.interval_active).unwrap_or(defaults.0);
        let idle = c.and_then(|p| p.interval_idle).unwrap_or(defaults.1);
        (active.max(FLOOR), idle.max(FLOOR))
    }

    pub fn is_enabled(&self, provider: &str) -> bool {
        self.providers.get(provider).map(|p| p.enabled).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unconfigured_provider_is_enabled_and_defers_its_auth_to_the_adapter() {
        let c = Config::default();
        assert_eq!(c.configured_auth("claude"), None);
        assert!(c.is_enabled("claude"));
    }

    #[test]
    fn setting_auth_does_not_disable_the_provider() {
        // Regression: a derived Default gave `enabled: false`, so switching a
        // provider's auth mode silently dropped it from every snapshot.
        let mut c = Config::default();
        c.set_auth_pref("claude", AuthPreference::Own);
        assert!(c.is_enabled("claude"));

        let back: Config = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert!(back.is_enabled("claude"));
    }

    #[test]
    fn round_trips_through_toml() {
        let mut c = Config::default();
        c.set_auth_pref("claude", AuthPreference::Own);
        c.set_auth_pref("codex", AuthPreference::Delegated);

        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();

        assert_eq!(back.configured_auth("claude"), Some(AuthPreference::Own));
        assert_eq!(back.configured_auth("codex"), Some(AuthPreference::Delegated));
    }

    #[test]
    fn loopback_needs_no_token() {
        let d = DaemonConfig::default();
        assert_eq!(d.check().unwrap().to_string(), "127.0.0.1:7878");
    }

    #[test]
    fn a_network_bind_without_a_token_is_refused() {
        // The snapshot holds no secrets, but it does broadcast when you are
        // close to your limits. Opening that to a LAN must be deliberate.
        let d = DaemonConfig {
            bind: "0.0.0.0:7878".into(),
            token: None,
            ..Default::default()
        };
        let err = d.check().unwrap_err().to_string();
        assert!(err.contains("token"), "{err}");
    }

    #[test]
    fn a_network_bind_with_a_token_is_allowed() {
        let d = DaemonConfig {
            bind: "100.64.0.1:7878".into(),
            token: Some("s3cret".into()),
            ..Default::default()
        };
        assert!(d.check().is_ok());
    }

    #[test]
    fn intervals_never_drop_below_the_floor() {
        let c: Config = toml::from_str(
            r#"
            [providers.claude]
            interval_active = 1
            "#,
        )
        .unwrap();
        // A hand-edited 1-second interval must not turn the watcher into a
        // meaningful share of the user's own quota.
        assert_eq!(c.intervals("claude", (60, 300)), (30, 300));
    }

    #[test]
    fn intervals_fall_back_to_the_adapter_defaults() {
        assert_eq!(Config::default().intervals("codex", (120, 600)), (120, 600));
    }

    #[test]
    fn parses_hand_written_toml() {
        let c: Config = toml::from_str(
            r#"
            [providers.claude]
            auth = "own"

            [providers.codex]
            auth = "delegated"
            enabled = false
            "#,
        )
        .unwrap();
        assert_eq!(c.configured_auth("claude"), Some(AuthPreference::Own));
        assert!(!c.is_enabled("codex"));
        assert!(c.is_enabled("claude"));
    }
}
