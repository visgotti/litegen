//! OAuth 2.1 for the MCP endpoints, over the real route table: discovery
//! documents, client registration, consent, tokens, and OAuth callers.

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
        "token_endpoint_auth_methods_supported": ["none", "private_key_jwt"],
        "token_endpoint_auth_signing_alg_values_supported": ["RS256", "PS256", "ES256", "EdDSA"],
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

#[tokio::test]
async fn dcr_registers_what_litegen_supports() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let registered = |body: Value| {
        let app = app.clone();
        async move { post_json(&app, "/oauth/register", body).await }
    };
    // No grant types: both of LiteGen's, and the answer says so.
    let (status, client) = registered(json!({ "redirect_uris": [REGISTERED] })).await;
    assert_eq!(status, StatusCode::CREATED, "{client}");
    assert_eq!(client["grant_types"], json!(["authorization_code", "refresh_token"]));
    assert_eq!(client["response_types"], json!(["code"]));
    // Types LiteGen does not offer are left out, not refused: VS Code asks
    // for device_code when the server lists no grant types.
    let (status, client) = registered(json!({
        "redirect_uris": [REGISTERED],
        "grant_types": ["authorization_code", "refresh_token", "urn:ietf:params:oauth:grant-type:device_code"],
        "response_types": ["code", "token"],
    }))
    .await;
    assert_eq!(status, StatusCode::CREATED, "{client}");
    assert_eq!(client["grant_types"], json!(["authorization_code", "refresh_token"]));
    assert_eq!(client["response_types"], json!(["code"]));

    // At most ten redirect URIs, names cut to 200 characters, bodies to 16 KiB.
    let eleven: Vec<String> = (0..11).map(|i| format!("http://127.0.0.1/cb{i}")).collect();
    let (status, err) = registered(json!({ "redirect_uris": eleven })).await;
    assert_eq!((status, err["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_redirect_uri")), "{err}");
    let (_, client) = registered(json!({ "redirect_uris": [REGISTERED], "client_name": "n".repeat(300) })).await;
    assert_eq!(client["client_name"].as_str().map(|n| n.chars().count()), Some(200), "{client}");
    let (status, _) = registered(json!({ "redirect_uris": [REGISTERED], "client_uri": format!("https://example.com/{}", "x".repeat(17 * 1024)) })).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

/// A request that arrived over a connection from `peer`.
fn from_peer(mut req: Request<Body>, peer: [u8; 4]) -> Request<Body> {
    req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((peer, 40000))));
    req
}

/// `POST /oauth/register` from `peer`, through a proxy that says
/// `forwarded_for` when given.
async fn register_from(app: &axum::Router, peer: [u8; 4], forwarded_for: Option<&str>) -> axum::response::Response {
    let mut req = Request::post("/oauth/register").header("content-type", "application/json");
    if let Some(forwarded_for) = forwarded_for {
        req = req.header("x-forwarded-for", forwarded_for);
    }
    let req = req.body(Body::from(json!({ "redirect_uris": ["http://127.0.0.1/callback"] }).to_string())).unwrap();
    app.clone().oneshot(from_peer(req, peer)).await.unwrap()
}

