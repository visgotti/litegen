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

// ─── Task 4: authorization and consent ────────────────────────────────────────

const VERIFIER: &str = "a-test-code-verifier-that-is-long-enough-0123456789";
const REGISTERED: &str = "http://127.0.0.1/callback";
/// A native client's redirect: the registered loopback URI on its own port.
const REDIRECT: &str = "http://127.0.0.1:53111/callback";

/// A dynamically registered client; returns its id.
async fn register(app: &axum::Router) -> String {
    let (status, client) = post_json(app, "/oauth/register", json!({ "redirect_uris": [REGISTERED], "client_name": "Test client" })).await;
    assert_eq!(status, StatusCode::CREATED, "{client}");
    client["client_id"].as_str().unwrap().to_string()
}

/// A valid authorization request's query parameters.
fn authorize_query(client_id: &str) -> Vec<(String, String)> {
    [
        ("response_type", "code".to_string()),
        ("client_id", client_id.to_string()),
        ("redirect_uri", REDIRECT.to_string()),
        ("code_challenge", litegen::mcp::oauth::crypto::s256_challenge(VERIFIER)),
        ("code_challenge_method", "S256".to_string()),
        ("state", "st-1".to_string()),
        ("scope", "ai:generate ai:read".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// `query` with `key` set to `value`, or removed when `None`.
fn with(mut query: Vec<(String, String)>, key: &str, value: Option<&str>) -> Vec<(String, String)> {
    query.retain(|(k, _)| k != key);
    if let Some(value) = value {
        query.push((key.to_string(), value.to_string()));
    }
    query
}

/// `GET /oauth/authorize`: the status, the `Location` and the body.
async fn authorize(app: &axum::Router, query: &[(String, String)]) -> (StatusCode, Option<String>, String) {
    let qs: Vec<String> = query.iter().map(|(k, v)| format!("{k}={}", urlencoding::encode(v))).collect();
    let req = Request::get(format!("/oauth/authorize?{}", qs.join("&"))).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let location = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, location, String::from_utf8_lossy(&bytes).to_string())
}

/// A URL's query parameters.
fn query_of(url: &str) -> std::collections::HashMap<String, String> {
    reqwest::Url::parse(url).unwrap().query_pairs().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The pending request id from a redirect to the consent page.
async fn request_id(app: &axum::Router, query: &[(String, String)]) -> String {
    let (status, location, body) = authorize(app, query).await;
    assert_eq!(status, StatusCode::FOUND, "{body}");
    let location = location.expect("a redirect");
    assert!(location.starts_with("https://app.example.com/oauth/consent?"), "{location}");
    query_of(&location)["request"].clone()
}

/// A session call to the consent API: the status and the JSON body.
async fn consent(app: &axum::Router, (cookie, csrf): &(String, String), method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path).header("cookie", cookie).header("x-csrf-token", csrf);
    if body.is_some() {
        req = req.header("content-type", "application/json");
    }
    let req = req.body(body.map(|b| Body::from(b.to_string())).unwrap_or_default()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// The id of the (only) org a newly signed-up user owns.
async fn own_org(app: &axum::Router, session: &(String, String)) -> String {
    let (status, orgs) = consent(app, session, "GET", "/v1/orgs", None).await;
    assert_eq!(status, StatusCode::OK, "{orgs}");
    orgs[0]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn authorize_redirects_to_consent() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let client = register(&app).await;
    let id = request_id(&app, &authorize_query(&client)).await;
    assert!(id.len() >= 43, "an unguessable request id: {id}");

    let session = mcp_support::oauth_session(&app).await;
    let (status, view) = consent(&app, &session, "GET", &format!("/v1/oauth/requests/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["id"], id.as_str());
    assert_eq!(view["surface"], "ai");
    assert_eq!(view["requested_scopes"], json!(["ai:generate", "ai:read"]));
    assert_eq!(view["client"]["client_id"], client.as_str());
    assert_eq!(view["client"]["name"], "Test client");
    assert_eq!(view["client"]["redirect_host"], "127.0.0.1");
    assert_eq!(view["client"]["registration"], "dcr");
    let org = &view["orgs"][0];
    assert_eq!(org["role"], "owner");
    assert_eq!(org["grantable_scopes"], json!(["ai:generate", "ai:read"]));
    assert!(!org["apps"].as_array().unwrap().is_empty(), "the org's apps to pick from: {view}");
}

#[tokio::test]
async fn authorize_bad_redirect_is_a_page() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let client = register(&app).await;
    for query in [
        with(authorize_query(&client), "redirect_uri", Some("http://127.0.0.1:53111/elsewhere")),
        with(authorize_query(&client), "redirect_uri", Some("https://evil.example/callback")),
        with(authorize_query(&client), "client_id", Some("lgc_no_such_client_000000")),
        with(authorize_query(&client), "client_id", None),
    ] {
        let (status, location, body) = authorize(&app, &query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query:?}");
        assert!(location.is_none(), "never a redirect to an unverified URI: {query:?}");
        assert!(body.contains("sign-in link is invalid"), "{body}");
    }
}

#[tokio::test]
async fn authorize_error_redirect_has_iss() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let client = register(&app).await;
    for (query, error) in [
        (with(authorize_query(&client), "code_challenge", None), "invalid_request"),
        (with(authorize_query(&client), "code_challenge_method", Some("plain")), "invalid_request"),
        (with(authorize_query(&client), "code_challenge_method", None), "invalid_request"),
        (with(authorize_query(&client), "response_type", Some("token")), "unsupported_response_type"),
    ] {
        let (status, location, body) = authorize(&app, &query).await;
        assert_eq!(status, StatusCode::FOUND, "{query:?}: {body}");
        let location = location.unwrap();
        assert!(location.starts_with(REDIRECT), "{location}");
        let params = query_of(&location);
        assert_eq!(params["error"], error, "{location}");
        assert_eq!(params["iss"], BASE, "{location}");
        assert_eq!(params["state"], "st-1", "{location}");
        assert!(params.contains_key("error_description"), "{location}");
    }
}

#[tokio::test]
async fn origin_resource_with_admin_scopes_targets_admin() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let client = register(&app).await;
    let query = with(with(authorize_query(&client), "resource", Some("https://app.example.com")), "scope", Some("admin:read"));
    let id = request_id(&app, &query).await;
    let session = mcp_support::oauth_session(&app).await;
    let (_, view) = consent(&app, &session, "GET", &format!("/v1/oauth/requests/{id}"), None).await;
    assert_eq!(view["surface"], "admin", "{view}");
    assert_eq!(view["requested_scopes"], json!(["admin:read"]));

    // An endpoint URL decides on its own, slash or not; no scopes means all of its.
    let query = with(with(authorize_query(&client), "resource", Some(&format!("{BASE}/admin/mcp/"))), "scope", None);
    let id = request_id(&app, &query).await;
    let (_, view) = consent(&app, &session, "GET", &format!("/v1/oauth/requests/{id}"), None).await;
    assert_eq!(view["surface"], "admin", "{view}");
    assert_eq!(view["requested_scopes"], json!(["admin:read", "admin:write"]));
}

#[tokio::test]
async fn mixed_scopes_are_invalid_scope() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let client = register(&app).await;
    let (_, location, _) = authorize(&app, &with(authorize_query(&client), "scope", Some("ai:read admin:read"))).await;
    assert_eq!(query_of(&location.unwrap())["error"], "invalid_scope");

    // A resource LiteGen does not serve is refused (RFC 8707).
    let (_, location, _) = authorize(&app, &with(authorize_query(&client), "resource", Some("https://evil.example/mcp"))).await;
    assert_eq!(query_of(&location.unwrap())["error"], "invalid_target");
}

#[tokio::test]
async fn member_cannot_approve_admin() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let owner = mcp_support::oauth_session(&app).await;
    let org = own_org(&app, &owner).await;
    let member = mcp_support::oauth_session(&app).await;
    let (_, me) = consent(&app, &member, "GET", "/v1/auth/me", None).await;
    let member_id = me["id"].as_str().or_else(|| me["user"]["id"].as_str()).expect("user id").to_string();
    use litegen::db::DatabaseStore;
    db.add_org_member(&org, &member_id, litegen::types::Role::Member).await.unwrap();

    let client = register(&app).await;
    let id = request_id(&app, &with(authorize_query(&client), "scope", Some("admin:read admin:write"))).await;
    let (_, view) = consent(&app, &member, "GET", &format!("/v1/oauth/requests/{id}"), None).await;
    let theirs = view["orgs"].as_array().unwrap().iter().find(|o| o["id"] == org.as_str()).expect("the org they joined").clone();
    assert_eq!(theirs["role"], "member");
    assert_eq!(theirs["grantable_scopes"], json!(["admin:read"]), "a member can't give admin:write: {view}");

    let (status, err) = consent(&app, &member, "POST", &format!("/v1/oauth/requests/{id}/approve"), Some(json!({ "org_id": org, "scopes": ["admin:write"] }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{err}");
    assert_eq!(err["error"]["code"], "scope_not_grantable", "{err}");

    // Not a member at all.
    let stranger = mcp_support::oauth_session(&app).await;
    let (status, err) = consent(&app, &stranger, "POST", &format!("/v1/oauth/requests/{id}/approve"), Some(json!({ "org_id": org, "scopes": ["admin:read"] }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{err}");
}

#[tokio::test]
async fn approve_returns_code_redirect() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let client = register(&app).await;
    let id = request_id(&app, &authorize_query(&client)).await;
    let session = mcp_support::oauth_session(&app).await;
    let org = own_org(&app, &session).await;
    let (status, answer) =
        consent(&app, &session, "POST", &format!("/v1/oauth/requests/{id}/approve"), Some(json!({ "org_id": org, "scopes": ["ai:generate", "ai:read"] }))).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let to = answer["redirect_to"].as_str().unwrap();
    assert!(to.starts_with(&format!("{REDIRECT}?")), "{to}");
    let params = query_of(to);
    assert_eq!(params["code"].len(), 43, "{to}");
    assert_eq!(params["state"], "st-1");
    assert_eq!(params["iss"], BASE);

    // An app that is not the org's is refused.
    let id = request_id(&app, &authorize_query(&client)).await;
    let (status, _) = consent(
        &app,
        &session,
        "POST",
        &format!("/v1/oauth/requests/{id}/approve"),
        Some(json!({ "org_id": org, "app_id": "no-such-app", "scopes": ["ai:read"] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn request_is_single_use() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let client = register(&app).await;
    let id = request_id(&app, &authorize_query(&client)).await;
    let session = mcp_support::oauth_session(&app).await;
    let org = own_org(&app, &session).await;
    let approve = json!({ "org_id": org, "scopes": ["ai:read"] });
    let path = format!("/v1/oauth/requests/{id}/approve");
    assert_eq!(consent(&app, &session, "POST", &path, Some(approve.clone())).await.0, StatusCode::OK);
    assert_eq!(consent(&app, &session, "POST", &path, Some(approve)).await.0, StatusCode::CONFLICT);
    assert_eq!(consent(&app, &session, "GET", &format!("/v1/oauth/requests/{id}"), None).await.0, StatusCode::CONFLICT);
    assert_eq!(consent(&app, &session, "GET", "/v1/oauth/requests/no-such-request", None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deny_returns_access_denied() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let client = register(&app).await;
    let id = request_id(&app, &authorize_query(&client)).await;
    let session = mcp_support::oauth_session(&app).await;
    let (status, answer) = consent(&app, &session, "POST", &format!("/v1/oauth/requests/{id}/deny"), None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let params = query_of(answer["redirect_to"].as_str().unwrap());
    assert_eq!(params["error"], "access_denied");
    assert_eq!(params["iss"], BASE);
    assert_eq!(params["state"], "st-1");
    assert_eq!(consent(&app, &session, "POST", &format!("/v1/oauth/requests/{id}/approve"), Some(json!({ "org_id": "x", "scopes": ["ai:read"] }))).await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn consent_api_needs_a_browser_session() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let client = register(&app).await;
    let id = request_id(&app, &authorize_query(&client)).await;
    let key = mcp_support::org_key(&app, "generate,read,admin").await;
    let req = Request::get(format!("/v1/oauth/requests/{id}")).header("authorization", format!("Bearer {key}")).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "an API key cannot approve sign-ins");
}
