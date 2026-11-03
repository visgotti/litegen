//! Unit tests for the OAuth authorization server's rules.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use aws_lc_rs::signature::{self as sig, KeyPair as _};
use base64::Engine as _;
use chrono::Utc;
use serde_json::{json, Value};

use super::assertion::{verify, AssertionError, KeyCache};
use super::clients::{redirect_allowed, resolve_client, uses_refresh_tokens, ClientError, MetadataFetcher, PublicFetcher};
use super::limit::{client_address, AddressLimiter};
use super::store::{McpOAuthStore, OAuthClient, Registration};
use crate::db::DatabaseStore;

// ─── Redirect URIs ────────────────────────────────────────────────────────────

#[test]
fn redirect_rules() {
    let loopback = vec!["http://127.0.0.1/callback".to_string()];
    assert!(redirect_allowed(&loopback, "http://127.0.0.1:53111/callback"), "loopback: any port");
    assert!(redirect_allowed(&loopback, "http://127.0.0.1/callback"));
    assert!(!redirect_allowed(&loopback, "http://127.0.0.1:1/other"), "but the same path");
    assert!(!redirect_allowed(&loopback, "http://127.0.0.1:1/callback?next=x"), "and the same query");
    assert!(!redirect_allowed(&loopback, "http://localhost:53111/callback"), "and the same host");
    assert!(!redirect_allowed(&loopback, "https://127.0.0.1:53111/callback"), "over http only");
    assert!(!redirect_allowed(&loopback, "http://evil.example/callback"));
    assert!(!redirect_allowed(&["http://evil.example/callback".to_string()], "http://evil.example:8080/callback"), "only loopback ignores the port");

    assert!(redirect_allowed(&["http://localhost/callback".to_string()], "http://localhost:8787/callback"));
    assert!(redirect_allowed(&["http://[::1]/cb".to_string()], "http://[::1]:9000/cb"));

    let claude = vec!["https://claude.ai/api/mcp/auth_callback".to_string()];
    assert!(redirect_allowed(&claude, "https://claude.ai/api/mcp/auth_callback"));
    assert!(!redirect_allowed(&claude, "https://claude.ai:8443/api/mcp/auth_callback"));
    assert!(!redirect_allowed(&claude, "https://claude.ai/api/mcp/auth_callback/"));
    assert!(!redirect_allowed(&claude, "https://claude.ai/api/mcp/auth_callback?x=1"));
    assert!(!redirect_allowed(&claude, "https://evil.example/api/mcp/auth_callback"));
    assert!(!redirect_allowed(&[], "http://127.0.0.1/callback"));
}

// ─── Client ID Metadata Documents ─────────────────────────────────────────────

/// Serves one document and counts the fetches.
struct Stub {
    body: String,
    calls: AtomicUsize,
}

impl Stub {
    fn new(doc: Value) -> Self {
        Stub { body: doc.to_string(), calls: AtomicUsize::new(0) }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl MetadataFetcher for Stub {
    async fn fetch(&self, _url: &str) -> Result<Vec<u8>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.body.clone().into_bytes())
    }
}

