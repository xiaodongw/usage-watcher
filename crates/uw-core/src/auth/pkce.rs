//! RFC 7636 PKCE. All four providers are public clients — there is no client
//! secret anywhere in this codebase, and none should be invented.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Self {
        let verifier = random_urlsafe(32);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Pkce {
            verifier,
            challenge,
        }
    }

    pub const METHOD: &'static str = "S256";
}

/// Opaque `state` nonce, checked on the callback to bind the redirect to the
/// request we actually made.
///
/// 32 bytes, not 16. RFC 6749 calls `state` opaque and sets no length, so 16
/// is defensible — but Claude Code generates `base64url(randomBytes(32))`, and
/// after comparing a rejected authorize submission field-by-field against the
/// CLI's, the 22-character state was the only value left that differed. Match
/// the reference implementation rather than argue with a server whose entire
/// vocabulary is "Invalid request format". A wider nonce costs nothing anyway.
pub fn random_state() -> String {
    random_urlsafe(32)
}

/// An unguessable identifier for something that is not a PKCE value: a login
/// session, a generated daemon token.
///
/// Same generator, because "unguessable" is the same requirement and a second
/// hand-rolled one would only be a second thing to get wrong.
pub fn random_id() -> String {
    random_urlsafe(24)
}

fn random_urlsafe(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_is_s256_of_verifier() {
        let p = Pkce::generate();
        let expect = URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes()));
        assert_eq!(p.challenge, expect);
        // Base64url, no padding — must never need escaping in a query string.
        assert!(!p.challenge.contains('=') && !p.challenge.contains('+'));
    }

    /// Both are 32 raw bytes, i.e. 43 base64url characters, matching Claude
    /// Code's `KHf()`/`XHf()`. See the note on [`random_state`].
    #[test]
    fn state_and_verifier_are_43_chars_like_the_cli() {
        assert_eq!(random_state().len(), 43);
        assert_eq!(Pkce::generate().verifier.len(), 43);
        assert_eq!(Pkce::generate().challenge.len(), 43);
    }

    #[test]
    fn values_are_unique_per_call() {
        assert_ne!(Pkce::generate().verifier, Pkce::generate().verifier);
        assert_ne!(random_state(), random_state());
    }
}
