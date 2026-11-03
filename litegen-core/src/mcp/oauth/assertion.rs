//! `private_key_jwt` client authentication (OpenID Connect Core §9, RFC 7523
//! §2.2). A client whose metadata document declares it is confidential
//! (Client ID Metadata Document §6.2): it proves who it is at the token
//! endpoint with a short-lived JWT signed by a key it publishes, inline as
//! `jwks` or at `jwks_uri`. ChatGPT's document declares it. LiteGen checks
//! the signature, that the client is the JWT's issuer and subject, that
//! LiteGen is its audience, that it is current, and that its `jti` was not
//! presented before.

use std::collections::HashMap;
use std::sync::Mutex;

use aws_lc_rs::signature::{self, RsaPublicKeyComponents, UnparsedPublicKey};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Map, Value};

use super::clients::MetadataFetcher;
use super::store::{McpOAuthStore, OAuthClient};

/// The `client_assertion_type` of a JWT client assertion.
pub const ASSERTION_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";
/// The JWS algorithms LiteGen verifies.
pub const SIGNING_ALGS: [&str; 4] = ["RS256", "PS256", "ES256", "EdDSA"];
/// How far apart LiteGen's clock and the client's may be.
const LEEWAY: Duration = Duration::seconds(60);
/// How long a key set fetched from a `jwks_uri` is used.
const KEYS_TTL: Duration = Duration::minutes(10);
/// A key set is fetched again for a key it lacks at most this often.
const KEYS_REFETCH: Duration = Duration::seconds(30);
const MAX_JTI_CHARS: usize = 256;

/// Why an assertion was refused.
#[derive(Debug)]
pub enum AssertionError {
    Invalid(String),
    Store(sqlx::Error),
}

impl From<sqlx::Error> for AssertionError {
    fn from(e: sqlx::Error) -> Self {
        AssertionError::Store(e)
    }
}

fn invalid(message: impl Into<String>) -> AssertionError {
    AssertionError::Invalid(message.into())
}

/// Whether `client` registered as confidential, with `private_key_jwt`.
pub fn required(client: &OAuthClient) -> bool {
    client.metadata.get("token_endpoint_auth_method").and_then(Value::as_str) == Some("private_key_jwt")
}

/// The client an assertion says it comes from (`sub`), unchecked: a token
/// request may name its client this way alone.
pub fn claimed_client(assertion: &str) -> Option<String> {
    Jwt::parse(assertion).ok()?.claims.get("sub")?.as_str().map(str::to_string)
}

/// Key sets fetched from clients' `jwks_uri`s.
#[derive(Default)]
pub struct KeyCache(Mutex<HashMap<String, (DateTime<Utc>, Vec<Value>)>>);

/// Check `assertion` as `client`'s authentication. `audiences` are the
/// values `aud` may hold: LiteGen's issuer and its token endpoint.
pub async fn verify(
    assertion: &str,
    client: &OAuthClient,
    audiences: &[String],
    cache: &KeyCache,
    fetcher: &dyn MetadataFetcher,
    store: &dyn McpOAuthStore,
    now: DateTime<Utc>,
) -> Result<(), AssertionError> {
    let jwt = Jwt::parse(assertion)?;
    let alg = jwt.header.get("alg").and_then(Value::as_str).unwrap_or_default();
    if !SIGNING_ALGS.contains(&alg) {
        return Err(invalid(format!("the client assertion is signed with '{alg}'; LiteGen accepts {}", SIGNING_ALGS.join(", "))));
    }
    if jwt.header.contains_key("crit") {
        return Err(invalid("the client assertion has critical header parameters LiteGen does not support"));
    }
    let expires_at = check_claims(&jwt.claims, &client.client_id, audiences, now)?;

    let kid = jwt.header.get("kid").and_then(Value::as_str);
    if !jwt.signed_by(alg, kid, &keys(client, cache, fetcher, now, false).await?) {
        // The client may have rotated its key since LiteGen fetched its set.
        let refetched = client.metadata.get("jwks").is_none() && jwt.signed_by(alg, kid, &keys(client, cache, fetcher, now, true).await?);
        if !refetched {
            return Err(invalid("the client assertion's signature does not match the client's published keys"));
        }
    }

    match jwt.claims.get("jti") {
        None => {}
        Some(Value::String(jti)) if !jti.is_empty() && jti.chars().count() <= MAX_JTI_CHARS => {
            if !store.record_assertion(&client.client_id, jti, expires_at + LEEWAY).await? {
                return Err(invalid("the client assertion was used before"));
            }
        }
        Some(_) => return Err(invalid(format!("the client assertion's jti must be a string of 1 to {MAX_JTI_CHARS} characters"))),
    }
    Ok(())
}

/// The claims' expiry, once they show the assertion is from and about the
/// client, for LiteGen, and current.
fn check_claims(claims: &Map<String, Value>, client_id: &str, audiences: &[String], now: DateTime<Utc>) -> Result<DateTime<Utc>, AssertionError> {
    for claim in ["iss", "sub"] {
        if claims.get(claim).and_then(Value::as_str) != Some(client_id) {
            return Err(invalid(format!("the client assertion's {claim} must be the client_id")));
        }
    }
    let for_litegen = |aud: &Value| aud.as_str().is_some_and(|aud| audiences.iter().any(|a| a == aud));
    let addressed = match claims.get("aud") {
        Some(Value::Array(list)) => list.iter().any(for_litegen),
        Some(aud) => for_litegen(aud),
        None => false,
    };
    if !addressed {
        return Err(invalid(format!("the client assertion's aud must be LiteGen's issuer ({}) or its token endpoint", audiences[0])));
    }
    let expires_at = numeric_date(claims, "exp").ok_or_else(|| invalid("the client assertion needs an exp"))?;
    if now > expires_at + LEEWAY {
        return Err(invalid("the client assertion expired"));
    }
    if numeric_date(claims, "nbf").is_some_and(|nbf| now + LEEWAY < nbf) {
        return Err(invalid("the client assertion is not valid yet"));
    }
    Ok(expires_at)
}

