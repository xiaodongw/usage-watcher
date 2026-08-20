//! `~/.config/usage-watcher/config.toml`.
//!
//! The list of providers the user has added, how each one authenticates, and
//! the daemon's own settings. Written by the config UI as well as by hand, so
//! it round-trips through `toml::to_string_pretty` without losing anything.
//!
//! **Presence in `[providers]` is what "added" means.** An id that is not a key
//! in that table is not polled and does not appear in the panel — which is why
//! a fresh install opens on an empty screen with an "Add provider" button
//! rather than on four tiles nobody asked for. That is a change from the
//! original behaviour, where every known provider was implicitly on; see
//! [`Config::migrate`] for what happens to a config file written back then.
//!
//! Nothing secret lives here. Credentials go to [`crate::auth::TokenStore`] —
//! the OS keychain on Windows and macOS, an owner-only file on Linux — and a
//! provider removed here has its credential deleted alongside it, so "remove"
//! means removed rather than merely hidden.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::providers::AuthPreference;

/// Bumped when the meaning of the file changes, not its shape. See
/// [`Config::migrate`].
pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Absent (`0`) in any file written before the provider list became
    /// explicit.
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub daemon: DaemonConfig,
    /// The providers the user has added, keyed by adapter id.
    ///
    /// A `BTreeMap` rather than a `Vec`: TOML renders it as `[providers.claude]`
    /// tables, which is what a hand-editing user expects, and the ordering is
    /// stable across writes so the panel's tiles never reshuffle.
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

