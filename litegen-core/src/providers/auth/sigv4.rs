//! AWS Signature Version 4 request signing (Amazon Bedrock).
//!
//! Implemented directly on `hmac` + `sha2` (no AWS SDK) to keep the binary lean.
//! Signs the finalized request (method, canonical URI, canonical query,
//! canonical headers, payload hash) and returns the `Authorization` +
//! `X-Amz-Date` headers to attach. `Host` is set by reqwest from the URL and is
//! always included in the signed headers.
//!
//! @see <https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_InvokeModel.html>
//!   — `POST /model/{modelId}/invoke`
//! @see <https://docs.aws.amazon.com/IAM/latest/UserGuide/create-signed-request.html>
//!   — SigV4 canonical request + string-to-sign + signing-key derivation.

use crate::providers::auth::ProviderCredentials;
use crate::providers::ProviderError;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// Sign a request using the current system time. Returns headers to attach.
pub fn sign(
    creds: &ProviderCredentials,
    service: &str,
    region: &str,
    method: &str,
    url: &reqwest::Url,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<Vec<(String, String)>, ProviderError> {
    let amz_date = now_amz_date();
    sign_at(creds, service, region, method, url, headers, body, &amz_date)
}

/// Sign with an explicit `amz_date` ("YYYYMMDDTHHMMSSZ"). Testable core.
#[allow(clippy::too_many_arguments)]
pub fn sign_at(
    creds: &ProviderCredentials,
    service: &str,
    region: &str,
    method: &str,
    url: &reqwest::Url,
    headers: &[(String, String)],
    body: &[u8],
    amz_date: &str,
) -> Result<Vec<(String, String)>, ProviderError> {
    let key_id = creds
        .key_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ProviderError::NotConfigured("sigv4: missing key_id".into()))?;
    let secret = creds
        .key_secret
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ProviderError::NotConfigured("sigv4: missing key_secret".into()))?;
    let date_stamp = &amz_date[..8];

    // --- Canonical headers: provided + host + x-amz-date, lowercased & sorted ---
    let host = canonical_host(url);
    let mut canon: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    canon.push(("host".to_string(), host));
    canon.push(("x-amz-date".to_string(), amz_date.to_string()));
    canon.sort_by(|a, b| a.0.cmp(&b.0));
    canon.dedup_by(|a, b| a.0 == b.0);

    let canonical_headers: String = canon.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = canon.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>().join(";");

    // --- Canonical request ---
    let canonical_uri = canonical_uri(url.path());
    let canonical_query = canonical_query(url.query().unwrap_or(""));
    let payload_hash = sha256_hex(body);
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    // --- String to sign ---
    let scope = format!("{date_stamp}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "{ALGORITHM}\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    // --- Signing key chain ---
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), date_stamp.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "{ALGORITHM} Credential={key_id}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );

    Ok(vec![
        ("x-amz-date".to_string(), amz_date.to_string()),
        ("Authorization".to_string(), authorization),
    ])
}

fn canonical_host(url: &reqwest::Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        (None, _) => String::new(),
    }
}

/// Canonical URI: URI-encode the path (each segment), preserving `/`. Empty → `/`.
fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    uri_encode(path, false)
}

/// Canonical query string: encode each key/value, sort by encoded key.
fn canonical_query(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let mut it = kv.splitn(2, '=');
            let k = uri_encode(it.next().unwrap_or(""), true);
            let v = uri_encode(it.next().unwrap_or(""), true);
            (k, v)
        })
        .collect();
    pairs.sort();
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// RFC 3986 URI encoding. Unreserved (A-Za-z0-9-._~) pass through; `/` passes
/// through unless `encode_slash`. Everything else is percent-encoded uppercase.
fn uri_encode(s: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b'/' if !encode_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
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

fn now_amz_date() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_creds() -> ProviderCredentials {
        ProviderCredentials {
            key_id: Some("AKIDEXAMPLE".to_string()),
            key_secret: Some("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string()),
            region: Some("us-east-1".to_string()),
            ..Default::default()
        }
    }

    /// AWS's canonical worked example (GET iam ListUsers, 20150830T123600Z).
    /// Documented expected signature; proves the full canonicalization chain.
    #[test]
    fn matches_aws_canonical_iam_example() {
        let url = reqwest::Url::parse(
            "https://iam.amazonaws.com/?Action=ListUsers&Version=2010-05-08",
        )
        .unwrap();
        let headers = vec![(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded; charset=utf-8".to_string(),
        )];
        let signed = sign_at(
            &example_creds(),
            "iam",
            "us-east-1",
            "GET",
            &url,
            &headers,
            b"",
            "20150830T123600Z",
        )
        .unwrap();

        let auth = &signed.iter().find(|(k, _)| k == "Authorization").unwrap().1;
        assert!(
            auth.contains("Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request"),
            "auth: {auth}"
        );
        assert!(auth.contains("SignedHeaders=content-type;host;x-amz-date"), "auth: {auth}");
        assert!(
            auth.contains("Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"),
            "auth: {auth}"
        );
    }

    #[test]
    fn includes_amz_date_header() {
        let url = reqwest::Url::parse("https://bedrock-runtime.us-east-1.amazonaws.com/model/x/invoke").unwrap();
        let signed = sign_at(&example_creds(), "bedrock", "us-east-1", "POST", &url, &[], b"{}", "20240101T000000Z").unwrap();
        assert_eq!(signed.iter().find(|(k, _)| k == "x-amz-date").unwrap().1, "20240101T000000Z");
    }

    #[test]
    fn missing_credentials_error() {
        let url = reqwest::Url::parse("https://x.amazonaws.com/").unwrap();
        assert!(sign_at(&ProviderCredentials::default(), "bedrock", "us-east-1", "POST", &url, &[], b"", "20240101T000000Z").is_err());
    }

    #[test]
    fn uri_encode_preserves_unreserved_and_slash() {
        assert_eq!(uri_encode("/model/amazon.nova-canvas-v1:0/invoke", false), "/model/amazon.nova-canvas-v1%3A0/invoke");
        assert_eq!(uri_encode("a b", true), "a%20b");
    }
}
