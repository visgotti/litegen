//! Reaching an app's bucket through a cross-account IAM role
//! (docs/storage-access.md).
//!
//! AWS's pattern for a third party: the customer's role trusts litegen's
//! account on condition of an external ID that litegen assigns, one per app,
//! so no tenant can name another's role and have litegen assume it (the
//! "confused deputy"). litegen calls STS `AssumeRole` with its own identity,
//! signs with Signature Version 4, and caches the session it gets until five
//! minutes before it ends.

use std::collections::HashMap;
use std::sync::Mutex;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::config::AwsConfig;

type HmacSha256 = Hmac<Sha256>;

/// How long an assumed session lasts: an hour, the most a role allows by default.
pub const SESSION_SECONDS: u32 = 3600;

/// A cached session is assumed again this long before it ends.
const REFRESH_MARGIN: chrono::Duration = chrono::Duration::minutes(5);

/// Temporary credentials for an assumed role.
#[derive(Debug, Clone, PartialEq)]
pub struct TempCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub expiration: chrono::DateTime<chrono::Utc>,
}

/// STS refused, or could not be reached.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StsError {
    #[error("{0}")]
    Refused(String),
    #[error("STS could not be reached: {0}")]
    Unreachable(String),
    #[error("litegen has no AWS identity to assume the role with: {0}")]
    NoIdentity(String),
}

/// The external ID of `app_id`'s role: `litegen-` and 32 hex characters of an
/// HMAC under the secrets key, so it is stable, needs no storage, and no
/// tenant can work out another's.
pub fn external_id(secrets_key: &[u8; 32], app_id: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secrets_key).expect("HMAC takes a key of any length");
    mac.update(b"aws-external-id:");
    mac.update(app_id.as_bytes());
    let digest = hex::encode(mac.finalize().into_bytes());
    format!("litegen-{}", &digest[..32])
}

/// Whether `arn` names an IAM role: `arn:aws[-partition]:iam::{12 digits}:role/{path and name}`.
pub fn is_role_arn(arn: &str) -> bool {
    let parts: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account, resource] = parts.as_slice() else { return false };
    let name = resource.strip_prefix("role/").unwrap_or("");
    *prefix == "arn"
        && (*partition == "aws" || partition.starts_with("aws-"))
        && *service == "iam"
        && region.is_empty()
        && account.len() == 12
        && account.bytes().all(|b| b.is_ascii_digit())
        && !name.is_empty()
        && name.len() <= 512
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"+=,.@_-/".contains(&b))
}

/// The session name litegen gives a role it assumes for `app_id`: at most 64
/// of STS's allowed characters.
pub fn session_name(app_id: &str) -> String {
    let clean: String = format!("litegen-{app_id}")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "+=,.@-_".contains(c) { c } else { '-' })
        .collect();
    clean.chars().take(64).collect()
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// SigV4's canonical request. `headers` are lowercase names and their
/// values; they are sorted here.
fn canonical_request(method: &str, path: &str, query: &str, headers: &[(String, String)], payload_hash: &str) -> String {
    let mut headers = headers.to_vec();
    headers.sort();
    let canonical: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
    let signed: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
    format!("{method}\n{path}\n{query}\n{canonical}\n{}\n{payload_hash}", signed.join(";"))
}

/// The `Authorization` header for a canonical request signed at `amz_date`
/// (`YYYYMMDDTHHMMSSZ`).
fn authorization(access_key: &str, secret: &str, amz_date: &str, region: &str, service: &str, headers: &[(String, String)], canonical: &str) -> String {
    let date = &amz_date[..8];
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", sha256_hex(canonical.as_bytes()));
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));
    let mut names: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
    names.sort_unstable();
    format!("AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={}, Signature={signature}", names.join(";"))
}

/// RFC 3986 percent-encoding, as SigV4 wants it: every byte but the unreserved.
fn uri_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// litegen's own credentials: the `aws` config's, else the standard chain.
async fn identity(cfg: &AwsConfig) -> Result<(String, String, Option<String>), StsError> {
    if let (Some(key), Some(secret)) = (cfg.access_key_id.as_deref(), cfg.secret_access_key.as_deref()) {
        return Ok((key.to_string(), secret.to_string(), cfg.session_token.clone()));
    }
    // The chain may ask instance metadata, which blocks.
    let creds = tokio::task::spawn_blocking(|| s3::creds::Credentials::new(None, None, None, None, None))
        .await
        .map_err(|e| StsError::NoIdentity(e.to_string()))?
        .map_err(|e| StsError::NoIdentity(e.to_string()))?;
    match (creds.access_key, creds.secret_key) {
        (Some(key), Some(secret)) => Ok((key, secret, creds.session_token.or(creds.security_token))),
        _ => Err(StsError::NoIdentity("the AWS credential chain found nothing".into())),
    }
}

