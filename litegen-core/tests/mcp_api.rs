//! LiteGen's MCP endpoints over the real route table: `/mcp` (the AI API) and
//! `/admin/mcp` (the management API), on both protocol revisions clients use.

#[allow(dead_code)]
mod harness;
mod mcp_support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
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