#[tokio::test]
async fn sign_in_endpoints_are_limited_per_address() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, |c| {
        public(c);
        c.mcp.oauth.rate_limit_per_minute = 2;
    })
    .await;
    let visitor = [203, 0, 113, 5];
    assert_eq!(register_from(&app, visitor, None).await.status(), StatusCode::CREATED);
    assert_eq!(register_from(&app, visitor, None).await.status(), StatusCode::CREATED);
    let refused = register_from(&app, visitor, None).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    let wait: u64 = refused.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).expect("Retry-After");
    assert!((1..=30).contains(&wait), "{wait}");
    let body: Value = serde_json::from_slice(&axum::body::to_bytes(refused.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(body["error"], "temporarily_unavailable", "{body}");

    // The sign-in page counts against the same allowance, and says so as a page.
    let page = app.clone().oneshot(from_peer(Request::get("/oauth/authorize?client_id=lgc_any").body(Body::empty()).unwrap(), visitor)).await.unwrap();
    assert_eq!(page.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(page.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").starts_with("text/html"));
    assert!(page.headers().get("retry-after").is_some());
    let text = String::from_utf8(axum::body::to_bytes(page.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    assert!(text.contains("Too many sign-in attempts"), "{text}");

    // Another address has its own allowance. A visitor can't pick its
    // address with X-Forwarded-For; a proxy on this machine names it.
    assert_eq!(register_from(&app, [203, 0, 113, 6], None).await.status(), StatusCode::CREATED);
    assert_eq!(register_from(&app, visitor, Some("198.51.100.7")).await.status(), StatusCode::TOO_MANY_REQUESTS);
    let proxy = [127, 0, 0, 1];
    assert_eq!(register_from(&app, proxy, Some("203.0.113.5")).await.status(), StatusCode::TOO_MANY_REQUESTS, "the visitor, behind the proxy");
    assert_eq!(register_from(&app, proxy, Some("203.0.113.5, 198.51.100.7")).await.status(), StatusCode::CREATED, "the proxy's own entry is the last");

    // Tokens are not limited: a refresh never waits on a busy network.
    let token = Request::post("/oauth/token").header("content-type", "application/x-www-form-urlencoded").body(Body::from("grant_type=refresh_token")).unwrap();
    assert_eq!(app.clone().oneshot(from_peer(token, visitor)).await.unwrap().status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sign_in_limit_can_be_turned_off() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, |c| {
        public(c);
        c.mcp.oauth.rate_limit_per_minute = 0;
    })
    .await;
    for _ in 0..40 {
        assert_eq!(register_from(&app, [203, 0, 113, 5], None).await.status(), StatusCode::CREATED);
    }
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

/// A client registered with `redirect` as its only redirect URI: its id.
async fn register_redirect(app: &axum::Router, redirect: &str) -> String {
    let (status, client) = post_json(app, "/oauth/register", json!({ "redirect_uris": [redirect], "client_name": "Web client" })).await;
    assert_eq!(status, StatusCode::CREATED, "{client}");
    client["client_id"].as_str().unwrap().to_string()
}

/// RFC 9700 §4.11.2: before anyone has signed in or seen anything, an error
/// goes back automatically only to a redirect URI LiteGen trusts. Anyone may
/// register a client with any `https` URI, so otherwise a link to LiteGen's
/// own `/oauth/authorize` with a broken parameter would bounce the browser
/// from LiteGen's origin to the attacker's site. LiteGen shows a page
/// instead, naming the problem and where the app's link leads.
#[tokio::test]
async fn a_pre_consent_error_never_bounces_the_browser_to_an_https_client() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let landing = "https://phish.example/landing";
    let client = register_redirect(&app, landing).await;
    let base = with(with(authorize_query(&client), "redirect_uri", Some(landing)), "state", Some("\"><script>alert(1)</script>"));
    for (query, error) in [
        (with(base.clone(), "response_type", Some("token")), "unsupported_response_type"),
        (with(base.clone(), "code_challenge", None), "invalid_request"),
        (with(base.clone(), "scope", Some("ai:read admin:read")), "invalid_scope"),
        (with(base.clone(), "resource", Some("https://evil.example/mcp")), "invalid_target"),
    ] {
        let (status, location, body) = authorize(&app, &query).await;
        assert_eq!((status, location.as_deref()), (StatusCode::BAD_REQUEST, None), "{error}: {body}");
        assert!(body.contains(error), "the page names the error: {body}");
        assert!(body.contains("phish.example"), "and where the app's link leads: {body}");
        assert!(!body.contains("<script>"), "the app's state is escaped: {body}");
        // The person may still hand the app its error, by choice.
        let href = body.split("href=\"").nth(1).and_then(|rest| rest.split('"').next()).expect("a link back");
        let back = href.replace("&amp;", "&");
        assert!(back.starts_with(&format!("{landing}?error={error}&")), "{href}");
        let params = query_of(&back);
        assert_eq!((params["state"].as_str(), params["iss"].as_str()), ("\"><script>alert(1)</script>", BASE), "{back}");
    }
}

/// Loopback redirect URIs (native apps, RFC 8252) and hosts the operator lists
/// in `mcp.oauth.trusted_redirect_hosts` still get their errors back at once.
/// The list matches a host exactly, ignoring case and port, so a host that
/// merely starts or ends with a listed one doesn't.
#[tokio::test]
async fn trusted_redirect_uris_get_their_errors_back_automatically() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, |c| {
        public(c);
        c.mcp.oauth.trusted_redirect_hosts = vec![" Claude.AI ".into()];
    })
    .await;
    for (redirect, trusted) in [
        ("https://claude.ai/api/mcp/auth_callback", true),
        ("https://CLAUDE.ai:8443/cb", true),
        ("https://localhost/cb", true),
        ("http://127.0.0.1:6274/oauth/callback", true),
        ("https://claude.ai.evil.example/cb", false),
        ("https://sub.claude.ai/cb", false),
        ("https://evil.example/claude.ai", false),
    ] {
        let client = register_redirect(&app, redirect).await;
        let query = with(with(authorize_query(&client), "redirect_uri", Some(redirect)), "response_type", Some("token"));
        let (status, location, body) = authorize(&app, &query).await;
        if trusted {
            assert_eq!(status, StatusCode::FOUND, "{redirect}: {body}");
            let location = location.expect("a redirect");
            assert_eq!(query_of(&location)["error"], "unsupported_response_type", "{location}");
        } else {
            assert_eq!((status, location), (StatusCode::BAD_REQUEST, None), "{redirect}");
        }
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
async fn unknown_scopes_are_ignored() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let client = register(&app).await;
    let session = mcp_support::oauth_session(&app).await;
    // Kiro asks for `openid email profile offline_access` unless told
    // otherwise: none is LiteGen's, so the request is for all of /mcp's.
    for (asked, requested) in [
        ("openid email profile offline_access", json!(["ai:generate", "ai:read"])),
        ("openid ai:read offline_access", json!(["ai:read"])),
    ] {
        let id = request_id(&app, &with(authorize_query(&client), "scope", Some(asked))).await;
        let (_, view) = consent(&app, &session, "GET", &format!("/v1/oauth/requests/{id}"), None).await;
        assert_eq!(view["surface"], "ai", "{asked}: {view}");
        assert_eq!(view["requested_scopes"], requested, "{asked}: {view}");
    }
}

#[tokio::test]
async fn repeated_authorize_parameters() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let client = register(&app).await;
    let repeated = |key: &str| {
        let mut query = authorize_query(&client);
        let value = query.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap();
        query.push((key.to_string(), value));
        query
    };
    // A second client_id or redirect_uri: LiteGen can't tell where to
    // answer, so it says so on a page.
    for key in ["client_id", "redirect_uri"] {
        let (status, location, body) = authorize(&app, &repeated(key)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{key}");
        assert!(location.is_none(), "{key}");
        assert!(body.contains("sign-in link is invalid") && body.contains("more than once"), "{key}: {body}");
    }
    // Any other parameter twice goes back to the app (RFC 6749 §3.1).
    let (status, location, body) = authorize(&app, &repeated("scope")).await;
    assert_eq!(status, StatusCode::FOUND, "{body}");
    let params = query_of(&location.unwrap());
    assert_eq!(params["error"], "invalid_request");
    assert!(params["error_description"].contains("scope"), "{params:?}");
    assert_eq!(params["iss"], BASE);
    // Two resources: a sign-in is for one endpoint (RFC 8707).
    let mut two = with(authorize_query(&client), "resource", Some(&format!("{BASE}/mcp")));
    two.push(("resource".into(), format!("{BASE}/admin/mcp")));
    let (_, location, _) = authorize(&app, &two).await;
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
async fn viewer_can_approve_generation() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let owner = mcp_support::oauth_session(&app).await;
    let org = own_org(&app, &owner).await;
    let viewer = mcp_support::oauth_session(&app).await;
    let (viewer_id, _) = whoami(&app, &viewer).await;
    use litegen::db::DatabaseStore;
    db.add_org_member(&org, &viewer_id, litegen::types::Role::Viewer).await.unwrap();

    // A viewer may generate on the dashboard, so they may let an app do it.
    let client = register(&app).await;
    let id = request_id(&app, &authorize_query(&client)).await;
    let (_, view) = consent(&app, &viewer, "GET", &format!("/v1/oauth/requests/{id}"), None).await;
    let theirs = view["orgs"].as_array().unwrap().iter().find(|o| o["id"] == org.as_str()).expect("the org they joined").clone();
    assert_eq!(theirs["role"], "viewer");
    assert_eq!(theirs["grantable_scopes"], json!(["ai:generate", "ai:read"]), "{view}");
    let (status, answer) = consent(&app, &viewer, "POST", &format!("/v1/oauth/requests/{id}/approve"), Some(json!({ "org_id": org, "scopes": ["ai:generate"] }))).await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    // But not management writes.
    let id = request_id(&app, &with(authorize_query(&client), "scope", Some("admin:read admin:write"))).await;
    let (status, err) = consent(&app, &viewer, "POST", &format!("/v1/oauth/requests/{id}/approve"), Some(json!({ "org_id": org, "scopes": ["admin:write"] }))).await;
    assert_eq!((status, err["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("scope_not_grantable")), "{err}");
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

// ─── Task 5: tokens and revocation ────────────────────────────────────────────

/// A registered client and the owner who approves its requests.
struct Flow {
    client: String,
    session: (String, String),
    org: String,
}

impl Flow {
    async fn new(app: &axum::Router) -> Flow {
        let client = register(app).await;
        let session = mcp_support::oauth_session(app).await;
        let org = own_org(app, &session).await;
        Flow { client, session, org }
    }

    /// The authorization code for an approved request.
    async fn code(&self, app: &axum::Router, query: &[(String, String)], scopes: &[&str]) -> String {
        let id = request_id(app, query).await;
        let approve = json!({ "org_id": self.org, "scopes": scopes });
        let (status, answer) = consent(app, &self.session, "POST", &format!("/v1/oauth/requests/{id}/approve"), Some(approve)).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        query_of(answer["redirect_to"].as_str().unwrap())["code"].clone()
    }

    /// Access and refresh tokens for an approved AI request.
    async fn tokens(&self, app: &axum::Router, scopes: &[&str]) -> (String, String) {
        self.tokens_for(app, &authorize_query(&self.client), scopes).await
    }

    /// Access and refresh tokens for an approved request with this query.
    async fn tokens_for(&self, app: &axum::Router, query: &[(String, String)], scopes: &[&str]) -> (String, String) {
        let code = self.code(app, query, scopes).await;
        let (status, _, body) = form_post(app, "/oauth/token", &exchange(&self.client, &code)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        (body["access_token"].as_str().unwrap().to_string(), body["refresh_token"].as_str().unwrap().to_string())
    }

    /// Admin-endpoint tokens with these scopes.
    async fn admin_tokens(&self, app: &axum::Router, scopes: &[&str]) -> (String, String) {
        self.tokens_for(app, &with(authorize_query(&self.client), "scope", Some("admin:read admin:write")), scopes).await
    }
}

/// A form POST: the status, the headers and the JSON body.
async fn form_post(app: &axum::Router, path: &str, form: &[(&str, &str)]) -> (StatusCode, axum::http::HeaderMap, Value) {
    let body: Vec<String> = form.iter().map(|(k, v)| format!("{k}={}", urlencoding::encode(v))).collect();
    let req = Request::post(path).header("content-type", "application/x-www-form-urlencoded").body(Body::from(body.join("&"))).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// The form that redeems `code`.
fn exchange<'a>(client: &'a str, code: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![("grant_type", "authorization_code"), ("code", code), ("redirect_uri", REDIRECT), ("client_id", client), ("code_verifier", VERIFIER)]
}

/// `form` with `key` set to `value`, or removed when `None`.
fn set<'a>(mut form: Vec<(&'a str, &'a str)>, key: &'a str, value: Option<&'a str>) -> Vec<(&'a str, &'a str)> {
    form.retain(|(k, _)| *k != key);
    if let Some(value) = value {
        form.push((key, value));
    }
    form
}

fn hashed(token: &str) -> String {
    litegen::mcp::oauth::crypto::sha256_hex(token)
}

#[tokio::test]
async fn code_exchange_issues_tokens() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let code = flow.code(&app, &authorize_query(&flow.client), &["ai:generate", "ai:read"]).await;
    let (status, headers, body) = form_post(&app, "/oauth/token", &exchange(&flow.client, &code)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let access = body["access_token"].as_str().unwrap();
    let refresh = body["refresh_token"].as_str().unwrap();
    assert!(access.starts_with("lgat_") && access.len() == 48, "{access}");
    assert!(refresh.starts_with("lgrt_") && refresh.len() == 48, "{refresh}");
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["expires_in"], 3600);
    assert_eq!(body["scope"], "ai:generate ai:read");
    assert_eq!(headers.get("cache-control").and_then(|v| v.to_str().ok()), Some("no-store"));

    use litegen::db::DatabaseStore;
    let store = db.mcp_oauth().unwrap();
    assert!(store.get_token(access).await.unwrap().is_none(), "a token itself is never stored");
    assert!(store.get_token(&hashed(access)).await.unwrap().is_some());
    assert!(store.get_token(&hashed(refresh)).await.unwrap().is_some());
    assert!(store.get_client(&flow.client).await.unwrap().unwrap().last_used_at.is_some(), "the client is marked used");
}

#[tokio::test]
async fn wrong_verifier_is_invalid_grant() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let code = flow.code(&app, &authorize_query(&flow.client), &["ai:read"]).await;
    let wrong = "another-verifier-of-the-right-length-0123456789-abc";
    let (status, _, body) = form_post(&app, "/oauth/token", &set(exchange(&flow.client, &code), "code_verifier", Some(wrong))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_grant")), "{body}");
    let (_, _, body) = form_post(&app, "/oauth/token", &exchange(&flow.client, &code)).await;
    assert_eq!(body["error"], "invalid_grant", "a code presented once is spent: {body}");
}

#[tokio::test]
async fn wrong_redirect_uri_is_invalid_grant() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let code = flow.code(&app, &authorize_query(&flow.client), &["ai:read"]).await;
    let (status, _, body) = form_post(&app, "/oauth/token", &set(exchange(&flow.client, &code), "redirect_uri", Some("http://127.0.0.1:1/callback"))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_grant")), "{body}");
}

#[tokio::test]
async fn code_reuse_revokes_grant() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let code = flow.code(&app, &authorize_query(&flow.client), &["ai:read"]).await;
    let (status, _, first) = form_post(&app, "/oauth/token", &exchange(&flow.client, &code)).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let (_, _, second) = form_post(&app, "/oauth/token", &exchange(&flow.client, &code)).await;
    assert_eq!(second["error"], "invalid_grant", "{second}");

    use litegen::db::DatabaseStore;
    let store = db.mcp_oauth().unwrap();
    let access = store.get_token(&hashed(first["access_token"].as_str().unwrap())).await.unwrap().unwrap();
    assert!(access.revoked_at.is_some(), "a replayed code revokes what it issued");
    assert!(store.get_grant(&access.grant_id).await.unwrap().unwrap().revoked_at.is_some());
    let rpc = Mcp::new(&app, "/mcp").bearer(first["access_token"].as_str().unwrap()).rpc("tools/list", json!({})).await;
    assert_eq!(rpc.status, StatusCode::UNAUTHORIZED, "the first token stops working");
}

#[tokio::test]
async fn refresh_rotates() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let (access, refresh) = flow.tokens(&app, &["ai:generate", "ai:read"]).await;
    let (status, _, renewed) = form_post(&app, "/oauth/token", &refresh_with(&flow.client, &refresh)).await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    let (access2, refresh2) = (renewed["access_token"].as_str().unwrap(), renewed["refresh_token"].as_str().unwrap());
    assert!(access2.starts_with("lgat_") && access2 != access);
    assert!(refresh2.starts_with("lgrt_") && refresh2 != refresh);
    assert_eq!(renewed["scope"], "ai:generate ai:read");

    // A narrower scope is fine, a wider one is not.
    let wider = [refresh_with(&flow.client, refresh2), vec![("scope", "admin:write")]].concat();
    assert_eq!(form_post(&app, "/oauth/token", &wider).await.2["error"], "invalid_scope");
    let narrower = [refresh_with(&flow.client, refresh2), vec![("scope", "ai:read")]].concat();
    let (status, _, renewed2) = form_post(&app, "/oauth/token", &narrower).await;
    assert_eq!(status, StatusCode::OK, "{renewed2}");

    // The rotated-out token again: a replay. The grant goes, and with it the newest pair.
    let (_, _, replay) = form_post(&app, "/oauth/token", &refresh_with(&flow.client, &refresh)).await;
    assert_eq!(replay["error"], "invalid_grant", "{replay}");
    let newest = refresh_with(&flow.client, renewed2["refresh_token"].as_str().unwrap());
    assert_eq!(form_post(&app, "/oauth/token", &newest).await.2["error"], "invalid_grant", "the grant was revoked");
}

/// The form that renews `client`'s tokens with `token`.
fn refresh_with<'a>(client: &'a str, token: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![("grant_type", "refresh_token"), ("refresh_token", token), ("client_id", client)]
}

#[tokio::test]
async fn refresh_needs_a_live_membership() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let (_, refresh) = flow.tokens(&app, &["ai:read"]).await;
    let (_, me) = consent(&app, &flow.session, "GET", "/v1/auth/me", None).await;
    let user_id = me["id"].as_str().or_else(|| me["user"]["id"].as_str()).unwrap().to_string();
    use litegen::db::DatabaseStore;
    db.update_user(&user_id, None, Some(false), None).await.unwrap();
    let (status, _, body) = form_post(&app, "/oauth/token", &refresh_with(&flow.client, &refresh)).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_grant")), "a deactivated user's grant is dead: {body}");
}

#[tokio::test]
async fn resource_origin_is_accepted_and_foreign_is_invalid_target() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let code = flow.code(&app, &authorize_query(&flow.client), &["ai:read"]).await;
    let (status, _, body) = form_post(&app, "/oauth/token", &set(exchange(&flow.client, &code), "resource", Some("https://app.example.com"))).await;
    assert_eq!(status, StatusCode::OK, "the origin leads to the endpoint: {body}");

    for foreign in ["https://evil.example/mcp", "https://app.example.com/api/admin/mcp"] {
        let code = flow.code(&app, &authorize_query(&flow.client), &["ai:read"]).await;
        let (status, _, body) = form_post(&app, "/oauth/token", &set(exchange(&flow.client, &code), "resource", Some(foreign))).await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_target")), "{foreign}: {body}");
    }
}

#[tokio::test]
async fn revoke_endpoint_is_always_200() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    assert_eq!(form_post(&app, "/oauth/revoke", &[("token", "lgat_not-a-token")]).await.0, StatusCode::OK);
    assert_eq!(form_post(&app, "/oauth/revoke", &[]).await.0, StatusCode::OK, "even with no token");

    use litegen::db::DatabaseStore;
    let store = db.mcp_oauth().unwrap();
    let (access, refresh) = flow.tokens(&app, &["ai:read"]).await;
    assert_eq!(form_post(&app, "/oauth/revoke", &[("token", access.as_str()), ("token_type_hint", "access_token")]).await.0, StatusCode::OK);
    let access_row = store.get_token(&hashed(&access)).await.unwrap().unwrap();
    assert!(access_row.revoked_at.is_some());
    assert!(store.get_grant(&access_row.grant_id).await.unwrap().unwrap().revoked_at.is_none(), "an access token goes alone");

    assert_eq!(form_post(&app, "/oauth/revoke", &[("token", refresh.as_str())]).await.0, StatusCode::OK);
    assert!(store.get_grant(&access_row.grant_id).await.unwrap().unwrap().revoked_at.is_some(), "a refresh token takes its grant");
}

#[tokio::test]
async fn token_endpoint_errors() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let error = |(status, headers, body): (StatusCode, axum::http::HeaderMap, Value)| {
        assert_eq!(headers.get("cache-control").and_then(|v| v.to_str().ok()), Some("no-store"), "{body}");
        (status, body["error"].as_str().unwrap_or("").to_string())
    };
    let bad = |e: &str| (StatusCode::BAD_REQUEST, e.to_string());

    assert_eq!(error(form_post(&app, "/oauth/token", &[("grant_type", "client_credentials"), ("client_id", &flow.client)]).await), bad("unsupported_grant_type"));
    assert_eq!(error(form_post(&app, "/oauth/token", &[("grant_type", "authorization_code"), ("client_id", &flow.client)]).await), bad("invalid_request"));
    assert_eq!(error(form_post(&app, "/oauth/token", &[("client_id", &flow.client)]).await), bad("invalid_request"));
    let code = flow.code(&app, &authorize_query(&flow.client), &["ai:read"]).await;
    assert_eq!(
        error(form_post(&app, "/oauth/token", &set(exchange(&flow.client, &code), "client_id", Some("lgc_no_such_client_000000"))).await),
        (StatusCode::UNAUTHORIZED, "invalid_client".to_string())
    );
    let duplicate = [exchange(&flow.client, &code), vec![("code", "another")]].concat();
    assert_eq!(error(form_post(&app, "/oauth/token", &duplicate).await), bad("invalid_request"));

    // Another registered client cannot redeem this client's code.
    let other = register(&app).await;
    let code = flow.code(&app, &authorize_query(&flow.client), &["ai:read"]).await;
    assert_eq!(error(form_post(&app, "/oauth/token", &set(exchange(&flow.client, &code), "client_id", Some(&other))).await), bad("invalid_grant"));

    // The form encoding is required (RFC 6749 §3.2).
    let req = Request::post("/oauth/token").header("content-type", "application/json").body(Body::from(r#"{"grant_type":"refresh_token"}"#)).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn client_without_refresh_grant_gets_no_refresh_token() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let (_, registered) = post_json(&app, "/oauth/register", json!({ "redirect_uris": [REGISTERED], "grant_types": ["authorization_code"] })).await;
    assert_eq!(registered["grant_types"], json!(["authorization_code"]), "{registered}");
    let client = registered["client_id"].as_str().unwrap().to_string();
    let session = mcp_support::oauth_session(&app).await;
    let org = own_org(&app, &session).await;
    let flow = Flow { client: client.clone(), session, org };
    let code = flow.code(&app, &authorize_query(&client), &["ai:read"]).await;
    let (status, _, tokens) = form_post(&app, "/oauth/token", &exchange(&client, &code)).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    assert!(tokens["access_token"].is_string());
    assert!(tokens.get("refresh_token").is_none(), "it registered without the refresh_token grant: {tokens}");

    // A refresh token it holds anyway is refused for it (RFC 6749 §5.2).
    use litegen::db::DatabaseStore;
    use litegen::mcp::oauth::store::{TokenKind, TokenRow};
    let store = db.mcp_oauth().unwrap();
    let grant_id = store.get_token(&hashed(tokens["access_token"].as_str().unwrap())).await.unwrap().unwrap().grant_id;
    let now = chrono::Utc::now();
    let smuggled = "lgrt_smuggled-refresh-token-0000000000000000000";
    let row = TokenRow { token_hash: hashed(smuggled), grant_id, kind: TokenKind::Refresh, created_at: now, expires_at: now + chrono::Duration::days(1), rotated_at: None, revoked_at: None };
    store.insert_token(&row).await.unwrap();
    let (status, _, body) = form_post(&app, "/oauth/token", &refresh_with(&client, smuggled)).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("unauthorized_client")), "{body}");
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A client that authenticates the way ChatGPT's metadata document says it
/// does: `private_key_jwt`, RS256.
struct Signer(aws_lc_rs::rsa::KeyPair);

impl Signer {
    fn new() -> Signer {
        Signer(aws_lc_rs::rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap())
    }

    fn jwks(&self) -> Value {
        use aws_lc_rs::signature::KeyPair as _;
        let key = aws_lc_rs::signature::RsaPublicKeyComponents::<Vec<u8>>::from(self.0.public_key());
        json!({ "keys": [{ "kty": "RSA", "kid": "k1", "alg": "RS256", "use": "sig", "n": b64(&key.n), "e": b64(&key.e) }] })
    }

    fn assertion(&self, client_id: &str, aud: &str) -> String {
        let now = chrono::Utc::now().timestamp();
        let header = json!({ "alg": "RS256", "kid": "k1", "typ": "JWT" });
        let claims = json!({ "iss": client_id, "sub": client_id, "aud": aud, "iat": now, "exp": now + 300, "jti": uuid::Uuid::new_v4().to_string() });
        let input = format!("{}.{}", b64(header.to_string().as_bytes()), b64(claims.to_string().as_bytes()));
        let mut signature = vec![0; self.0.public_modulus_len()];
        let rng = aws_lc_rs::rand::SystemRandom::new();
        self.0.sign(&aws_lc_rs::signature::RSA_PKCS1_SHA256, &rng, input.as_bytes(), &mut signature).unwrap();
        format!("{input}.{}", b64(&signature))
    }
}

#[tokio::test]
async fn private_key_jwt_client_must_sign() {
    use litegen::db::DatabaseStore;
    use litegen::mcp::oauth::store::{OAuthClient, Registration};
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    // A client known by its metadata document, which publishes its key; the
    // copy is fresh, so LiteGen does not fetch it again.
    let signer = Signer::new();
    let client = "https://client.example.com/oauth/client.json";
    let now = chrono::Utc::now();
    let metadata = json!({ "client_id": client, "redirect_uris": [REGISTERED], "token_endpoint_auth_method": "private_key_jwt", "jwks": signer.jwks() });
    db.mcp_oauth()
        .unwrap()
        .upsert_client(&OAuthClient {
            client_id: client.into(),
            registration: Registration::Cimd,
            client_name: "Signed client".into(),
            redirect_uris: vec![REGISTERED.into()],
            logo_uri: None,
            client_uri: None,
            metadata,
            created_at: now,
            refreshed_at: now,
            last_used_at: None,
        })
        .await
        .unwrap();
    let session = mcp_support::oauth_session(&app).await;
    let org = own_org(&app, &session).await;
    let flow = Flow { client: client.into(), session, org };
    let code = flow.code(&app, &authorize_query(client), &["ai:read"]).await;
    let invalid_client = |(status, _, body): (StatusCode, axum::http::HeaderMap, Value)| {
        assert_eq!((status, body["error"].as_str()), (StatusCode::UNAUTHORIZED, Some("invalid_client")), "{body}");
    };

    // No assertion, or another key's: invalid_client, and the code is not spent.
    invalid_client(signed_token_request(&app, &exchange(client, &code), None).await);
    invalid_client(signed_token_request(&app, &exchange(client, &code), Some(&Signer::new().assertion(client, BASE))).await);

    // Its own: tokens.
    let assertion = signer.assertion(client, BASE);
    let (status, _, tokens) = signed_token_request(&app, &exchange(client, &code), Some(&assertion)).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    let refresh = tokens["refresh_token"].as_str().unwrap().to_string();

    // Each request needs a new assertion. The token endpoint may be the
    // audience, and the client may leave client_id out.
    invalid_client(signed_token_request(&app, &refresh_with(client, &refresh), Some(&assertion)).await);
    let fresh = signer.assertion(client, &format!("{BASE}/oauth/token"));
    let without_id = [("grant_type", "refresh_token"), ("refresh_token", refresh.as_str())];
    let (status, _, body) = signed_token_request(&app, &without_id, Some(&fresh)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A token request with `form` and, if given, a client assertion.
async fn signed_token_request(app: &axum::Router, form: &[(&str, &str)], assertion: Option<&str>) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut pairs = form.to_vec();
    if let Some(assertion) = assertion {
        pairs.push(("client_assertion_type", "urn:ietf:params:oauth:client-assertion-type:jwt-bearer"));
        pairs.push(("client_assertion", assertion));
    }
    form_post(app, "/oauth/token", &pairs).await
}

// ─── Task 6: OAuth callers on the MCP endpoints ───────────────────────────────

fn is_error(result: &Value) -> bool {
    result["isError"].as_bool().unwrap_or(false)
}

fn text_of(result: &Value) -> String {
    result["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")
}

/// The signed-in user's id and email.
async fn whoami(app: &axum::Router, session: &(String, String)) -> (String, String) {
    let (_, me) = consent(app, session, "GET", "/v1/auth/me", None).await;
    let user = if me["user"].is_object() { &me["user"] } else { &me };
    (user["id"].as_str().unwrap().to_string(), user["email"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn oauth_ai_token_lists_and_generates() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let (access, _) = flow.tokens(&app, &["ai:generate", "ai:read"]).await;
    let mcp = Mcp::new(&app, "/mcp").bearer(&access);
    let names = mcp.tool_names().await;
    assert!(names.iter().any(|n| n == "generate_image"), "{names:?}");
    assert!(names.iter().any(|n| n == "get_current_user"), "{names:?}");

    let image = mcp.call("generate_image", json!({ "model": "mock/image-gen", "prompt": "a fox", "preview": false })).await;
    assert!(!is_error(&image), "{image}");
    let me = mcp.call("get_current_user", json!({})).await;
    assert!(!is_error(&me), "{me}");
    let (_, email) = whoami(&app, &flow.session).await;
    assert!(text_of(&me).contains(&email), "acts as the person who approved: {me}");

    // ai:read alone cannot generate.
    let (reader, _) = flow.tokens(&app, &["ai:read"]).await;
    let names = Mcp::new(&app, "/mcp").bearer(&reader).tool_names().await;
    assert!(!names.iter().any(|n| n == "generate_image"), "{names:?}");
    assert!(names.iter().any(|n| n == "list_models"), "{names:?}");
}

#[tokio::test]
async fn oauth_management_read_token_sees_only_reads() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let (access, _) = flow.admin_tokens(&app, &["admin:read"]).await;
    let names = Mcp::new(&app, "/admin/mcp").bearer(&access).tool_names().await;
    for read in ["get_spend_report", "list_members", "get_org"] {
        assert!(names.iter().any(|n| n == read), "{read} missing: {names:?}");
    }
    for write in ["set_org_limits", "revoke_api_key", "update_org", "create_org"] {
        assert!(!names.iter().any(|n| n == write), "{write} must be hidden: {names:?}");
    }

    let (writer, _) = flow.admin_tokens(&app, &["admin:read", "admin:write"]).await;
    let names = Mcp::new(&app, "/admin/mcp").bearer(&writer).tool_names().await;
    assert!(names.iter().any(|n| n == "set_org_limits"), "{names:?}");
    assert!(!names.iter().any(|n| n == "create_org"), "a connected app acts in its one org: {names:?}");
}

#[tokio::test]
async fn oauth_token_wrong_endpoint_is_401() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let (access, _) = flow.tokens(&app, &["ai:read"]).await;
    let rpc = Mcp::new(&app, "/admin/mcp").bearer(&access).rpc("tools/list", json!({})).await;
    assert_eq!(rpc.status, StatusCode::UNAUTHORIZED, "{}", rpc.body);
    let challenge = www_authenticate(&rpc);
    assert!(challenge.contains(r#"error="invalid_token""#), "{challenge}");
    assert!(challenge.contains("oauth-protected-resource/admin/mcp"), "{challenge}");

    let rpc = Mcp::new(&app, "/mcp").bearer("lgat_not-a-real-token").rpc("tools/list", json!({})).await;
    assert_eq!(rpc.status, StatusCode::UNAUTHORIZED);
    assert!(www_authenticate(&rpc).contains(r#"error="invalid_token""#));
}

#[tokio::test]
async fn oauth_token_on_rest_api_is_401() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let (access, _) = flow.tokens(&app, &["ai:generate", "ai:read"]).await;
    for path in ["/v1/models", "/v1/generations"] {
        let req = Request::get(path).header("authorization", format!("Bearer {access}")).body(Body::empty()).unwrap();
        assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::UNAUTHORIZED, "{path}");
    }
}

#[tokio::test]
async fn revoked_grant_stops_working() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let (access, refresh) = flow.tokens(&app, &["ai:read"]).await;
    let mcp = Mcp::new(&app, "/mcp").bearer(&access);
    assert_eq!(mcp.rpc("tools/list", json!({})).await.status, StatusCode::OK);
    assert_eq!(form_post(&app, "/oauth/revoke", &[("token", refresh.as_str())]).await.0, StatusCode::OK);
    assert_eq!(mcp.rpc("tools/list", json!({})).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn removed_member_stops_working() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let owner = Flow::new(&app).await;
    let member_session = mcp_support::oauth_session(&app).await;
    let (member_id, _) = whoami(&app, &member_session).await;
    use litegen::db::DatabaseStore;
    db.add_org_member(&owner.org, &member_id, litegen::types::Role::Member).await.unwrap();
    let member = Flow { client: owner.client.clone(), session: member_session, org: owner.org.clone() };
    let (access, _) = member.tokens(&app, &["ai:read"]).await;
    let mcp = Mcp::new(&app, "/mcp").bearer(&access);
    assert_eq!(mcp.rpc("tools/list", json!({})).await.status, StatusCode::OK);

    let (status, _) = consent(&app, &owner.session, "DELETE", &format!("/v1/orgs/{}/members/{member_id}", owner.org), None).await;
    assert!(status.is_success(), "the owner removes the member: {status}");
    assert_eq!(mcp.rpc("tools/list", json!({})).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn deactivated_person_access_token_is_refused() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let (access, _) = flow.tokens(&app, &["ai:read"]).await;
    let mcp = Mcp::new(&app, "/mcp").bearer(&access);
    assert_eq!(mcp.rpc("tools/list", json!({})).await.status, StatusCode::OK);

    let (user_id, _) = whoami(&app, &flow.session).await;
    use litegen::db::DatabaseStore;
    db.update_user(&user_id, None, Some(false), None).await.unwrap();
    let rpc = mcp.rpc("tools/list", json!({})).await;
    assert_eq!(rpc.status, StatusCode::UNAUTHORIZED, "{}", rpc.body);
    assert!(www_authenticate(&rpc).contains(r#"error="invalid_token""#));
    assert!(rpc.body.contains("can no longer act in its organization"), "{}", rpc.body);
}

#[tokio::test]
async fn deleted_app_ends_the_sign_in() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    // A second app in the org, picked on the consent page.
    use litegen::db::DatabaseStore;
    let now = chrono::Utc::now();
    let app_id = uuid::Uuid::new_v4().to_string();
    let doomed = litegen::types::Application {
        id: app_id.clone(),
        org_id: flow.org.clone(),
        name: "Doomed".into(),
        slug: app_id.clone(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_application(&doomed).await.unwrap();
    let id = request_id(&app, &authorize_query(&flow.client)).await;
    let approve = json!({ "org_id": flow.org, "app_id": app_id, "scopes": ["ai:read"] });
    let (status, answer) = consent(&app, &flow.session, "POST", &format!("/v1/oauth/requests/{id}/approve"), Some(approve)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let code = query_of(answer["redirect_to"].as_str().unwrap())["code"].clone();
    let (_, _, tokens) = form_post(&app, "/oauth/token", &exchange(&flow.client, &code)).await;
    let (access, refresh) = (tokens["access_token"].as_str().unwrap(), tokens["refresh_token"].as_str().unwrap());
    let mcp = Mcp::new(&app, "/mcp").bearer(access);
    assert_eq!(mcp.rpc("tools/list", json!({})).await.status, StatusCode::OK);

    assert!(db.delete_application(&app_id).await.unwrap());
    let rpc = mcp.rpc("tools/list", json!({})).await;
    assert_eq!(rpc.status, StatusCode::UNAUTHORIZED, "{}", rpc.body);
    assert!(rpc.body.contains("no longer exists"), "{}", rpc.body);

    // Refreshing ends the sign-in instead of handing out a token that would
    // be refused.
    let (status, _, body) = form_post(&app, "/oauth/token", &refresh_with(&flow.client, refresh)).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_grant")), "{body}");
    assert!(body["error_description"].as_str().unwrap_or("").contains("no longer exists"), "{body}");
    let store = db.mcp_oauth().unwrap();
    let grant_id = store.get_token(&hashed(access)).await.unwrap().unwrap().grant_id;
    assert!(store.get_grant(&grant_id).await.unwrap().unwrap().revoked_at.is_some(), "the grant is revoked");
}

#[tokio::test]
async fn oauth_token_is_confined_to_its_org() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    // The same person owns a second org.
    let elsewhere = Flow::new(&app).await;
    let (user_id, _) = whoami(&app, &flow.session).await;
    use litegen::db::DatabaseStore;
    db.add_org_member(&elsewhere.org, &user_id, litegen::types::Role::Owner).await.unwrap();

    let (access, _) = flow.admin_tokens(&app, &["admin:read", "admin:write"]).await;
    let mcp = Mcp::new(&app, "/admin/mcp").bearer(&access);
    let own = mcp.call("get_org", json!({ "id": flow.org })).await;
    assert!(!is_error(&own), "{own}");
    let other = mcp.call("get_org", json!({ "id": elsewhere.org })).await;
    assert!(is_error(&other), "the grant is for one org: {other}");
    assert!(text_of(&other).starts_with("HTTP 403"), "{other}");
    let renamed = mcp.call("update_org", json!({ "id": elsewhere.org, "name": "hijacked" })).await;
    assert!(is_error(&renamed), "{renamed}");

    let orgs = mcp.call("list_orgs", json!({})).await;
    let listed: Vec<String> = orgs["structuredContent"]
        .as_array()
        .or_else(|| orgs["structuredContent"]["data"].as_array())
        .or_else(|| orgs["structuredContent"]["items"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|o| o["id"].as_str().map(String::from))
        .collect();
    assert_eq!(listed, vec![flow.org.clone()], "only the grant's org: {orgs}");
}

#[tokio::test]
async fn get_current_user_shows_only_the_grants_org() {
    let (app, db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    // The same person owns a second org, which the grant is not for.
    let elsewhere = Flow::new(&app).await;
    let (user_id, _) = whoami(&app, &flow.session).await;
    use litegen::db::DatabaseStore;
    db.add_org_member(&elsewhere.org, &user_id, litegen::types::Role::Owner).await.unwrap();

    let (access, _) = flow.tokens(&app, &["ai:read"]).await;
    let me = Mcp::new(&app, "/mcp").bearer(&access).call("get_current_user", json!({})).await;
    assert!(!is_error(&me), "{me}");
    let listed: Vec<&str> = me["structuredContent"]["orgs"].as_array().into_iter().flatten().filter_map(|o| o["id"].as_str()).collect();
    assert_eq!(listed, vec![flow.org.as_str()], "the app learns nothing of the person's other orgs: {me}");
}

// ─── Task 7: connected apps ───────────────────────────────────────────────────

/// A master-key call: the status and the JSON body.
async fn as_master(app: &axum::Router, method: &str, path: &str) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {}", mcp_support::MASTER_KEY)).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn grants_list_and_revoke() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let (access, _) = flow.tokens(&app, &["ai:generate", "ai:read"]).await;

    let (status, grants) = consent(&app, &flow.session, "GET", "/v1/oauth/grants", None).await;
    assert_eq!(status, StatusCode::OK, "{grants}");
    let grants = grants.as_array().expect("a list").clone();
    assert_eq!(grants.len(), 1, "{grants:?}");
    let grant = &grants[0];
    assert_eq!(grant["client_id"], flow.client.as_str());
    assert_eq!(grant["client_name"], "Test client");
    assert_eq!(grant["surface"], "ai");
    assert_eq!(grant["scopes"], json!(["ai:generate", "ai:read"]));
    assert_eq!(grant["org_id"], flow.org.as_str());
    assert!(grant["created_at"].is_string(), "{grant}");

    // Using the token records when.
    assert_eq!(Mcp::new(&app, "/mcp").bearer(&access).rpc("tools/list", json!({})).await.status, StatusCode::OK);
    let (_, grants) = consent(&app, &flow.session, "GET", "/v1/oauth/grants", None).await;
    assert!(grants[0]["last_used_at"].is_string(), "{grants}");

    let id = grant["id"].as_str().unwrap();
    let (status, revoked) = consent(&app, &flow.session, "DELETE", &format!("/v1/oauth/grants/{id}"), None).await;
    assert_eq!((status, &revoked), (StatusCode::OK, &json!({ "revoked": true })));
    assert_eq!(Mcp::new(&app, "/mcp").bearer(&access).rpc("tools/list", json!({})).await.status, StatusCode::UNAUTHORIZED);
    let (_, grants) = consent(&app, &flow.session, "GET", "/v1/oauth/grants", None).await;
    assert_eq!(grants, json!([]));
    assert_eq!(consent(&app, &flow.session, "DELETE", &format!("/v1/oauth/grants/{id}"), None).await.0, StatusCode::NOT_FOUND, "already gone");
}

#[tokio::test]
async fn grant_of_another_user_is_404() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    flow.tokens(&app, &["ai:read"]).await;
    let (_, grants) = consent(&app, &flow.session, "GET", "/v1/oauth/grants", None).await;
    let id = grants[0]["id"].as_str().unwrap().to_string();

    let stranger = mcp_support::oauth_session(&app).await;
    assert_eq!(consent(&app, &stranger, "GET", "/v1/oauth/grants", None).await.1, json!([]));
    assert_eq!(consent(&app, &stranger, "DELETE", &format!("/v1/oauth/grants/{id}"), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(consent(&app, &stranger, "GET", &format!("/v1/oauth/grants?user_id={id}"), None).await.0, StatusCode::FORBIDDEN, "only the platform admin names a user");

    // The platform admin can see and revoke anyone's, naming the person.
    let (user_id, _) = whoami(&app, &flow.session).await;
    assert_eq!(as_master(&app, "GET", "/v1/oauth/grants").await.0, StatusCode::BAD_REQUEST, "user_id is required");
    let (status, listed) = as_master(&app, "GET", &format!("/v1/oauth/grants?user_id={user_id}")).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed[0]["id"], id.as_str());
    assert_eq!(as_master(&app, "DELETE", &format!("/v1/oauth/grants/{id}")).await.0, StatusCode::OK);

    // An API key has no connected apps of its own.
    let key = mcp_support::org_key(&app, "generate,read,admin").await;
    let req = Request::get("/v1/oauth/grants").header("authorization", format!("Bearer {key}")).body(Body::empty()).unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn management_tool_lists_connected_apps() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let flow = Flow::new(&app).await;
    let (reader, _) = flow.admin_tokens(&app, &["admin:read"]).await;
    let mcp = Mcp::new(&app, "/admin/mcp").bearer(&reader);
    let names = mcp.tool_names().await;
    assert!(names.iter().any(|n| n == "list_connected_apps"), "{names:?}");
    assert!(!names.iter().any(|n| n == "revoke_connected_app"), "revoking needs admin:write: {names:?}");
    let listed = mcp.call("list_connected_apps", json!({})).await;
    assert!(!is_error(&listed), "{listed}");
    assert!(text_of(&listed).contains(&flow.client), "its own grant is listed: {listed}");

    let (writer, _) = flow.admin_tokens(&app, &["admin:read", "admin:write"]).await;
    let mcp = Mcp::new(&app, "/admin/mcp").bearer(&writer);
    assert!(mcp.tool_names().await.iter().any(|n| n == "revoke_connected_app"));
    let (_, grants) = consent(&app, &flow.session, "GET", "/v1/oauth/grants", None).await;
    let id = grants[0]["id"].as_str().unwrap();
    let revoked = mcp.call("revoke_connected_app", json!({ "id": id })).await;
    assert!(!is_error(&revoked), "an app may disconnect itself: {revoked}");
    assert_eq!(mcp.rpc("tools/list", json!({})).await.status, StatusCode::UNAUTHORIZED);
}

// ─── Task 9: the endpoints the dashboard's Connect page shows ────────────────

#[tokio::test]
async fn auth_config_names_the_mcp_endpoints() {
    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, public).await;
    let (status, config) = get_json(&app, "/v1/auth/config").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        config["mcp"],
        json!({ "ai_url": format!("{BASE}/mcp"), "admin_url": format!("{BASE}/admin/mcp"), "oauth": true })
    );

    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, |c| {
        public(c);
        c.mcp.admin_enabled = false;
        c.mcp.oauth.enabled = false;
    })
    .await;
    let (_, config) = get_json(&app, "/v1/auth/config").await;
    assert_eq!(config["mcp"], json!({ "ai_url": format!("{BASE}/mcp"), "admin_url": null, "oauth": false }));

    let (app, _db, _dir) = mcp_support::secured_app_with(Mode::Hosted, |c| c.mcp.enabled = false).await;
    let (_, config) = get_json(&app, "/v1/auth/config").await;
    assert!(config["mcp"].is_null(), "{config}");
}