/// The text of the first `<tag>` in `xml`, its five XML entities undone.
fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = start + xml[start..].find(&format!("</{tag}>"))?;
    Some(
        xml[start..end]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

/// POST one STS action, signed as litegen. Answers the response body.
async fn call_sts(cfg: &AwsConfig, params: &[(&str, &str)]) -> Result<String, StsError> {
    let region = cfg.region.clone().unwrap_or_else(|| "us-east-1".into());
    let endpoint = cfg.sts_endpoint.clone().unwrap_or_else(|| format!("https://sts.{region}.amazonaws.com"));
    let url = reqwest::Url::parse(&endpoint).map_err(|e| StsError::Unreachable(format!("bad STS endpoint {endpoint}: {e}")))?;
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or("")),
        None => url.host_str().unwrap_or("").to_string(),
    };
    let (key, secret, token) = identity(cfg).await?;
    let mut sorted = params.to_vec();
    sorted.sort();
    let body: String = sorted.iter().map(|(k, v)| format!("{k}={}", uri_encode(v))).collect::<Vec<_>>().join("&");
    let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut headers = vec![
        ("content-type".to_string(), "application/x-www-form-urlencoded; charset=utf-8".to_string()),
        ("host".to_string(), host),
        ("x-amz-date".to_string(), amz_date.clone()),
    ];
    if let Some(t) = token.as_deref() {
        headers.push(("x-amz-security-token".to_string(), t.to_string()));
    }
    let path = if url.path().is_empty() { "/" } else { url.path() };
    let canonical = canonical_request("POST", path, "", &headers, &sha256_hex(body.as_bytes()));
    let auth = authorization(&key, &secret, &amz_date, &region, "sts", &headers, &canonical);
    static CLIENT: once_cell::sync::Lazy<reqwest::Client> = once_cell::sync::Lazy::new(|| {
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().expect("an STS client")
    });
    let mut req = CLIENT.post(url).header("authorization", auth).body(body);
    for (k, v) in headers.iter().filter(|(k, _)| k != "host") {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| StsError::Unreachable(e.without_url().to_string()))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| StsError::Unreachable(e.to_string()))?;
    if !status.is_success() {
        let message = xml_text(&text, "Message").unwrap_or_else(|| format!("STS answered HTTP {status}"));
        let code = xml_text(&text, "Code").unwrap_or_default();
        return Err(StsError::Refused(if code.is_empty() { message } else { format!("{code}: {message}") }));
    }
    Ok(text)
}

/// Sessions by (STS endpoint, role ARN, external id).
type SessionCache = Mutex<HashMap<(String, String, String), TempCredentials>>;

fn session_cache() -> &'static SessionCache {
    static CACHE: std::sync::OnceLock<SessionCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A session of `role_arn` under `external_id`: the cached one while it has
/// more than five minutes left, else a new `AssumeRole`.
pub async fn assumed_credentials(cfg: &AwsConfig, role_arn: &str, external_id: &str, session: &str) -> Result<TempCredentials, StsError> {
    let cache_key = (cfg.sts_endpoint.clone().unwrap_or_default(), role_arn.to_string(), external_id.to_string());
    if let Some(c) = session_cache().lock().unwrap().get(&cache_key) {
        if c.expiration - chrono::Utc::now() > REFRESH_MARGIN {
            return Ok(c.clone());
        }
    }
    let duration = SESSION_SECONDS.to_string();
    let body = call_sts(cfg, &[
        ("Action", "AssumeRole"),
        ("DurationSeconds", &duration),
        ("ExternalId", external_id),
        ("RoleArn", role_arn),
        ("RoleSessionName", session),
        ("Version", "2011-06-15"),
    ])
    .await?;
    let field = |tag: &str| xml_text(&body, tag).ok_or_else(|| StsError::Refused(format!("STS's answer has no {tag}")));
    let creds = TempCredentials {
        access_key_id: field("AccessKeyId")?,
        secret_access_key: field("SecretAccessKey")?,
        session_token: field("SessionToken")?,
        expiration: chrono::DateTime::parse_from_rfc3339(&field("Expiration")?)
            .map_err(|e| StsError::Refused(format!("STS's Expiration does not parse: {e}")))?
            .with_timezone(&chrono::Utc),
    };
    session_cache().lock().unwrap().insert(cache_key, creds.clone());
    Ok(creds)
}