#[tokio::test]
async fn cimd_document_rules() {
    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    let store = db.mcp_oauth().unwrap();
    let now = Utc::now();
    let id = "https://app.example.org/oauth/client.json";

    let good = Stub::new(json!({
        "client_id": id,
        "client_name": "Example",
        "redirect_uris": ["https://app.example.org/callback"],
        "token_endpoint_auth_method": "none",
    }));
    let client = resolve_client(store, &good, id, now).await.unwrap();
    assert_eq!(client.registration, Registration::Cimd);
    assert_eq!(client.client_name, "Example");
    assert_eq!(client.redirect_uris, vec!["https://app.example.org/callback".to_string()]);

    resolve_client(store, &good, id, now + chrono::Duration::hours(23)).await.unwrap();
    assert_eq!(good.calls(), 1, "cached for a day");
    resolve_client(store, &good, id, now + chrono::Duration::hours(25)).await.unwrap();
    assert_eq!(good.calls(), 2, "fetched again after a day");

    let invalid = |doc: Value| async move { resolve_client(store, &Stub::new(doc), "https://other.example.org/client.json", now).await };
    let other = "https://other.example.org/client.json";
    assert!(
        matches!(invalid(json!({ "client_id": "https://evil.example/c.json", "redirect_uris": ["https://other.example.org/cb"] })).await, Err(ClientError::InvalidMetadata(_))),
        "client_id must be the document's own URL"
    );
    assert!(matches!(invalid(json!({ "client_id": other, "redirect_uris": [] })).await, Err(ClientError::InvalidMetadata(_))));
    assert!(matches!(invalid(json!({ "client_id": other })).await, Err(ClientError::InvalidMetadata(_))));
    assert!(matches!(invalid(json!({ "client_id": other, "redirect_uris": ["http://example.com/cb"] })).await, Err(ClientError::InvalidMetadata(_))), "https or loopback only");
    assert!(matches!(
        invalid(json!({ "client_id": other, "redirect_uris": ["https://other.example.org/cb"], "token_endpoint_auth_method": "client_secret_basic" })).await,
        Err(ClientError::InvalidMetadata(_))
    ));
    assert!(matches!(invalid(Value::String("not an object".into())).await, Err(ClientError::InvalidMetadata(_))));

    // A private_key_jwt client publishes the keys it signs with, at an https
    // jwks_uri or inline; the name defaults to the host.
    let cb = ["https://other.example.org/cb"];
    for keyless in [
        json!({ "client_id": other, "redirect_uris": cb, "token_endpoint_auth_method": "private_key_jwt" }),
        json!({ "client_id": other, "redirect_uris": cb, "token_endpoint_auth_method": "private_key_jwt", "jwks_uri": "http://other.example.org/jwks.json" }),
    ] {
        assert!(matches!(invalid(keyless.clone()).await, Err(ClientError::InvalidMetadata(_))), "{keyless}");
    }
    // Grant types, when listed, must include the authorization-code flow.
    assert!(matches!(invalid(json!({ "client_id": other, "redirect_uris": cb, "grant_types": ["client_credentials"] })).await, Err(ClientError::InvalidMetadata(_))));
    // (A valid document is kept for a day: this one last.)
    let keyed = invalid(json!({ "client_id": other, "redirect_uris": cb, "token_endpoint_auth_method": "private_key_jwt", "jwks_uri": "https://other.example.org/jwks.json" }))
        .await
        .unwrap();
    assert_eq!(keyed.client_name, "other.example.org");

    // Ids that are not documents are dynamic registrations, never fetched.
    let never = Stub::new(json!({}));
    for not_a_document in ["lgc_unknown", "http://insecure.example/client.json", "https://no-path.example", "https://no-path.example/"] {
        assert!(matches!(resolve_client(store, &never, not_a_document, now).await, Err(ClientError::Unknown(_))), "{not_a_document}");
    }
    assert_eq!(never.calls(), 0);
}

#[tokio::test]
async fn production_fetcher_refuses_private_addresses() {
    for url in ["https://127.0.0.1/x", "https://10.0.0.1/x", "https://169.254.169.254/x", "https://[::1]/x", "http://example.com/x"] {
        let started = Instant::now();
        assert!(PublicFetcher.fetch(url).await.is_err(), "{url}");
        assert!(started.elapsed() < Duration::from_secs(1), "{url} refused without a network call");
    }
}

// ─── The per-address limit ────────────────────────────────────────────────────

#[test]
fn the_address_a_request_counts_against() {
    use std::net::{IpAddr, SocketAddr};
    let peer = |ip: &str| Some(SocketAddr::new(ip.parse().unwrap(), 4000));
    let ip = |s: &str| s.parse::<IpAddr>().ok();
    // A visitor counts as itself, whatever its X-Forwarded-For says.
    assert_eq!(client_address(peer("203.0.113.5"), Some("198.51.100.7")), ip("203.0.113.5"));
    // Behind a proxy on this machine or a private network: the address the
    // proxy saw, which it put last.
    for proxy in ["127.0.0.1", "10.1.2.3", "172.18.0.4", "192.168.1.10", "::1", "fd00::5", "::ffff:172.18.0.4"] {
        assert_eq!(client_address(peer(proxy), Some("198.51.100.1, 203.0.113.5")), ip("203.0.113.5"), "{proxy}");
        assert_eq!(client_address(peer(proxy), Some("203.0.113.5:4711")), ip("203.0.113.5"), "{proxy}: with a port");
        assert!(client_address(peer(proxy), None).is_some(), "{proxy}: no header, the proxy itself");
        assert_eq!(client_address(peer(proxy), Some("unknown")), client_address(peer(proxy), None), "{proxy}: nothing usable");
    }
    // IPv6 counts by /64, which one subscriber holds whole; IPv4-mapped as IPv4.
    assert_eq!(client_address(peer("2001:db8:1:2:aaaa::1"), None), ip("2001:db8:1:2::"));
    assert_eq!(client_address(peer("2001:db8:1:2:bbbb::9"), None), ip("2001:db8:1:2::"));
    assert_eq!(client_address(peer("::ffff:203.0.113.5"), None), ip("203.0.113.5"));
    assert_eq!(client_address(None, Some("203.0.113.5")), None, "no connection, nothing to count");
}

