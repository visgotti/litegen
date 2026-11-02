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

// ─── Task 5: tools/list per credential ────────────────────────────────────────

/// Every AI-endpoint tool a non-user key can see: the 19 AI tools, the
/// endpoint-shared `get_my_limits`, and `wait_for_generation`.
const AI_TOOLS_FOR_KEYS: usize = 21;

#[tokio::test]
async fn ai_key_is_exactly_ai() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::SingleTenant).await;
    let key = mcp_support::create_key(&app, "generate").await;

    let ai = Mcp::new(&app, "/mcp").bearer(&key).tool_names().await;
    assert_eq!(ai.len(), AI_TOOLS_FOR_KEYS, "{ai:?}");
    for tool in ["generate_image", "list_models", "wait_for_generation", "get_my_limits"] {
        assert!(ai.contains(&tool.to_string()), "{tool} missing: {ai:?}");
    }
    assert!(!ai.contains(&"get_current_user".to_string()), "a key is not a signed-in user");

    let admin = Mcp::new(&app, "/admin/mcp").bearer(&key).tool_names().await;
    assert_eq!(admin, vec!["get_my_limits".to_string()]);
}

#[tokio::test]
async fn default_key_lists_ai_and_admin_reads() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::SingleTenant).await;
    let key = mcp_support::create_key(&app, "generate,read").await;

    assert_eq!(Mcp::new(&app, "/mcp").bearer(&key).tool_names().await.len(), AI_TOOLS_FOR_KEYS);
    let admin = Mcp::new(&app, "/admin/mcp").bearer(&key).tool_names().await;
    assert!(admin.contains(&"get_spend_report".to_string()), "{admin:?}");
    assert!(admin.contains(&"list_request_logs".to_string()), "{admin:?}");
    assert!(!admin.contains(&"create_api_key".to_string()), "{admin:?}");
    assert!(!admin.contains(&"set_org_limits".to_string()), "{admin:?}");
}

#[tokio::test]
async fn admin_key_lists_key_tools() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::SingleTenant).await;
    let key = mcp_support::create_key(&app, "generate,read,admin").await;
    let admin = Mcp::new(&app, "/admin/mcp").bearer(&key).tool_names().await;
    for tool in ["create_api_key", "revoke_api_key", "set_key_limits", "set_org_limits", "list_audit_log"] {
        assert!(admin.contains(&tool.to_string()), "{tool} missing: {admin:?}");
    }
    // Org, app and member management needs a signed-in user, even for an admin key.
    assert!(!admin.contains(&"list_orgs".to_string()), "{admin:?}");
}

#[tokio::test]
async fn toolsets_filter() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let names = Mcp::new(&app, "/mcp?toolsets=images").tool_names().await;
    assert_eq!(names, vec!["estimate_image_cost", "generate_image", "segment_image"]);
}

#[tokio::test]
async fn annotations_are_explicit() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let mut tools = Mcp::new(&app, "/mcp").tools().await;
    tools.extend(Mcp::new(&app, "/admin/mcp").tools().await);
    assert!(tools.len() > 100, "dev mode should list nearly everything: {}", tools.len());
    for tool in &tools {
        let name = tool["name"].as_str().unwrap();
        assert!(tool["title"].as_str().is_some_and(|t| !t.is_empty()), "{name} has no title");
        assert!(tool["description"].as_str().is_some_and(|d| !d.is_empty()), "{name} has no description");
        for hint in ["readOnlyHint", "destructiveHint", "idempotentHint", "openWorldHint"] {
            assert!(tool["annotations"][hint].is_boolean(), "{name} lacks {hint}");
        }
    }
    let find = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap_or_else(|| panic!("{n} not listed")).clone();
    assert_eq!(find("revoke_api_key")["annotations"]["destructiveHint"], true);
    assert_eq!(find("estimate_image_cost")["annotations"]["readOnlyHint"], true);
    assert_eq!(find("generate_image")["annotations"]["readOnlyHint"], false);
    assert_eq!(find("generate_image")["annotations"]["openWorldHint"], true);
    assert_eq!(find("wait_for_generation")["annotations"]["readOnlyHint"], true);
    assert_eq!(find("generate_3d_model")["title"], "Generate 3D model");
    // OpenAPI text is cleaned of internal plan references before clients see it.
    for tool in &tools {
        let description = tool["description"].as_str().unwrap();
        assert!(!description.contains("Live:") && !description.contains("```"), "{}: {description}", tool["name"]);
        assert!(description.chars().count() <= 1_200, "{} description is {} chars", tool["name"], description.len());
    }
}

#[tokio::test]
async fn legacy_and_current_protocols_list_tools() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::SingleTenant).await;
    let key = mcp_support::create_key(&app, "generate,read").await;

    let legacy = Mcp::new(&app, "/mcp").bearer(&key).legacy();
    legacy.ok("initialize", initialize_params()).await;
    let legacy_names = legacy.tool_names().await;
    let current_names = Mcp::new(&app, "/mcp").bearer(&key).tool_names().await;
    assert_eq!(legacy_names, current_names);
    assert_eq!(current_names.len(), AI_TOOLS_FOR_KEYS);
}
