//! Kling per-request JWT authentication.
//!
//! Kling (Kuaishou) authenticates each request with a short-lived JWT minted
//! client-side from the account's Access Key + Secret Key and sent as
//! `Authorization: Bearer <jwt>`. The token is HS256 over the claims
//! `{iss: <access_key>, exp: now + 1800, nbf: now - 5}`.
//!
//! @see <https://app.klingai.com/global/dev/document-api/quickStart/userManual> — auth (JWT) overview
//! Verbatim (official Node SDK, github.com/aself101/kling-api src/auth.ts):
//!   header = { alg: 'HS256', typ: 'JWT' };
//!   payload = { iss: accessKey, exp: now + 1800, nbf: now - 5 };
//!   // TOKEN_VALIDITY_SECONDS = 1800; CLOCK_SKEW_SECONDS = 5

use crate::providers::auth::ProviderCredentials;
use crate::providers::ProviderError;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

/// Token lifetime (seconds). Matches the official Kling SDK.
const TOKEN_VALIDITY_SECONDS: u64 = 1800;
/// Clock-skew buffer applied to `nbf` (seconds).
const CLOCK_SKEW_SECONDS: u64 = 5;

/// Mint a Kling JWT from the provider credentials using the current time.
pub fn mint(creds: &ProviderCredentials) -> Result<String, ProviderError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    mint_at(creds, now)
}

/// Mint a Kling JWT for a fixed `now` (unix seconds). Testable core.
pub fn mint_at(creds: &ProviderCredentials, now: u64) -> Result<String, ProviderError> {
    let access_key = creds
        .key_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ProviderError::NotConfigured("kling: missing key_id (access key)".into()))?;
    let secret_key = creds
        .key_secret
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ProviderError::NotConfigured("kling: missing key_secret".into()))?;

    let header = r#"{"alg":"HS256","typ":"JWT"}"#;
    let exp = now + TOKEN_VALIDITY_SECONDS;
    let nbf = now.saturating_sub(CLOCK_SKEW_SECONDS);
    // Compact, fixed key order matching the official SDK payload.
    let payload = format!(r#"{{"iss":"{access_key}","exp":{exp},"nbf":{nbf}}}"#);

    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let signing_input = format!("{}.{}", b64.encode(header), b64.encode(payload));

    let mut mac = HmacSha256::new_from_slice(secret_key.as_bytes())
        .map_err(|e| ProviderError::InvalidRequest(format!("kling: hmac key error: {e}")))?;
    mac.update(signing_input.as_bytes());
    let sig = b64.encode(mac.finalize().into_bytes());

    Ok(format!("{signing_input}.{sig}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn creds(ak: &str, sk: &str) -> ProviderCredentials {
        ProviderCredentials {
            key_id: Some(ak.to_string()),
            key_secret: Some(sk.to_string()),
            ..Default::default()
        }
    }

    fn decode_part(p: &str) -> serde_json::Value {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn mints_three_part_token_with_expected_claims() {
        let jwt = mint_at(&creds("my-access-key", "my-secret"), 1_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT must have header.payload.signature");

        let header = decode_part(parts[0]);
        assert_eq!(header["alg"], "HS256");
        assert_eq!(header["typ"], "JWT");

        let payload = decode_part(parts[1]);
        assert_eq!(payload["iss"], "my-access-key");
        assert_eq!(payload["exp"], 1_000_000 + 1800);
        assert_eq!(payload["nbf"], 1_000_000 - 5);
    }

    #[test]
    fn signature_is_deterministic_for_same_inputs() {
        let a = mint_at(&creds("ak", "sk"), 42).unwrap();
        let b = mint_at(&creds("ak", "sk"), 42).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn different_secret_yields_different_signature() {
        let a = mint_at(&creds("ak", "sk1"), 42).unwrap();
        let b = mint_at(&creds("ak", "sk2"), 42).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn missing_credentials_error() {
        assert!(mint_at(&ProviderCredentials::default(), 0).is_err());
        assert!(mint_at(
            &ProviderCredentials { key_id: Some("ak".into()), ..Default::default() },
            0
        )
        .is_err());
    }
}