/// How long a failed identity lookup is remembered: with STS unreachable,
/// each storage read would otherwise wait out its timeout again.
const FAILED_LOOKUP_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// litegen's own principal, which a role's trust policy names: STS
/// `GetCallerIdentity`, cached once found, and a failure for a minute.
pub async fn caller_arn(cfg: &AwsConfig) -> Result<String, StsError> {
    static FOUND: std::sync::OnceLock<Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    static FAILED: std::sync::OnceLock<Mutex<HashMap<String, (std::time::Instant, StsError)>>> = std::sync::OnceLock::new();
    let found = FOUND.get_or_init(|| Mutex::new(HashMap::new()));
    let failed = FAILED.get_or_init(|| Mutex::new(HashMap::new()));
    let key = format!("{}|{}", cfg.sts_endpoint.clone().unwrap_or_default(), cfg.access_key_id.clone().unwrap_or_default());
    if let Some(arn) = found.lock().unwrap().get(&key) {
        return Ok(arn.clone());
    }
    if let Some((at, e)) = failed.lock().unwrap().get(&key) {
        if at.elapsed() < FAILED_LOOKUP_TTL {
            return Err(e.clone());
        }
    }
    let lookup = async {
        let body = call_sts(cfg, &[("Action", "GetCallerIdentity"), ("Version", "2011-06-15")]).await?;
        xml_text(&body, "Arn").ok_or_else(|| StsError::Refused("STS's answer has no Arn".into()))
    };
    match lookup.await {
        Ok(arn) => {
            failed.lock().unwrap().remove(&key);
            found.lock().unwrap().insert(key, arn.clone());
            Ok(arn)
        }
        Err(e) => {
            failed.lock().unwrap().insert(key, (std::time::Instant::now(), e.clone()));
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigv4_matches_the_aws_example() {
        // AWS's own worked example (IAM ListUsers, AKIDEXAMPLE, 2015-08-30).
        let headers = vec![
            ("content-type".to_string(), "application/x-www-form-urlencoded; charset=utf-8".to_string()),
            ("host".to_string(), "iam.amazonaws.com".to_string()),
            ("x-amz-date".to_string(), "20150830T123600Z".to_string()),
        ];
        let canonical = canonical_request("GET", "/", "Action=ListUsers&Version=2010-05-08", &headers, &sha256_hex(b""));
        assert_eq!(sha256_hex(canonical.as_bytes()), "f536975d06c0309214f805bb90ccff089219ecd68b2577efef23edd43b7e1a59");
        let auth = authorization("AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "20150830T123600Z", "us-east-1", "iam", &headers, &canonical);
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date, \
             Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }

    #[test]
    fn the_external_id_is_stable_per_app_and_differs_between_apps() {
        let key = [7u8; 32];
        let a = external_id(&key, "app-a");
        assert_eq!(a, external_id(&key, "app-a"));
        assert_ne!(a, external_id(&key, "app-b"));
        assert_ne!(a, external_id(&[8u8; 32], "app-a"), "another gateway's secrets key");
        assert!(a.starts_with("litegen-") && a.len() == 40 && a[8..].bytes().all(|b| b.is_ascii_hexdigit()), "{a}");
    }

    #[test]
    fn only_iam_role_arns_are_taken() {
        for good in ["arn:aws:iam::123456789012:role/litegen-writer", "arn:aws-cn:iam::123456789012:role/path/to/r", "arn:aws-us-gov:iam::123456789012:role/r+=,.@_-"] {
            assert!(is_role_arn(good), "{good}");
        }
        for bad in ["arn:aws:s3:::bucket", "arn:aws:iam::12345:role/x", "arn:aws:iam::123456789012:user/x", "arn:aws:iam:us-east-1:123456789012:role/x", "arn:aws:iam::123456789012:role/", "role/x", ""] {
            assert!(!is_role_arn(bad), "{bad}");
        }
    }

    #[test]
    fn session_names_fit_sts() {
        assert_eq!(session_name("app 1/x"), "litegen-app-1-x");
        assert_eq!(session_name(&"a".repeat(100)).len(), 64);
    }

    #[test]
    fn xml_text_reads_a_tag_and_unescapes_it() {
        let xml = "<E><Code>AccessDenied</Code><Message>no &amp; never &lt;here&gt;</Message></E>";
        assert_eq!(xml_text(xml, "Code").as_deref(), Some("AccessDenied"));
        assert_eq!(xml_text(xml, "Message").as_deref(), Some("no & never <here>"));
        assert_eq!(xml_text(xml, "Missing"), None);
    }

    #[test]
    fn uri_encoding_keeps_only_the_unreserved() {
        assert_eq!(uri_encode("arn:aws:iam::1:role/a b~_.-"), "arn%3Aaws%3Aiam%3A%3A1%3Arole%2Fa%20b~_.-");
    }

    #[tokio::test]
    async fn a_failed_identity_lookup_is_remembered_for_a_minute() {
        use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
        let sts = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).mount(&sts).await;
        let cfg = AwsConfig {
            access_key_id: Some("AKIA-failed-lookup-test".into()),
            secret_access_key: Some("secret".into()),
            sts_endpoint: Some(sts.uri()),
            ..Default::default()
        };
        let first = caller_arn(&cfg).await.unwrap_err();
        let again = caller_arn(&cfg).await.unwrap_err();
        assert_eq!(first, again);
        assert_eq!(sts.received_requests().await.unwrap().len(), 1, "the second read answered from the failure it remembered");
    }
}
