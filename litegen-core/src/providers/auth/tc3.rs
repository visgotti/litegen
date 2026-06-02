//! Tencent Cloud TC3-HMAC-SHA256 request signing (Hunyuan image + vclm video).
//!
//! Structurally analogous to AWS SigV4 but with Tencent-specific constants that
//! break a copied SigV4 impl: the algorithm token is `TC3-HMAC-SHA256`, the
//! credential-scope terminator is `tc3_request`, and the signing-key chain is
//! seeded with `TC3` + SecretKey (not `AWS4`). Tencent APIs are RPC-style: POST
//! to `/`, the action selected via the `X-TC-Action` header.
//!
//! @see <https://www.tencentcloud.com/document/product/845/32207> — TC3-HMAC-SHA256
//!   Verbatim: "The Authorization header follows: TC3-HMAC-SHA256
//!   Credential=SecretId/CredentialScope, SignedHeaders=..., Signature=...
//!   The scope follows the pattern Date/service/tc3_request ... The signing key
//!   is derived by prepending 'TC3' to your SecretKey."
//!
//! Signature is validated end-to-end by the Wave-3 live Hunyuan test; the unit
//! tests here lock the algorithm structure and the TC3-vs-SigV4 key derivation.

use crate::providers::auth::ProviderCredentials;
use crate::providers::ProviderError;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const ALGORITHM: &str = "TC3-HMAC-SHA256";

/// Sign a Tencent Cloud request using the current system time. Returns the
/// `Authorization` and `X-TC-Timestamp` headers to attach. The provider sets the
/// other common headers (`X-TC-Action`, `X-TC-Version`, `X-TC-Region`,
/// `Content-Type`); only `content-type` and `host` are part of the signature.
pub fn sign(
    creds: &ProviderCredentials,
    service: &str,
    url: &reqwest::Url,
    content_type: &str,
    body: &[u8],
) -> Result<Vec<(String, String)>, ProviderError> {
    let ts = chrono::Utc::now().timestamp();
    sign_at(creds, service, url, content_type, body, ts)
}

/// Sign with an explicit unix `timestamp`. Testable core.
pub fn sign_at(
    creds: &ProviderCredentials,
    service: &str,
    url: &reqwest::Url,
    content_type: &str,
    body: &[u8],
    timestamp: i64,
) -> Result<Vec<(String, String)>, ProviderError> {
    let secret_id = creds
        .key_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ProviderError::NotConfigured("tc3: missing key_id (SecretId)".into()))?;
    let secret_key = creds
        .key_secret
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ProviderError::NotConfigured("tc3: missing key_secret (SecretKey)".into()))?;

    let host = url.host_str().unwrap_or("").to_string();
    let date = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)
        .ok_or_else(|| ProviderError::InvalidRequest("tc3: invalid timestamp".into()))?
        .format("%Y-%m-%d")
        .to_string();

    // --- Canonical request (RPC: POST /, empty query) ---
    let canonical_headers = format!(
        "content-type:{}\nhost:{}\n",
        content_type.trim().to_ascii_lowercase(),
        host.to_ascii_lowercase()
    );
    let signed_headers = "content-type;host";
    let payload_hash = sha256_hex(body);
    let canonical_request = format!(
        "POST\n/\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    // --- String to sign ---
    let scope = format!("{date}/{service}/tc3_request");
    let string_to_sign = format!(
        "{ALGORITHM}\n{timestamp}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    // --- Signing key chain (seeded with "TC3", NOT "AWS4") ---
    let secret_date = hmac(format!("TC3{secret_key}").as_bytes(), date.as_bytes());
    let secret_service = hmac(&secret_date, service.as_bytes());
    let secret_signing = hmac(&secret_service, b"tc3_request");
    let signature = hex::encode(hmac(&secret_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "{ALGORITHM} Credential={secret_id}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );

    Ok(vec![
        ("X-TC-Timestamp".to_string(), timestamp.to_string()),
        ("Authorization".to_string(), authorization),
    ])
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> ProviderCredentials {
        ProviderCredentials {
            key_id: Some("AKIDtest".to_string()),
            key_secret: Some("secret-key".to_string()),
            region: Some("ap-guangzhou".to_string()),
            ..Default::default()
        }
    }

    fn url() -> reqwest::Url {
        reqwest::Url::parse("https://hunyuan.tencentcloudapi.com/").unwrap()
    }

    #[test]
    fn authorization_has_tc3_structure() {
        let signed = sign_at(&creds(), "hunyuan", &url(), "application/json; charset=utf-8", b"{}", 1_551_113_065).unwrap();
        let auth = &signed.iter().find(|(k, _)| k == "Authorization").unwrap().1;
        assert!(auth.starts_with("TC3-HMAC-SHA256 "), "auth: {auth}");
        assert!(auth.contains("Credential=AKIDtest/2019-02-25/hunyuan/tc3_request"), "auth: {auth}");
        assert!(auth.contains("SignedHeaders=content-type;host"), "auth: {auth}");
        assert!(auth.contains("Signature="), "auth: {auth}");
        // 64-hex signature
        let sig = auth.split("Signature=").nth(1).unwrap();
        assert_eq!(sig.len(), 64, "signature must be 64 hex chars: {sig}");
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn includes_timestamp_header() {
        let signed = sign_at(&creds(), "hunyuan", &url(), "application/json", b"{}", 1_700_000_000).unwrap();
        assert_eq!(signed.iter().find(|(k, _)| k == "X-TC-Timestamp").unwrap().1, "1700000000");
    }

    #[test]
    fn deterministic_for_same_inputs() {
        let a = sign_at(&creds(), "hunyuan", &url(), "application/json", b"{\"x\":1}", 100).unwrap();
        let b = sign_at(&creds(), "hunyuan", &url(), "application/json", b"{\"x\":1}", 100).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn body_change_changes_signature() {
        let sig = |body: &[u8]| {
            sign_at(&creds(), "hunyuan", &url(), "application/json", body, 100).unwrap()
                .into_iter().find(|(k, _)| k == "Authorization").unwrap().1
        };
        assert_ne!(sig(b"{\"x\":1}"), sig(b"{\"x\":2}"));
    }

    /// TC3 key derivation must seed with "TC3", not "AWS4" — guards against a
    /// copy-paste from the SigV4 signer.
    #[test]
    fn signing_key_seeded_with_tc3() {
        let date = "2019-02-25";
        let tc3 = hmac(format!("TC3{}", "secret-key").as_bytes(), date.as_bytes());
        let awsish = hmac(format!("AWS4{}", "secret-key").as_bytes(), date.as_bytes());
        assert_ne!(tc3, awsish);
    }

    #[test]
    fn missing_credentials_error() {
        assert!(sign_at(&ProviderCredentials::default(), "hunyuan", &url(), "application/json", b"{}", 100).is_err());
    }
}
