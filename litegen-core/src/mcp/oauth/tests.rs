//! Unit tests for the OAuth authorization server's rules.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};

use super::clients::{redirect_allowed, resolve_client, ClientError, MetadataFetcher, PublicFetcher};
use super::store::Registration;
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

    // private_key_jwt is allowed (the assertion is ignored: the client is
    // public and PKCE protects the exchange); the name defaults to the host.
    let keyed = invalid(json!({ "client_id": other, "redirect_uris": ["https://other.example.org/cb"], "token_endpoint_auth_method": "private_key_jwt" }))
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
