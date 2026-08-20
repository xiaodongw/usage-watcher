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

// ------------------------------------------------------------- size limits

/// Largest value one backend entry can hold, in bytes, where the backend has a
/// limit worth working around.
///
/// Windows does. `CRED_MAX_CREDENTIAL_BLOB_SIZE` is 2560 bytes and the blob is
/// the value encoded as UTF-16, so an ASCII payload gets about 1280 characters.
/// A Codex credential is a ChatGPT JWT plus a refresh token and runs to roughly
/// three kilobytes, so signing in to Codex on Windows failed outright with
/// "longer than platform limit of 2560 chars" — while Claude and OpenRouter,
/// whose tokens are short, fit and worked.
///
/// The headroom is deliberate: the limit applies to the whole credential
/// structure, not only the value, and being a few hundred bytes under costs
/// nothing.
#[cfg(target_os = "windows")]
const MAX_ENTRY_BYTES: Option<usize> = Some(2000);

/// macOS keeps arbitrarily large items, and the Linux file store has no limit
/// at all beyond the filesystem's.
#[cfg(not(target_os = "windows"))]
const MAX_ENTRY_BYTES: Option<usize> = None;

/// Marks a primary entry that holds a part count rather than a credential.
///
/// Chosen so it can never be confused with a stored value: a credential is
/// always JSON and therefore always starts with `{`. That is what lets entries
/// written before chunking existed keep working untouched.
const CHUNKED: &str = "uw-chunked:";

/// Bounds the sweep that removes parts when the primary entry is unreadable.
const MAX_PARTS: usize = 64;

fn part_entry(provider: &str, index: usize) -> String {
    format!("{provider}#{index}")
}

/// UTF-16 is what Windows measures, so it is what we count.
fn utf16_bytes(s: &str) -> usize {
    s.encode_utf16().count() * 2
}