#[test]
fn an_address_gets_its_allowance_back_over_a_minute() {
    let limiter = AddressLimiter::default();
    let (a, b) = ("203.0.113.5".parse().unwrap(), "203.0.113.6".parse().unwrap());
    let start = Instant::now();
    assert!(limiter.try_take(a, 2, start).is_ok());
    assert!(limiter.try_take(a, 2, start).is_ok());
    let wait = limiter.try_take(a, 2, start).unwrap_err();
    assert!((1..=30).contains(&wait), "two a minute: one back within 30 s, said {wait}");
    assert!(limiter.try_take(b, 2, start).is_ok(), "each address has its own");
    assert!(limiter.try_take(a, 2, start + Duration::from_secs(30)).is_ok(), "one back after half a minute");
    assert!(limiter.try_take(a, 2, start + Duration::from_secs(30)).is_err());
}

#[test]
fn refresh_tokens_follow_the_registered_grant_types() {
    let with = |metadata: Value| OAuthClient {
        client_id: "c".into(),
        registration: Registration::Cimd,
        client_name: "c".into(),
        redirect_uris: vec![],
        logo_uri: None,
        client_uri: None,
        metadata,
        created_at: Utc::now(),
        refreshed_at: Utc::now(),
        last_used_at: None,
    };
    assert!(uses_refresh_tokens(&with(json!({}))), "LiteGen's default: both grants");
    assert!(uses_refresh_tokens(&with(json!({ "grant_types": ["authorization_code", "refresh_token"] }))));
    assert!(!uses_refresh_tokens(&with(json!({ "grant_types": ["authorization_code"] }))));
}

// ─── private_key_jwt client assertions ────────────────────────────────────────

const SIGNED_CLIENT: &str = "https://client.example.com/oauth/client.json";
const ISSUER: &str = "https://app.example.com/api";

fn audiences() -> Vec<String> {
    vec![ISSUER.to_string(), format!("{ISSUER}/oauth/token")]
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A key a client signs assertions with, of each algorithm LiteGen takes.
enum TestKey {
    Rs256(sig::RsaKeyPair),
    Ps256(sig::RsaKeyPair),
    Es256(sig::EcdsaKeyPair),
    EdDsa(sig::Ed25519KeyPair),
}

impl TestKey {
    fn all() -> Vec<TestKey> {
        let rsa = || aws_lc_rs::rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap();
        vec![
            TestKey::Rs256(rsa()),
            TestKey::Ps256(rsa()),
            TestKey::es256(),
            TestKey::EdDsa(sig::Ed25519KeyPair::generate().unwrap()),
        ]
    }

    fn es256() -> TestKey {
        TestKey::Es256(sig::EcdsaKeyPair::generate(&sig::ECDSA_P256_SHA256_FIXED_SIGNING).unwrap())
    }

    fn alg(&self) -> &'static str {
        match self {
            TestKey::Rs256(_) => "RS256",
            TestKey::Ps256(_) => "PS256",
            TestKey::Es256(_) => "ES256",
            TestKey::EdDsa(_) => "EdDSA",
        }
    }

    /// The public key as a JWK with `kid`.
    fn jwk(&self, kid: &str) -> Value {
        match self {
            TestKey::Rs256(k) | TestKey::Ps256(k) => {
                let c = sig::RsaPublicKeyComponents::<Vec<u8>>::from(k.public_key());
                json!({ "kty": "RSA", "kid": kid, "n": b64(&c.n), "e": b64(&c.e) })
            }
            TestKey::Es256(k) => {
                let point = k.public_key().as_ref();
                json!({ "kty": "EC", "crv": "P-256", "kid": kid, "x": b64(&point[1..33]), "y": b64(&point[33..65]) })
            }
            TestKey::EdDsa(k) => json!({ "kty": "OKP", "crv": "Ed25519", "kid": kid, "x": b64(k.public_key().as_ref()) }),
        }
    }

    /// A compact JWS of `claims` under `header`.
    fn sign(&self, header: &Value, claims: &Value) -> String {
        let input = format!("{}.{}", b64(header.to_string().as_bytes()), b64(claims.to_string().as_bytes()));
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let signature = match self {
            TestKey::Rs256(k) | TestKey::Ps256(k) => {
                let padding: &'static dyn sig::RsaEncoding = if matches!(self, TestKey::Rs256(_)) { &sig::RSA_PKCS1_SHA256 } else { &sig::RSA_PSS_SHA256 };
                let mut out = vec![0; k.public_modulus_len()];
                k.sign(padding, &rng, input.as_bytes(), &mut out).unwrap();
                out
            }
            TestKey::Es256(k) => k.sign(&rng, input.as_bytes()).unwrap().as_ref().to_vec(),
            TestKey::EdDsa(k) => k.sign(input.as_bytes()).as_ref().to_vec(),
        };
        format!("{input}.{}", b64(&signature))
    }

    /// A valid assertion, as `kid`, with `claims` merged over the usual ones.
    fn assertion(&self, kid: &str, claims: Value) -> String {
        let now = Utc::now().timestamp();
        let mut all = json!({
            "iss": SIGNED_CLIENT,
            "sub": SIGNED_CLIENT,
            "aud": ISSUER,
            "iat": now,
            "exp": now + 300,
            "jti": uuid::Uuid::new_v4().to_string(),
        });
        for (k, v) in claims.as_object().unwrap() {
            if v.is_null() {
                all.as_object_mut().unwrap().remove(k);
            } else {
                all[k] = v.clone();
            }
        }
        self.sign(&json!({ "alg": self.alg(), "kid": kid, "typ": "JWT" }), &all)
    }
}

