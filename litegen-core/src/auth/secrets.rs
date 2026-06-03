//! API-key id/secret pair generation and AES-256-GCM encryption for BYO provider credentials.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};

/// A freshly minted API key: a public id (shown to the user) plus a secret (shown once).
pub struct KeyPair {
    /// Public, non-secret identifier, e.g. "pk_live_<hex>". Safe to display/store in cleartext.
    pub public_id: String,
    /// The secret bearer token, e.g. "sk_live_<hex>". Returned to the caller exactly once.
    pub secret: String,
    /// SHA-256 hex of `secret` (what we persist in api_keys.key_hash).
    pub secret_hash: String,
    /// Short display prefix of the secret for the dashboard list view.
    pub prefix: String,
}

fn random_token(prefix: &str, bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    format!("{prefix}{}", hex::encode(buf))
}

/// SHA-256 hex of the input.
pub fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

/// Generate a new public-id + secret pair.
pub fn generate_key_pair() -> KeyPair {
    let public_id = random_token("pk_live_", 12);
    let secret = random_token("sk_live_", 24);
    let prefix = secret.chars().take(16).collect();
    KeyPair {
        public_id,
        prefix,
        secret_hash: sha256_hex(&secret),
        secret,
    }
}

/// Encrypt `plaintext` with AES-256-GCM. Returns (base64 ciphertext, base64 nonce).
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<(String, String), String> {
    let cipher = Aes256Gcm::new(key.into());
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher.encrypt(nonce, plaintext).map_err(|e| e.to_string())?;
    Ok((STANDARD.encode(ct), STANDARD.encode(nonce_bytes)))
}

/// Decrypt base64 ciphertext + base64 nonce produced by `encrypt`.
pub fn decrypt(key: &[u8; 32], ct_b64: &str, nonce_b64: &str) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new(key.into());
    let ct = STANDARD.decode(ct_b64).map_err(|e| e.to_string())?;
    let nonce_bytes = STANDARD.decode(nonce_b64).map_err(|e| e.to_string())?;
    // AES-GCM uses a 12-byte nonce; `Nonce::from_slice` panics on any other length,
    // so validate here to keep the Result contract instead of panicking.
    if nonce_bytes.len() != 12 {
        return Err(format!("invalid nonce length: {}", nonce_bytes.len()));
    }
    cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ct.as_ref())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_prefixed_keypair() {
        let kp = generate_key_pair();
        assert!(kp.public_id.starts_with("pk_live_"));
        assert!(kp.secret.starts_with("sk_live_"));
        assert_ne!(kp.public_id, kp.secret);
        assert_eq!(kp.secret_hash, sha256_hex(&kp.secret));
        assert!(kp.prefix.len() <= kp.secret.len());
        // two calls differ
        assert_ne!(generate_key_pair().secret, generate_key_pair().secret);
    }

    #[test]
    fn aes_roundtrip_and_wrong_key_fails() {
        let key = [7u8; 32];
        let (ct, nonce) = encrypt(&key, b"super-secret-openai-key").unwrap();
        assert_eq!(
            decrypt(&key, &ct, &nonce).unwrap(),
            b"super-secret-openai-key"
        );
        assert!(decrypt(&[9u8; 32], &ct, &nonce).is_err()); // wrong key
        assert!(decrypt(&key, &ct, "not-base64!!").is_err()); // bad nonce
        // valid base64 but wrong nonce length must return Err, not panic
        let short_nonce = STANDARD.encode([0u8; 11]);
        assert!(decrypt(&key, &ct, &short_nonce).is_err());
    }
}