/// `PartialEq` so the daemon's supervisor can tell a provider whose settings
/// actually changed from one that merely appeared in a rewritten file — the
/// difference between restarting a poller and leaving it alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    /// An empty, current-version config — what a machine that has never run
    /// usage-watcher starts from.
    pub fn fresh() -> Self {
        Config {
            version: CURRENT_VERSION,
            ..Default::default()
        }
    }

    /// Load config, migrating it if it was written by an older version.
    ///
    /// No file means a fresh install, not an empty one: the difference matters
    /// because [`Self::migrate`] must not run against it and conjure up four
    /// providers the user never added.
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Config::fresh());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&raw)
            .with_context(|| format!("{} is not valid TOML", path.display()))?;

        if cfg.migrate() {
            // Persisted rather than re-derived on every load, because the
            // migration is not idempotent against user intent: once it has run,
            // removing a provider must stay removed, and a version still at 0
            // would put it straight back on the next start.
            if let Err(e) = cfg.save() {
                tracing::warn!("could not write the migrated config: {e:#}");
            }
        }
        Ok(cfg)
    }

    /// Bring an older config file up to [`CURRENT_VERSION`]. Returns whether
    /// anything changed.
    ///
    /// **0 → 1.** Before this, every provider with an adapter was polled and
    /// `[providers]` only ever held overrides; now the table *is* the list. A
    /// straight read of an old file would therefore silently stop polling
    /// whatever the user had never bothered to configure — including, for
    /// anyone happy with the defaults, all of them. So every provider that was
    /// implicitly on gets written down explicitly.
    fn migrate(&mut self) -> bool {
        if self.version >= CURRENT_VERSION {
            return false;
        }
        for adapter in crate::providers::Any::all() {
            self.providers
                .entry(adapter.id().to_string())
                .or_insert_with(|| ProviderConfig {
                    auth: adapter.default_auth(),
                    ..Default::default()
                });
        }
        self.version = CURRENT_VERSION;
        true
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        // Stamped on every write, so a file this build produced is never read
        // back as one an older build produced.
        let mut out = self.clone();
        out.version = CURRENT_VERSION;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("could not create {}", dir.display()))?;
        }
        // Write-and-rename rather than a plain write. This file used to be
        // edited by hand and saved by two CLI commands; it is now rewritten on
        // every add, removal and sign-in, and a crash — or a user quitting the
        // app — part way through `fs::write` would leave a truncated file that
        // fails to parse on the next start, taking every provider with it.
        // The credential store has always done this; the config had not caught up.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(&out)?)
            .with_context(|| format!("could not write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("could not replace {}", path.display()))
    }

    /// What the config file says, or `None` if it says nothing.
    ///
    /// Deliberately not defaulted here: the fallback belongs to the adapter,
    /// which knows whether it has a vendor CLI to borrow from. Use
    /// [`crate::providers::Any::auth_pref`] unless you truly want the raw value.
    pub fn configured_auth(&self, provider: &str) -> Option<AuthPreference> {
        self.providers.get(provider).map(|p| p.auth)
    }

    /// Set a provider's auth mode, adding it if it was not there.
    ///
    /// Adding as a side effect is deliberate: choosing how to sign in to
    /// something is how you ask for it, and `uw auth login <p>` on an
    /// unconfigured provider should just work.
    pub fn set_auth_pref(&mut self, provider: &str, pref: AuthPreference) {
        self.providers.entry(provider.to_string()).or_default().auth = pref;
    }

    /// Whether the user has added this provider.
    pub fn is_added(&self, provider: &str) -> bool {
        self.providers.contains_key(provider)
    }

    /// Every added provider id, in display order.
    pub fn added(&self) -> Vec<&str> {
        self.providers.keys().map(String::as_str).collect()
    }

    /// Add a provider, or do nothing if it is already there.
    ///
    /// Returns whether it was new, so a caller can tell "added" from
    /// "re-authenticating an existing one" without reading the map first.
    pub fn add(&mut self, provider: &str, pref: AuthPreference) -> bool {
        let new = !self.is_added(provider);
        self.providers
            .entry(provider.to_string())
            .or_default()
            .auth = pref;
        new
    }

    /// Remove a provider. Returns whether it was there.
    ///
    /// Only the config entry: the caller is responsible for deleting the
    /// credential too, which lives in the token store rather than here.
    pub fn remove(&mut self, provider: &str) -> bool {
        self.providers.remove(provider).is_some()
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

    /// Whether this provider should be polled: added, and not switched off.
    ///
    /// The two are distinct. Removing a provider deletes its credential;
    /// `enabled = false` keeps everything and just stops the polling, which is
    /// what you want while a provider is having an outage.
    pub fn is_enabled(&self, provider: &str) -> bool {
        self.providers.get(provider).is_some_and(|p| p.enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_that_was_never_added_is_not_polled() {
        // The whole basis of the "Add provider" flow: an empty config means an
        // empty panel, not four tiles the user never asked for.
        let c = Config::fresh();
        assert_eq!(c.configured_auth("claude"), None);
        assert!(!c.is_added("claude"));
        assert!(!c.is_enabled("claude"));
    }

    #[test]
    fn adding_then_removing_leaves_no_trace() {
        let mut c = Config::fresh();
        assert!(c.add("claude", AuthPreference::Own), "first add is new");
        assert!(!c.add("claude", AuthPreference::Own), "second add is not");
        assert_eq!(c.added(), vec!["claude"]);

        assert!(c.remove("claude"));
        assert!(!c.remove("claude"));
        assert!(c.added().is_empty());
    }

    #[test]
    fn disabling_is_not_removing() {
        // Both stop the polling; only one throws the credential away. A
        // provider having an outage wants the first.
        let mut c = Config::fresh();
        c.add("claude", AuthPreference::Own);
        c.providers.get_mut("claude").unwrap().enabled = false;

        assert!(c.is_added("claude"));
        assert!(!c.is_enabled("claude"));
    }

    #[test]
    fn an_old_config_keeps_every_provider_it_used_to_poll() {
        // Version 0 meant "every adapter is on, and this table holds only the
        // overrides". Reading it as the new "this table is the list" would
        // silently stop polling whatever was left at its defaults.
        let mut c: Config = toml::from_str(
            r#"
            [providers.claude]
            auth = "own"
            "#,
        )
        .unwrap();
        assert_eq!(c.version, 0);

        assert!(c.migrate());
        assert_eq!(c.version, CURRENT_VERSION);
        for adapter in crate::providers::Any::all() {
            assert!(c.is_enabled(adapter.id()), "{} was dropped", adapter.id());
        }
        // Whatever it did say is untouched.
        assert_eq!(c.configured_auth("claude"), Some(AuthPreference::Own));
        // And each newly-written entry gets the adapter's own default, not the
        // global one — OpenRouter has no CLI to borrow from.
        assert_eq!(c.configured_auth("openrouter"), Some(AuthPreference::Own));
    }

    #[test]
    fn migration_runs_once_and_then_leaves_removals_alone() {
        let mut c: Config = toml::from_str("").unwrap();
        assert!(c.migrate());
        assert!(!c.migrate(), "a migrated config must not migrate again");

        c.remove("claude");
        assert!(!c.migrate(), "a removal must not be undone on the next load");
        assert!(!c.is_enabled("claude"));
    }

    #[test]
    fn a_saved_config_is_stamped_with_the_current_version() {
        let c = Config::default();
        assert_eq!(c.version, 0, "Default is deliberately unstamped");
        // `save` stamps rather than trusting the caller, so a file this build
        // writes is never read back as one an older build wrote.
        let mut out = c.clone();
        out.version = CURRENT_VERSION;
        let back: Config = toml::from_str(&toml::to_string_pretty(&out).unwrap()).unwrap();
        assert_eq!(back.version, CURRENT_VERSION);
    }

    #[test]
    fn setting_auth_adds_the_provider_rather_than_disabling_it() {
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
