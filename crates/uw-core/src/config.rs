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
    pub providers: BTreeMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub auth: AuthPreference,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
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

    pub fn auth_pref(&self, provider: &str) -> AuthPreference {
        self.providers
            .get(provider)
            .map(|p| p.auth)
            .unwrap_or_default()
    }

    pub fn set_auth_pref(&mut self, provider: &str, pref: AuthPreference) {
        self.providers.entry(provider.to_string()).or_default().auth = pref;
    }

    pub fn is_enabled(&self, provider: &str) -> bool {
        self.providers.get(provider).map(|p| p.enabled).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_provider_defaults_to_delegated_and_enabled() {
        let c = Config::default();
        assert_eq!(c.auth_pref("claude"), AuthPreference::Delegated);
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

        assert_eq!(back.auth_pref("claude"), AuthPreference::Own);
        assert_eq!(back.auth_pref("codex"), AuthPreference::Delegated);
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
        assert_eq!(c.auth_pref("claude"), AuthPreference::Own);
        assert!(!c.is_enabled("codex"));
        assert!(c.is_enabled("claude"));
    }
}