/// A JWT NumericDate claim: seconds since the epoch.
fn numeric_date(claims: &Map<String, Value>, claim: &str) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(claims.get(claim)?.as_f64()?.floor() as i64, 0)
}

/// The client's public keys: inline, or from its `jwks_uri`, kept for ten
/// minutes. `refetch` asks for a fresh copy, which LiteGen fetches only if
/// its copy is half a minute old.
async fn keys(client: &OAuthClient, cache: &KeyCache, fetcher: &dyn MetadataFetcher, now: DateTime<Utc>, refetch: bool) -> Result<Vec<Value>, AssertionError> {
    if let Some(jwks) = client.metadata.get("jwks") {
        return Ok(key_list(jwks));
    }
    let Some(uri) = client.metadata.get("jwks_uri").and_then(Value::as_str) else {
        return Err(invalid("the client declares private_key_jwt but publishes no keys (jwks or jwks_uri)"));
    };
    let cached = cache.0.lock().unwrap_or_else(|e| e.into_inner()).get(uri).cloned();
    if let Some((fetched_at, keys)) = cached {
        let age = now - fetched_at;
        if age < KEYS_TTL && !(refetch && age >= KEYS_REFETCH) {
            return Ok(keys);
        }
    }
    let bytes = fetcher.fetch(uri).await.map_err(|e| invalid(format!("the client's keys at {uri} could not be fetched: {e}")))?;
    let keys = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .map(|set| key_list(&set))
        .filter(|keys| !keys.is_empty())
        .ok_or_else(|| invalid(format!("the client's keys at {uri} are not a JWK set")))?;
    let mut cache = cache.0.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, (fetched_at, _)| now - *fetched_at < KEYS_TTL);
    cache.insert(uri.to_string(), (now, keys.clone()));
    Ok(keys)
}

/// A JWK set's keys.
fn key_list(set: &Value) -> Vec<Value> {
    set.get("keys").and_then(Value::as_array).map(|keys| keys.iter().filter(|k| k.is_object()).cloned().collect()).unwrap_or_default()
}

/// A compact JWS, split and decoded.
struct Jwt {
    header: Map<String, Value>,
    claims: Map<String, Value>,
    signing_input: String,
    signature: Vec<u8>,
}

impl Jwt {
    fn parse(token: &str) -> Result<Jwt, AssertionError> {
        let not_a_jwt = || invalid("the client assertion is not a signed JWT");
        let mut parts = token.split('.');
        let (Some(header), Some(claims), Some(signature), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
            return Err(not_a_jwt());
        };
        let object = |part: &str| match serde_json::from_slice::<Value>(&URL_SAFE_NO_PAD.decode(part).ok()?).ok()? {
            Value::Object(map) => Some(map),
            _ => None,
        };
        Ok(Jwt {
            header: object(header).ok_or_else(not_a_jwt)?,
            claims: object(claims).ok_or_else(not_a_jwt)?,
            signing_input: format!("{header}.{claims}"),
            signature: URL_SAFE_NO_PAD.decode(signature).map_err(|_| not_a_jwt())?,
        })
    }

    /// Whether one of `keys` (the one named `kid`, if given) signed this.
    fn signed_by(&self, alg: &str, kid: Option<&str>, keys: &[Value]) -> bool {
        let text = |key: &Value, name: &str| key.get(name).and_then(Value::as_str).map(str::to_string);
        keys.iter()
            .filter(|key| kid.is_none_or(|kid| text(key, "kid").as_deref() == Some(kid)))
            .filter(|key| text(key, "use").is_none_or(|u| u == "sig"))
            .filter(|key| text(key, "alg").is_none_or(|a| a == alg))
            .any(|key| verify_signature(key, alg, self.signing_input.as_bytes(), &self.signature))
    }
}

/// Whether `signature` over `message` verifies with the JWK `key` under `alg`.
fn verify_signature(key: &Value, alg: &str, message: &[u8], signature: &[u8]) -> bool {
    let bytes = |name: &str| key.get(name).and_then(Value::as_str).and_then(|v| URL_SAFE_NO_PAD.decode(v).ok());
    let kty = key.get("kty").and_then(Value::as_str);
    let crv = key.get("crv").and_then(Value::as_str);
    match (alg, kty) {
        ("RS256" | "PS256", Some("RSA")) => {
            let (Some(n), Some(e)) = (bytes("n"), bytes("e")) else { return false };
            let unpadded = |b: &[u8]| b.iter().position(|&x| x != 0).map_or_else(Vec::new, |i| b[i..].to_vec());
            let params = if alg == "RS256" { &signature::RSA_PKCS1_2048_8192_SHA256 } else { &signature::RSA_PSS_2048_8192_SHA256 };
            RsaPublicKeyComponents { n: unpadded(&n), e: unpadded(&e) }.verify(params, message, signature).is_ok()
        }
        ("ES256", Some("EC")) if crv == Some("P-256") => {
            let (Some(x), Some(y)) = (bytes("x"), bytes("y")) else { return false };
            if x.len() != 32 || y.len() != 32 {
                return false;
            }
            let point = [&[4u8][..], &x, &y].concat();
            UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, point).verify(message, signature).is_ok()
        }
        ("EdDSA", Some("OKP")) if crv == Some("Ed25519") => {
            let Some(x) = bytes("x") else { return false };
            UnparsedPublicKey::new(&signature::ED25519, x).verify(message, signature).is_ok()
        }
        _ => false,
    }
}
