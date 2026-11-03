//! OAuth 2.1 for the MCP endpoints, over the real route table: discovery
//! documents, client registration, consent, tokens, and OAuth callers.

#[allow(dead_code)]
mod harness;
mod mcp_support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use litegen::config::{AppConfig, Mode};
use mcp_support::Mcp;
use serde_json::{json, Value};
use tower::ServiceExt;

/// The public base URL every test serves under: the hosted service's shape,
/// with the API behind an `/api` prefix.
const BASE: &str = "https://app.example.com/api";

fn public(c: &mut AppConfig) {
    c.server.public_base_url = Some(BASE.into());
}

async fn get_json(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn post_json(app: &axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::post(path).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn www_authenticate(rpc: &mcp_support::Rpc) -> String {
    rpc.headers.get("www-authenticate").and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

// ─── Task 2: discovery documents and the 401 challenge ───────────────────────

#[tokio::test]
async fn protected_resource_documents() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    // Derived from the resource URL (root well-known) or reached through the
    // `/api` prefix the challenge names: the same document.
    for path in ["/.well-known/oauth-protected-resource/api/mcp", "/.well-known/oauth-protected-resource/mcp"] {
        let (status, doc) = get_json(&app, path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(doc["resource"], "https://app.example.com/api/mcp", "{path}");
        assert_eq!(doc["scopes_supported"], json!(["ai:generate", "ai:read"]));
        assert_eq!(doc["authorization_servers"], json!([BASE]));
        assert_eq!(doc["bearer_methods_supported"], json!(["header"]));
        assert_eq!(doc["resource_name"], "LiteGen");
    }

    let (status, admin) = get_json(&app, "/.well-known/oauth-protected-resource/api/admin/mcp").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(admin["resource"], "https://app.example.com/api/admin/mcp");
    assert_eq!(admin["scopes_supported"], json!(["admin:read", "admin:write"]));
    assert_eq!(admin["resource_name"], "LiteGen admin");

    // A client that called the slash form gets that form back (RFC 9728 §3.3).
    let (_, slash) = get_json(&app, "/.well-known/oauth-protected-resource/api/admin/mcp/").await;
    assert_eq!(slash["resource"], "https://app.example.com/api/admin/mcp/");
    assert_eq!(slash["scopes_supported"], json!(["admin:read", "admin:write"]));
}

#[tokio::test]
async fn authorization_server_document() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let expected = json!({
        "issuer": BASE,
        "authorization_endpoint": format!("{BASE}/oauth/authorize"),
        "token_endpoint": format!("{BASE}/oauth/token"),
        "registration_endpoint": format!("{BASE}/oauth/register"),
        "revocation_endpoint": format!("{BASE}/oauth/revoke"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "revocation_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": ["ai:generate", "ai:read", "admin:read", "admin:write"],
        "client_id_metadata_document_supported": true,
        "authorization_response_iss_parameter_supported": true,
    });
    for path in ["/.well-known/oauth-authorization-server/api", "/.well-known/oauth-authorization-server"] {
        let (status, doc) = get_json(&app, path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(doc, expected, "{path}");
    }
}

#[tokio::test]
async fn challenge_points_at_metadata() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let rpc = Mcp::new(&app, "/admin/mcp").rpc("server/discover", json!({})).await;
    assert_eq!(rpc.status, StatusCode::UNAUTHORIZED, "{}", rpc.body);
    let challenge = www_authenticate(&rpc);
    assert!(challenge.starts_with("Bearer "), "{challenge}");
    assert!(
        challenge.contains(r#"resource_metadata="https://app.example.com/api/.well-known/oauth-protected-resource/admin/mcp""#),
        "{challenge}"
    );
    assert!(challenge.contains(r#"scope="admin:read admin:write""#), "{challenge}");

    let rpc = Mcp::new(&app, "/mcp/").rpc("server/discover", json!({})).await;
    let challenge = www_authenticate(&rpc);
    assert!(
        challenge.contains(r#"resource_metadata="https://app.example.com/api/.well-known/oauth-protected-resource/mcp/""#),
        "the slash form points at its own document: {challenge}"
    );
    assert!(challenge.contains(r#"scope="ai:generate ai:read""#), "{challenge}");
}

#[tokio::test]
async fn oauth_disabled_serves_nothing() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, |c| {
        public(c);
        c.mcp.oauth.enabled = false;
    })
    .await;
    assert_eq!(get_json(&app, "/.well-known/oauth-protected-resource/mcp").await.0, StatusCode::NOT_FOUND);
    assert_eq!(get_json(&app, "/.well-known/oauth-authorization-server").await.0, StatusCode::NOT_FOUND);
    let rpc = Mcp::new(&app, "/mcp").rpc("server/discover", json!({})).await;
    assert_eq!(rpc.status, StatusCode::UNAUTHORIZED);
    assert_eq!(www_authenticate(&rpc), r#"Bearer realm="litegen""#);
}

#[tokio::test]
async fn admin_endpoint_off_has_no_admin_document() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, |c| {
        public(c);
        c.mcp.admin_enabled = false;
    })
    .await;
    assert_eq!(get_json(&app, "/.well-known/oauth-protected-resource/api/admin/mcp").await.0, StatusCode::NOT_FOUND);
    let (_, doc) = get_json(&app, "/.well-known/oauth-authorization-server").await;
    assert_eq!(doc["scopes_supported"], json!(["ai:generate", "ai:read"]), "only the scopes of endpoints that exist");
}

// ─── Task 3: client registration ──────────────────────────────────────────────

#[tokio::test]
async fn dcr_registers_public_client() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let (status, client) = post_json(
        &app,
        "/oauth/register",
        json!({ "redirect_uris": ["http://127.0.0.1/callback"], "client_name": "Test client", "grant_types": ["authorization_code", "refresh_token"] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{client}");
    let id = client["client_id"].as_str().unwrap();
    assert!(id.starts_with("lgc_") && id.len() == 26, "{id}");
    assert!(id[4..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "base64url: {id}");
    assert!(client["client_id_issued_at"].as_i64().is_some(), "{client}");
    assert_eq!(client["token_endpoint_auth_method"], "none");
    assert_eq!(client["redirect_uris"], json!(["http://127.0.0.1/callback"]));
    assert_eq!(client["client_name"], "Test client");
    assert_eq!(client["grant_types"], json!(["authorization_code", "refresh_token"]));

    let refused = |body: Value| {
        let app = app.clone();
        async move { post_json(&app, "/oauth/register", body).await }
    };
    let (status, err) = refused(json!({ "redirect_uris": ["http://127.0.0.1/cb"], "token_endpoint_auth_method": "client_secret_basic" })).await;
    assert_eq!((status, err["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_client_metadata")), "{err}");
    let (status, err) = refused(json!({ "redirect_uris": ["http://example.com/cb"] })).await;
    assert_eq!((status, err["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_redirect_uri")), "{err}");
    let (_, err) = refused(json!({ "client_name": "no redirect" })).await;
    assert_eq!(err["error"], "invalid_redirect_uri", "{err}");
    let (_, err) = refused(json!({ "redirect_uris": ["https://ok.example/cb#frag"] })).await;
    assert_eq!(err["error"], "invalid_redirect_uri", "a fragment is not allowed: {err}");
    let (_, err) = refused(json!({ "redirect_uris": ["http://127.0.0.1/cb"], "grant_types": ["client_credentials"] })).await;
    assert_eq!(err["error"], "invalid_client_metadata", "{err}");
    let (_, err) = refused(json!({ "redirect_uris": ["http://127.0.0.1/cb"], "response_types": ["token"] })).await;
    assert_eq!(err["error"], "invalid_client_metadata", "{err}");
    let (_, err) = refused(Value::String("not an object".into())).await;
    assert_eq!(err["error"], "invalid_client_metadata", "{err}");
}
