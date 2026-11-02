//! LiteGen's MCP endpoints over the real route table: `/mcp` (the AI API) and
//! `/admin/mcp` (the management API), on both protocol revisions clients use.

#[allow(dead_code)]
mod harness;
mod mcp_support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use litegen::config::Mode;
use mcp_support::{Mcp, CURRENT, LEGACY};
use serde_json::json;
use tower::ServiceExt;

const AI_INSTRUCTIONS: &str = "LiteGen generates images, video and 3D models";
const ADMIN_INSTRUCTIONS: &str = "LiteGen management";

fn initialize_params() -> serde_json::Value {
    json!({ "protocolVersion": LEGACY, "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } })
}

// ─── Task 1: the two endpoints answer ─────────────────────────────────────────

#[tokio::test]
async fn initialize_answers_on_both_endpoints_legacy() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;

    let ai = Mcp::new(&app, "/mcp").legacy().ok("initialize", initialize_params()).await;
    assert_eq!(ai["serverInfo"]["name"], "litegen");
    assert!(ai["instructions"].as_str().unwrap().starts_with(AI_INSTRUCTIONS), "{ai}");

    let admin = Mcp::new(&app, "/admin/mcp").legacy().ok("initialize", initialize_params()).await;
    assert_eq!(admin["serverInfo"]["name"], "litegen");
    assert!(admin["instructions"].as_str().unwrap().starts_with(ADMIN_INSTRUCTIONS), "{admin}");
}

#[tokio::test]
async fn discover_answers_on_current_protocol() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;

    let result = Mcp::new(&app, "/mcp").ok("server/discover", json!({})).await;
    assert!(result["instructions"].as_str().unwrap().starts_with(AI_INSTRUCTIONS), "{result}");
    let versions: Vec<&str> = result["supportedVersions"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    assert!(versions.contains(&CURRENT), "{versions:?}");
}

#[tokio::test]
async fn get_is_405() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let req = Request::builder()
        .method("GET")
        .uri("/mcp")
        .header("host", "localhost")
        .header("accept", "text/event-stream")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn disabled_endpoints_are_404() {
    let (app, _db, _dir) = mcp_support::app(|c| c.mcp.enabled = false).await;
    assert_eq!(Mcp::new(&app, "/mcp").rpc("server/discover", json!({})).await.status, StatusCode::NOT_FOUND);

    let (app, _db, _dir) = mcp_support::app(|c| c.mcp.admin_enabled = false).await;
    assert_eq!(Mcp::new(&app, "/admin/mcp").rpc("server/discover", json!({})).await.status, StatusCode::NOT_FOUND);
    assert_eq!(Mcp::new(&app, "/mcp").rpc("server/discover", json!({})).await.status, StatusCode::OK);
}

// ─── Task 2: who is calling ───────────────────────────────────────────────────

#[tokio::test]
async fn hosted_without_credentials_is_401_with_challenge() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::Hosted).await;
    let rpc = Mcp::new(&app, "/mcp").rpc("server/discover", json!({})).await;
    assert_eq!(rpc.status, StatusCode::UNAUTHORIZED, "{}", rpc.body);
    let challenge = rpc.headers.get("www-authenticate").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(challenge.starts_with("Bearer"), "www-authenticate: {challenge:?}");
}

#[tokio::test]
async fn invalid_key_is_401() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::SingleTenant).await;
    let rpc = Mcp::new(&app, "/mcp").bearer("sk_live_nope").rpc("server/discover", json!({})).await;
    assert_eq!(rpc.status, StatusCode::UNAUTHORIZED, "{}", rpc.body);
}

#[tokio::test]
async fn valid_key_initializes() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::SingleTenant).await;
    let key = mcp_support::create_key(&app, "generate,read").await;
    let result = Mcp::new(&app, "/mcp").bearer(&key).legacy().ok("initialize", initialize_params()).await;
    assert_eq!(result["serverInfo"]["name"], "litegen");
}

#[tokio::test]
async fn session_cookie_is_ignored() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::Hosted).await;
    let cookie = mcp_support::signup_cookie(&app).await;
    let rpc = Mcp::new(&app, "/mcp").header("cookie", &cookie).rpc("server/discover", json!({})).await;
    assert_eq!(rpc.status, StatusCode::UNAUTHORIZED, "a session cookie must not authenticate MCP: {}", rpc.body);
}

#[tokio::test]
async fn foreign_origin_is_403() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let rpc = Mcp::new(&app, "/mcp").header("origin", "https://evil.example").rpc("server/discover", json!({})).await;
    assert_eq!(rpc.status, StatusCode::FORBIDDEN, "{}", rpc.body);
    // Non-browser clients send no Origin at all and pass.
    assert_eq!(Mcp::new(&app, "/mcp").rpc("server/discover", json!({})).await.status, StatusCode::OK);
}

#[tokio::test]
async fn public_host_passes_and_foreign_host_is_403() {
    let (app, _db, _dir) = mcp_support::app(|c| c.server.public_base_url = Some("https://app.example.com/api".into())).await;
    let ok = Mcp::new(&app, "/mcp").host("app.example.com").rpc("server/discover", json!({})).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    let origin_ok = Mcp::new(&app, "/mcp")
        .host("app.example.com")
        .header("origin", "https://app.example.com")
        .rpc("server/discover", json!({}))
        .await;
    assert_eq!(origin_ok.status, StatusCode::OK, "the public origin itself is allowed: {}", origin_ok.body);
    let foreign = Mcp::new(&app, "/mcp").host("evil.example").rpc("server/discover", json!({})).await;
    assert_eq!(foreign.status, StatusCode::FORBIDDEN, "{}", foreign.body);
}

#[tokio::test]
async fn unknown_toolset_is_400() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let rpc = Mcp::new(&app, "/mcp?toolsets=images,nope").rpc("server/discover", json!({})).await;
    assert_eq!(rpc.status, StatusCode::BAD_REQUEST, "{}", rpc.body);
    let body: serde_json::Value = serde_json::from_str(&rpc.body).unwrap();
    assert_eq!(body["error"]["code"], "unknown_toolset");
    assert!(body["error"]["message"].as_str().unwrap().contains("'nope'"), "{body}");
}
