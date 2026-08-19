//! Credential storage.
//!
//! Where the platform has a keychain that works without extra system libraries
//! we use it. Linux deliberately does not: the Secret Service backend needs
//! D-Bus and `libdbus-1-dev`, and WSL — the primary target here — runs no
//! Secret Service daemon at all, so a keychain-only design would simply fail to
//! start. Linux therefore gets an owner-only (0600) file, which is what `gh`,
//! `aws` and `docker` do in the same position.

use anyhow::{Context, Result};

use super::oauth::Credential;

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
const SERVICE: &str = "usage-watcher";

pub struct TokenStore;

// ---------------------------------------------------------------- keychain

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows"))]
mod backend {
    use super::*;

    fn entry(provider: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, provider)
            .with_context(|| format!("could not open keychain entry for {provider}"))
    }

    pub fn save(provider: &str, json: &str) -> Result<()> {
        entry(provider)?
            .set_password(json)
            .with_context(|| format!("could not write {provider} credential to the keychain"))
    }

    pub fn load(provider: &str) -> Result<Option<String>> {
        match entry(provider)?.get_password() {
            Ok(json) => Ok(Some(json)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e).with_context(|| format!("could not read {provider} credential")),
        }
    }

    pub fn delete(provider: &str) -> Result<()> {
        match entry(provider)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e).with_context(|| format!("could not delete {provider} credential")),
        }
    }

    pub fn describe() -> &'static str {
        "OS keychain"
    }
}

// --------------------------------------------------------------- file 0600

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
mod backend {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn path() -> Result<PathBuf> {
        Ok(dirs::data_dir()
            .context("no data directory on this platform")?
            .join("usage-watcher")
            .join("credentials.json"))
    }

    fn read_all() -> Result<BTreeMap<String, String>> {
        let p = path()?;
        if !p.exists() {
            return Ok(BTreeMap::new());
        }
        let raw = std::fs::read_to_string(&p)
            .with_context(|| format!("could not read {}", p.display()))?;
        Ok(serde_json::from_str(&raw).unwrap_or_default())
    }

    fn write_all(map: &BTreeMap<String, String>) -> Result<()> {
        let p = path()?;
        let dir = p.parent().expect("credential path always has a parent");
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        restrict(dir, 0o700)?;

        // Write to a temp file and rename, so a crash mid-write cannot leave a
        // truncated credential store behind.
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(map)?)
            .with_context(|| format!("could not write {}", tmp.display()))?;
        restrict(&tmp, 0o600)?;
        std::fs::rename(&tmp, &p)
            .with_context(|| format!("could not replace {}", p.display()))?;
        Ok(())
    }

    #[cfg(unix)]
    fn restrict(path: &std::path::Path, mode: u32) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("could not set {mode:o} on {}", path.display()))
    }

    #[cfg(not(unix))]
    fn restrict(_path: &std::path::Path, _mode: u32) -> Result<()> {
        Ok(())
    }

    pub fn save(provider: &str, json: &str) -> Result<()> {
        let mut all = read_all()?;
        all.insert(provider.to_string(), json.to_string());
        write_all(&all)
    }

    pub fn load(provider: &str) -> Result<Option<String>> {
        Ok(read_all()?.get(provider).cloned())
    }

    pub fn delete(provider: &str) -> Result<()> {
        let mut all = read_all()?;
        if all.remove(provider).is_some() {
            write_all(&all)?;
        }
        Ok(())
    }

    pub fn describe() -> &'static str {
        "owner-only file (0600)"
    }
}

impl TokenStore {
    /// Human-readable name of the active backend, for `uw auth status`.
    pub fn backend() -> &'static str {
        backend::describe()
    }

    pub fn save(provider: &str, cred: &Credential) -> Result<()> {
        backend::save(provider, &serde_json::to_string(cred)?)
    }

    pub fn load(provider: &str) -> Result<Option<Credential>> {
        match backend::load(provider)? {
            None => Ok(None),
            Some(json) => Ok(Some(serde_json::from_str(&json).with_context(|| {
                format!("stored {provider} credential is corrupt; run `uw auth login {provider}`")
            })?)),
        }
    }

    pub fn delete(provider: &str) -> Result<()> {
        backend::delete(provider)
    }
}