/// Split a value into pieces that each fit, cutting only on character
/// boundaries so every piece is still valid UTF-8.
fn split(value: &str, max_bytes: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut size = 0;

    for ch in value.chars() {
        let cost = ch.len_utf16() * 2;
        // A single character wider than the limit is impossible at any sane
        // limit, but starting a new part for it would loop forever, so an
        // over-long part is preferable to a hang.
        if size + cost > max_bytes && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
            size = 0;
        }
        current.push(ch);
        size += cost;
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// The three operations a backend provides, as a trait.
///
/// Not abstraction for its own sake: the chunking below only ever runs on
/// Windows, so on any machine that can run these tests the interesting path is
/// dead code. Naming the interface lets the tests drive it against an in-memory
/// store, which is the only coverage it can have short of a Windows CI runner.
trait Entries {
    fn save(&self, entry: &str, value: &str) -> Result<()>;
    fn load(&self, entry: &str) -> Result<Option<String>>;
    fn delete(&self, entry: &str) -> Result<()>;
}

/// The real one, whichever it is for this platform.
struct Platform;

impl Entries for Platform {
    fn save(&self, entry: &str, value: &str) -> Result<()> {
        backend::save(entry, value)
    }
    fn load(&self, entry: &str) -> Result<Option<String>> {
        backend::load(entry)
    }
    fn delete(&self, entry: &str) -> Result<()> {
        backend::delete(entry)
    }
}

fn put(store: &impl Entries, max: Option<usize>, provider: &str, json: &str) -> Result<()> {
    // Whatever a previous, longer credential left behind. Done first: a stale
    // part beyond the new count would otherwise never be collected, and would
    // be read back as part of a later credential if the count grew again.
    clear_parts(store, provider)?;

    let Some(max) = max.filter(|max| utf16_bytes(json) > *max) else {
        return store.save(provider, json);
    };

    let parts = split(json, max);
    for (i, part) in parts.iter().enumerate() {
        store.save(&part_entry(provider, i), part)?;
    }
    store.save(provider, &format!("{CHUNKED}{}", parts.len()))
}

fn get(store: &impl Entries, provider: &str) -> Result<Option<String>> {
    let Some(head) = store.load(provider)? else {
        return Ok(None);
    };
    let Some(count) = head.strip_prefix(CHUNKED) else {
        return Ok(Some(head));
    };

    let count: usize = count
        .trim()
        .parse()
        .with_context(|| format!("stored {provider} credential has an unreadable part count"))?;

    let mut joined = String::new();
    for i in 0..count {
        let part = store.load(&part_entry(provider, i))?.with_context(|| {
            format!(
                "the {provider} credential is stored in {count} parts and part {i} \
                 is missing; sign in to {provider} again"
            )
        })?;
        joined.push_str(&part);
    }
    Ok(Some(joined))
}

/// Remove the numbered parts belonging to `provider`, if it has any.
///
/// Driven by the count in the primary entry, with a bounded sweep as the
/// fallback for a primary that is missing or unreadable — otherwise a corrupt
/// header would strand every part forever.
fn clear_parts(store: &impl Entries, provider: &str) -> Result<()> {
    let count = match store.load(provider) {
        Ok(Some(head)) => match head.strip_prefix(CHUNKED) {
            // Not chunked, so there is nothing extra to remove.
            None => return Ok(()),
            Some(n) => n.trim().parse::<usize>().unwrap_or(MAX_PARTS),
        },
        // Unreadable or absent: sweep, since we cannot ask.
        _ => MAX_PARTS,
    };

    for i in 0..count.min(MAX_PARTS) {
        store.delete(&part_entry(provider, i))?;
    }
    Ok(())
}

impl TokenStore {
    /// Human-readable name of the active backend, for `uw auth status`.
    pub fn backend() -> &'static str {
        backend::describe()
    }

    pub fn save(provider: &str, cred: &Credential) -> Result<()> {
        put(
            &Platform,
            MAX_ENTRY_BYTES,
            provider,
            &serde_json::to_string(cred)?,
        )
    }

    pub fn load(provider: &str) -> Result<Option<Credential>> {
        let Some(json) = get(&Platform, provider)? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(&json).with_context(|| {
            format!("stored {provider} credential is corrupt; sign in to {provider} again")
        })?))
    }

    pub fn delete(provider: &str) -> Result<()> {
        clear_parts(&Platform, provider)?;
        backend::delete(provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn credential(size: usize) -> Credential {
        Credential {
            access_token: "t".repeat(size),
            refresh_token: Some("r".repeat(size)),
            expires_at: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn a_value_that_fits_is_stored_whole() {
        // Entries written before chunking existed are plain JSON, and must keep
        // loading without any migration.
        let json = serde_json::to_string(&credential(8)).unwrap();
        assert!(json.starts_with('{'));
        assert!(!json.starts_with(CHUNKED));
    }

    #[test]
    fn splitting_covers_the_value_exactly_and_respects_the_limit() {
        let json = serde_json::to_string(&credential(4000)).unwrap();
        let parts = split(&json, 2000);

        assert!(parts.len() > 1, "a 4KB credential should not fit in one part");
        assert_eq!(parts.concat(), json, "reassembly must be lossless");
        for p in &parts {
            assert!(utf16_bytes(p) <= 2000, "a part exceeded the platform limit");
        }
    }

    #[test]
    fn splitting_never_cuts_a_character_in_half() {
        // Not expected in a token, but a corrupt part is a credential that
        // cannot be read back, and the failure would look like a bad password.
        let value = "é€𝄞".repeat(200);
        let parts = split(&value, 16);
        assert_eq!(parts.concat(), value);
        for p in &parts {
            assert!(p.chars().count() > 0);
        }
    }

    #[test]
    fn a_single_oversized_character_still_terminates() {
        // A limit below one character's width must not loop forever building
        // empty parts.
        let parts = split("𝄞𝄞", 1);
        assert_eq!(parts.concat(), "𝄞𝄞");
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn utf16_is_what_gets_measured() {
        // Windows counts the UTF-16 encoding, so an ASCII character costs two
        // bytes and an astral one costs four. Measuring UTF-8 here would let a
        // value through that the platform then rejects.
        assert_eq!(utf16_bytes("abc"), 6);
        assert_eq!(utf16_bytes("é"), 2);
        assert_eq!(utf16_bytes("𝄞"), 4);
    }

    #[test]
    fn part_entries_are_namespaced_under_the_provider() {
        assert_eq!(part_entry("codex", 0), "codex#0");
        assert_eq!(part_entry("codex-token", 3), "codex-token#3");
    }

    // ------------------------------------------------ round trips in memory

    /// Stands in for the Windows Credential Manager, limit and all.
    #[derive(Default)]
    struct Memory {
        entries: std::sync::Mutex<BTreeMap<String, String>>,
    }

    impl Memory {
        fn keys(&self) -> Vec<String> {
            self.entries.lock().unwrap().keys().cloned().collect()
        }
    }

    impl Entries for Memory {
        fn save(&self, entry: &str, value: &str) -> Result<()> {
            // The real backend rejects an oversized value outright, which is
            // the failure this whole mechanism exists to avoid. Reproduce it,
            // so a chunk that is too big fails the test rather than passing it.
            if utf16_bytes(value) > 2560 {
                anyhow::bail!(
                    "Attribute 'password encoded as UTF-16' is longer than \
                     platform limit of 2560 chars"
                );
            }
            self.entries
                .lock()
                .unwrap()
                .insert(entry.to_string(), value.to_string());
            Ok(())
        }
        fn load(&self, entry: &str) -> Result<Option<String>> {
            Ok(self.entries.lock().unwrap().get(entry).cloned())
        }
        fn delete(&self, entry: &str) -> Result<()> {
            self.entries.lock().unwrap().remove(entry);
            Ok(())
        }
    }

    #[test]
    fn a_codex_sized_credential_round_trips_through_a_capped_store() {
        // The bug this fixes: a ChatGPT JWT plus a refresh token is around
        // three kilobytes, and storing it whole failed on Windows outright.
        let store = Memory::default();
        let json = serde_json::to_string(&credential(1500)).unwrap();
        assert!(utf16_bytes(&json) > 2560, "the fixture must exceed the limit");

        put(&store, Some(2000), "codex", &json).unwrap();
        assert_eq!(get(&store, "codex").unwrap().as_deref(), Some(json.as_str()));
        assert!(store.keys().len() > 1, "it should have been split");
    }

    #[test]
    fn a_small_credential_uses_one_entry_and_no_parts() {
        let store = Memory::default();
        let json = serde_json::to_string(&credential(10)).unwrap();

        put(&store, Some(2000), "claude", &json).unwrap();
        assert_eq!(store.keys(), vec!["claude".to_string()]);
        assert_eq!(get(&store, "claude").unwrap().as_deref(), Some(json.as_str()));
    }

    #[test]
    fn shrinking_a_credential_leaves_no_stale_parts_behind() {
        // Sign in to Codex, then paste a short token. The leftover parts would
        // otherwise sit there and be re-read as part of a later, longer one.
        let store = Memory::default();
        put(&store, Some(2000), "codex", &serde_json::to_string(&credential(1500)).unwrap()).unwrap();
        assert!(store.keys().len() > 1);

        let small = serde_json::to_string(&credential(10)).unwrap();
        put(&store, Some(2000), "codex", &small).unwrap();

        assert_eq!(store.keys(), vec!["codex".to_string()]);
        assert_eq!(get(&store, "codex").unwrap().as_deref(), Some(small.as_str()));
    }

    #[test]
    fn deleting_removes_every_part() {
        let store = Memory::default();
        put(&store, Some(2000), "codex", &serde_json::to_string(&credential(1500)).unwrap()).unwrap();

        clear_parts(&store, "codex").unwrap();
        store.delete("codex").unwrap();
        assert!(store.keys().is_empty(), "left behind: {:?}", store.keys());
    }

    #[test]
    fn a_missing_part_is_an_error_not_a_silently_truncated_token() {
        // Half a JWT would be rejected by the provider as an authentication
        // failure, sending the user hunting for the wrong problem.
        let store = Memory::default();
        put(&store, Some(2000), "codex", &serde_json::to_string(&credential(1500)).unwrap()).unwrap();
        store.delete("codex#1").unwrap();

        let err = get(&store, "codex").unwrap_err().to_string();
        assert!(err.contains("part 1 is missing"), "{err}");
    }

    #[test]
    fn an_entry_written_before_chunking_still_loads() {
        // Anyone upgrading has plain-JSON entries already in their keychain,
        // and they must keep working without a migration step.
        let store = Memory::default();
        let json = serde_json::to_string(&credential(10)).unwrap();
        store.save("claude", &json).unwrap();

        assert_eq!(get(&store, "claude").unwrap().as_deref(), Some(json.as_str()));
    }
}