/// A confidential client whose metadata publishes `keys` inline.
fn signed_client(metadata: Value) -> OAuthClient {
    OAuthClient {
        client_id: SIGNED_CLIENT.into(),
        registration: Registration::Cimd,
        client_name: "Signed client".into(),
        redirect_uris: vec!["https://client.example.com/cb".into()],
        logo_uri: None,
        client_uri: None,
        metadata,
        created_at: Utc::now(),
        refreshed_at: Utc::now(),
        last_used_at: None,
    }
}

/// Serves a key set that can change, counting the fetches.
struct KeySet {
    body: std::sync::Mutex<String>,
    calls: AtomicUsize,
}

impl KeySet {
    fn new(keys: Value) -> Self {
        KeySet { body: std::sync::Mutex::new(keys.to_string()), calls: AtomicUsize::new(0) }
    }

    fn serve(&self, keys: Value) {
        *self.body.lock().unwrap() = keys.to_string();
    }
}

#[async_trait]
impl MetadataFetcher for KeySet {
    async fn fetch(&self, _url: &str) -> Result<Vec<u8>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.body.lock().unwrap().clone().into_bytes())
    }
}

async fn check(store: &dyn McpOAuthStore, client: &OAuthClient, assertion: &str) -> Result<(), String> {
    check_at(store, client, assertion, &KeySet::new(json!({})), &KeyCache::default(), Utc::now()).await
}

async fn check_at(
    store: &dyn McpOAuthStore,
    client: &OAuthClient,
    assertion: &str,
    fetcher: &dyn MetadataFetcher,
    cache: &KeyCache,
    now: chrono::DateTime<Utc>,
) -> Result<(), String> {
    match verify(assertion, client, &audiences(), cache, fetcher, store, now).await {
        Ok(()) => Ok(()),
        Err(AssertionError::Invalid(e)) => Err(e),
        Err(AssertionError::Store(e)) => panic!("store: {e}"),
    }
}

#[tokio::test]
async fn assertions_verify_with_each_algorithm() {
    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    let store = db.mcp_oauth().unwrap();
    for key in TestKey::all() {
        let client = signed_client(json!({ "jwks": { "keys": [key.jwk("k1")] } }));
        assert_eq!(check(store, &client, &key.assertion("k1", json!({}))).await, Ok(()), "{}", key.alg());
    }
}

