//! Random identifiers and secrets, and the hashes LiteGen stores instead of
//! secrets.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// `prefix` followed by `bytes` random bytes from the OS, base64url without
/// padding (32 bytes → 43 characters).
pub fn random_token(prefix: &str, bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(buf))
}

/// SHA-256 of `secret`, as lowercase hex: what is stored for a token or code.
pub fn sha256_hex(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// The PKCE S256 challenge for `verifier`: base64url(SHA-256(verifier)).
pub fn s256_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_prefixed_base64url() {
        let token = random_token("lgat_", 32);
        assert!(token.starts_with("lgat_"));
        assert_eq!(token.len(), 5 + 43);
        assert!(token[5..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(random_token("lgat_", 32), token);
    }

    #[test]
    fn s256_matches_rfc_7636_appendix_b() {
        assert_eq!(s256_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }
}
