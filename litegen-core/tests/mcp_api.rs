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

// ─── Task 6: tools/call through the API router ────────────────────────────────

fn text_of(result: &serde_json::Value) -> String {
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["type"] == "text")
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_error(result: &serde_json::Value) -> bool {
    result["isError"].as_bool().unwrap_or(false)
}

#[tokio::test]
async fn generate_image_with_mock_returns_data() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::SingleTenant).await;
    let key = mcp_support::create_key(&app, "generate,read").await;
    let result = Mcp::new(&app, "/mcp")
        .bearer(&key)
        .call("generate_image", json!({ "model": "mock/image-gen", "prompt": "a red cube", "preview": false }))
        .await;
    assert!(!is_error(&result), "{result}");
    let first = &result["structuredContent"]["data"][0];
    assert!(first.get("url").is_some() || first.get("b64_json").is_some(), "{result}");
    // The text block carries the same JSON, for clients that read only text.
    let text: serde_json::Value = serde_json::from_str(&text_of(&result).lines().next().unwrap()).unwrap();
    assert_eq!(text["model"], result["structuredContent"]["model"]);
}

#[tokio::test]
async fn get_model_with_slash_id() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let mcp = Mcp::new(&app, "/mcp");
    let models = mcp.call("list_models", json!({ "media_type": "image" })).await;
    let id = models["structuredContent"]["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .find(|id| id.contains('/') && !id.starts_with("mock/"))
        .expect("a catalog id with a slash")
        .to_string();
    let result = mcp.call("get_model", json!({ "id": id })).await;
    assert!(!is_error(&result), "{result}");
    assert_eq!(result["structuredContent"]["id"], id.as_str());
}

#[tokio::test]
async fn bad_arguments_return_api_error() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let rpc = Mcp::new(&app, "/mcp")
        .rpc("tools/call", json!({ "name": "generate_image", "arguments": { "model": "mock/image-gen", "prompt": 5 } }))
        .await;
    assert!(rpc.error.is_none(), "a bad argument is a tool error, not a protocol error: {}", rpc.body);
    let result = rpc.result.unwrap();
    assert!(is_error(&result), "{result}");
    assert!(text_of(&result).starts_with("HTTP 4"), "{result}");
}

#[tokio::test]
async fn unknown_model_is_api_error() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let result = Mcp::new(&app, "/mcp").call("generate_image", json!({ "model": "nope/nope", "prompt": "x" })).await;
    assert!(is_error(&result), "{result}");
    let code = result["structuredContent"]["error"]["code"].as_str().expect("the API's error code").to_string();
    assert!(text_of(&result).contains(&code), "{result}");
}

#[tokio::test]
async fn hidden_tool_call_is_refused() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::SingleTenant).await;
    let key = mcp_support::create_key(&app, "generate").await;
    let result = Mcp::new(&app, "/admin/mcp").bearer(&key).call("list_request_logs", json!({})).await;
    assert!(is_error(&result), "{result}");
    assert_eq!(text_of(&result), "Unknown tool: list_request_logs");
}

#[tokio::test]
async fn budget_is_enforced_through_mcp() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::SingleTenant).await;
    let admin_key = mcp_support::create_key(&app, "generate,read,admin").await;
    let (key_id, key) = mcp_support::create_key_with_id(&app, "generate").await;

    let set = Mcp::new(&app, "/admin/mcp")
        .bearer(&admin_key)
        .call("set_key_limits", json!({ "id": key_id, "max_budget_usd": 0.0000001 }))
        .await;
    assert!(!is_error(&set), "{set}");

    let result = Mcp::new(&app, "/mcp")
        .bearer(&key)
        .call("generate_image", json!({ "model": "mock/expensive-image", "prompt": "x", "preview": false }))
        .await;
    assert!(is_error(&result), "{result}");
    assert!(text_of(&result).contains("budget_exceeded"), "{result}");
}