#[tokio::test]
async fn assertion_rules() {
    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    let store = db.mcp_oauth().unwrap();
    let key = TestKey::es256();
    let client = signed_client(json!({ "jwks": { "keys": [key.jwk("k1")] } }));
    let now = Utc::now().timestamp();
    let ok = |claims: Value| key.assertion("k1", claims);
    let refused = |assertion: String, why: &'static str| {
        let client = client.clone();
        async move {
            let result = check(store, &client, &assertion).await;
            assert!(result.as_ref().is_err_and(|e| e.contains(why)), "expected '{why}': {result:?}");
        }
    };

    // LiteGen is the audience by its issuer or its token endpoint, alone or in a list.
    for aud in [json!(ISSUER), json!(format!("{ISSUER}/oauth/token")), json!(["https://other.example", ISSUER])] {
        assert_eq!(check(store, &client, &ok(json!({ "aud": aud }))).await, Ok(()), "{aud}");
    }
    refused(ok(json!({ "aud": "https://evil.example" })), "aud").await;
    refused(ok(json!({ "aud": null })), "aud").await;
    refused(ok(json!({ "iss": "https://evil.example/client.json" })), "iss").await;
    refused(ok(json!({ "sub": "https://evil.example/client.json" })), "sub").await;

    // Current, within a minute of clock difference.
    assert_eq!(check(store, &client, &ok(json!({ "exp": now - 30 }))).await, Ok(()));
    refused(ok(json!({ "exp": now - 120 })), "expired").await;
    refused(ok(json!({ "exp": null })), "exp").await;
    refused(ok(json!({ "nbf": now + 600 })), "not valid yet").await;

    // Each assertion once.
    let once = ok(json!({}));
    assert_eq!(check(store, &client, &once).await, Ok(()));
    refused(once, "used before").await;
    refused(ok(json!({ "jti": 7 })), "jti").await;

    // Signed by the client's key, with an algorithm LiteGen takes.
    let stranger = TestKey::es256();
    refused(stranger.assertion("k1", json!({})), "signature").await;
    refused(key.assertion("k2", json!({})), "signature").await;
    let genuine = ok(json!({}));
    let parts: Vec<&str> = genuine.split('.').collect();
    let forged_claims = b64(json!({ "iss": SIGNED_CLIENT, "sub": SIGNED_CLIENT, "aud": ISSUER, "exp": now + 300, "jti": "forged" }).to_string().as_bytes());
    refused(format!("{}.{forged_claims}.{}", parts[0], parts[2]), "signature").await;
    let unsigned = format!("{}.{}.", b64(br#"{"alg":"none"}"#), b64(json!({ "iss": SIGNED_CLIENT, "sub": SIGNED_CLIENT, "aud": ISSUER, "exp": now + 300 }).to_string().as_bytes()));
    refused(unsigned, "'none'").await;
    refused(key.sign(&json!({ "alg": "HS256", "kid": "k1" }), &json!({ "iss": SIGNED_CLIENT, "sub": SIGNED_CLIENT, "aud": ISSUER, "exp": now + 300 })), "'HS256'").await;
    refused("not.a.jwt".into(), "not a signed JWT").await;
    refused("two.parts".into(), "not a signed JWT").await;

    // Without a kid, any of the client's keys may verify it.
    let kidless = key.sign(&json!({ "alg": "ES256" }), &json!({ "iss": SIGNED_CLIENT, "sub": SIGNED_CLIENT, "aud": ISSUER, "exp": now + 300 }));
    assert_eq!(check(store, &client, &kidless).await, Ok(()));

    // A client that declares private_key_jwt but publishes no keys cannot sign in.
    let keyless = signed_client(json!({}));
    let result = check(store, &keyless, &ok(json!({}))).await;
    assert!(result.as_ref().is_err_and(|e| e.contains("publishes no keys")), "{result:?}");
}

#[tokio::test]
async fn assertion_keys_come_from_jwks_uri() {
    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    let store = db.mcp_oauth().unwrap();
    let (old, new) = (TestKey::es256(), TestKey::es256());
    let served = KeySet::new(json!({ "keys": [old.jwk("old")] }));
    let client = signed_client(json!({ "jwks_uri": "https://client.example.com/jwks.json" }));
    let cache = KeyCache::default();
    let now = Utc::now();

    assert_eq!(check_at(store, &client, &old.assertion("old", json!({})), &served, &cache, now).await, Ok(()));
    assert_eq!(check_at(store, &client, &old.assertion("old", json!({})), &served, &cache, now).await, Ok(()));
    assert_eq!(served.calls.load(Ordering::SeqCst), 1, "the key set is kept");

    // The client rotates its key: an unknown kid fetches the set again, but
    // not more than every half minute.
    served.serve(json!({ "keys": [new.jwk("new")] }));
    let result = check_at(store, &client, &new.assertion("new", json!({})), &served, &cache, now + chrono::Duration::seconds(5)).await;
    assert!(result.is_err(), "fetched too recently: {result:?}");
    assert_eq!(served.calls.load(Ordering::SeqCst), 1);
    let later = now + chrono::Duration::seconds(45);
    assert_eq!(check_at(store, &client, &new.assertion("new", json!({})), &served, &cache, later).await, Ok(()));
    assert_eq!(served.calls.load(Ordering::SeqCst), 2);

    // A key set that is not one is refused.
    served.serve(json!(["not", "a", "key set"]));
    let fresh_cache = KeyCache::default();
    let result = check_at(store, &client, &new.assertion("new", json!({})), &served, &fresh_cache, later).await;
    assert!(result.as_ref().is_err_and(|e| e.contains("not a JWK set")), "{result:?}");
}