#[tokio::test]
async fn list_models_fits() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let result = Mcp::new(&app, "/mcp").call("list_models", json!({})).await;
    assert!(!is_error(&result), "{result}");
    let text = text_of(&result);
    assert!(text.len() <= 100_000, "list_models text is {} characters", text.len());
    assert!(!result["structuredContent"]["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn generation_is_tagged_mcp() {
    let (app, _db, _dir) = mcp_support::secured_app(Mode::SingleTenant).await;
    let key = mcp_support::create_key(&app, "generate,read").await;
    let result = Mcp::new(&app, "/mcp")
        .bearer(&key)
        .call("generate_image", json!({ "model": "mock/image-gen", "prompt": "a red cube", "preview": false, "tags": ["campaign-7"] }))
        .await;
    assert!(!is_error(&result), "{result}");

    // The API writes a request's log row and its tags after responding, so
    // give that background write a moment.
    let mut groups: Vec<String> = Vec::new();
    for _ in 0..50 {
        let report = mcp_support::http_get(&app, "/v1/spend/report?group_by=tag", mcp_support::MASTER_KEY).await;
        groups = report["data"].as_array().unwrap().iter().filter_map(|r| r["group"].as_str().map(str::to_string)).collect();
        if groups.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(groups.contains(&"mcp".to_string()), "{groups:?}");
    assert!(groups.contains(&"campaign-7".to_string()), "{groups:?}");
}

// ─── Task 7: images, assets and mesh files ────────────────────────────────────

fn blocks<'a>(result: &'a serde_json::Value, kind: &str) -> Vec<&'a serde_json::Value> {
    result["content"].as_array().into_iter().flatten().filter(|c| c["type"] == kind).collect()
}

#[tokio::test]
async fn generate_image_includes_preview_and_link() {
    use base64::Engine;
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    // mock/visual-image-gen renders a real PNG (mock/image-gen's 1x1 placeholder
    // is not decodable, so it gets no preview); 1024x1024 exercises the downscale.
    let result = Mcp::new(&app, "/mcp")
        .call("generate_image", json!({ "model": "mock/visual-image-gen", "prompt": "a red cube", "size": "1024x1024" }))
        .await;
    assert!(!is_error(&result), "{result}");

    let images = blocks(&result, "image");
    assert!((1..=2).contains(&images.len()), "{result}");
    for image in images {
        assert_eq!(image["mimeType"], "image/jpeg");
        let bytes = base64::engine::general_purpose::STANDARD.decode(image["data"].as_str().unwrap()).unwrap();
        let decoded = image::load_from_memory(&bytes).expect("a decodable JPEG preview");
        assert!(decoded.width() <= 512 && decoded.height() <= 512, "{}x{}", decoded.width(), decoded.height());
    }
    if let Some(url) = result["structuredContent"]["data"][0]["url"].as_str() {
        let links = blocks(&result, "resource_link");
        assert!(links.iter().any(|l| l["uri"] == url), "a link to {url}: {result}");
    }
    // Base64 image data never floods the text the model reads.
    assert!(text_of(&result).len() < 20_000, "{}", text_of(&result).len());
}

#[tokio::test]
async fn preview_false_has_no_image() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let result = Mcp::new(&app, "/mcp")
        .call("generate_image", json!({ "model": "mock/image-gen", "prompt": "a red cube", "preview": false }))
        .await;
    assert!(!is_error(&result), "{result}");
    assert!(blocks(&result, "image").is_empty(), "{result}");
}

#[tokio::test]
async fn convert_round_trip() {
    use base64::Engine;
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let triangle = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let result = Mcp::new(&app, "/mcp")
        .call(
            "convert_3d_model",
            json!({ "from": "obj", "to": "glb", "data_base64": base64::engine::general_purpose::STANDARD.encode(triangle) }),
        )
        .await;
    assert!(!is_error(&result), "{result}");
    let resource = blocks(&result, "resource");
    assert_eq!(resource.len(), 1, "{result}");
    let blob = resource[0]["resource"]["blob"].as_str().unwrap();
    let bytes = base64::engine::general_purpose::STANDARD.decode(blob).unwrap();
    assert_eq!(&bytes[..4], b"glTF", "a binary glTF");
    assert_eq!(resource[0]["resource"]["mimeType"], "model/gltf-binary");
}

#[tokio::test]
async fn convert_refuses_two_sources_or_none() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let result = Mcp::new(&app, "/mcp").call("convert_3d_model", json!({ "from": "obj", "to": "glb" })).await;
    assert!(is_error(&result), "{result}");
    assert!(text_of(&result).contains("source_url") && text_of(&result).contains("data_base64"), "{result}");
}

#[tokio::test]
async fn three_d_status_links_assets() {
    let (app, _db, _dir) = mcp_support::app(|_| {}).await;
    let id = harness::submit(&app, "mock/mesh-3d", "a fox").await;
    harness::poll_until_terminal(&app, &id).await;
    let result = Mcp::new(&app, "/mcp").call("get_3d_model_status", json!({ "id": id })).await;
    assert!(!is_error(&result), "{result}");
    let assets = result["structuredContent"]["assets"].as_array().cloned().unwrap_or_default();
    assert!(!assets.is_empty(), "{result}");
    let links = blocks(&result, "resource_link");
    assert_eq!(links.len(), assets.len(), "{result}");
    for (link, asset) in links.iter().zip(&assets) {
        assert_eq!(link["uri"], asset["url"]);
    }
}
