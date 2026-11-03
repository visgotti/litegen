//! Unit tests for the MCP catalog and its gates.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use uuid::Uuid;

use crate::api::middleware::{AppState, KeyContext, Scope, DEFAULT_APP_ID, DEFAULT_ORG_ID};
use crate::config::Mode;
use crate::mcp::auth::{Caller, CallerKind};
use crate::mcp::catalog::{Surfaces, EXCLUDED, TOOLS};
use crate::mcp::gate::visible;
use crate::mcp::Surface;

// ─── Catalog ↔ OpenAPI contract ───────────────────────────────────────────────

fn openapi_operations() -> HashSet<(String, String)> {
    use utoipa::OpenApi;
    let doc = serde_json::to_value(crate::api::openapi::ApiDoc::openapi()).unwrap();
    let mut ops = HashSet::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for method in ["get", "post", "put", "patch", "delete"] {
            if item.get(method).is_some() {
                ops.insert((method.to_uppercase(), path.clone()));
            }
        }
    }
    ops
}

#[test]
fn every_openapi_operation_is_a_tool_or_excluded() {
    let ops = openapi_operations();
    let mut listed = HashSet::new();
    for d in TOOLS {
        assert!(listed.insert((d.method.as_str().to_string(), d.path.to_string())), "listed twice: {} {}", d.method.as_str(), d.path);
    }
    for e in EXCLUDED {
        assert!(listed.insert((e.method.as_str().to_string(), e.path.to_string())), "listed twice: {} {}", e.method.as_str(), e.path);
    }
    let mut missing: Vec<_> = ops.difference(&listed).collect();
    missing.sort();
    assert!(missing.is_empty(), "OpenAPI operations that are neither a tool nor excluded: {missing:?}");
    let mut stale: Vec<_> = listed.difference(&ops).collect();
    stale.sort();
    assert!(stale.is_empty(), "catalog rows naming operations the API does not have: {stale:?}");
    assert_eq!(TOOLS.len(), 111);
    assert_eq!(EXCLUDED.len(), 24);
}

fn portable_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    (3..=41).contains(&name.len())
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[test]
fn tool_names_are_valid_and_unique() {
    let mut seen = HashSet::new();
    for d in TOOLS {
        assert!(portable_tool_name(d.name), "tool name not portable: {}", d.name);
        assert!(seen.insert(d.name), "duplicate tool name: {}", d.name);
    }
    assert!(!portable_tool_name("Generate"), "the check itself must reject capitals");
    assert!(!portable_tool_name("a-b"), "the check itself must reject dashes");
}

#[test]
fn toolsets_are_known() {
    for d in TOOLS {
        let ok = match d.surfaces {
            Surfaces::Ai => Surface::Ai.toolsets().contains(&d.toolset),
            Surfaces::Admin => Surface::Admin.toolsets().contains(&d.toolset),
            Surfaces::Both => Surface::Ai.toolsets().contains(&d.toolset) && Surface::Admin.toolsets().contains(&d.toolset),
        };
        assert!(ok, "{}: toolset {:?} is not offered on its endpoint", d.name, d.toolset);
    }
}

// ─── Gates mirror the routes' own checks ──────────────────────────────────────

struct NoopStorage;

#[async_trait]
impl crate::proxy::materializer::TempStorage for NoopStorage {
    async fn put(&self, key: &str, _bytes: Bytes, _ct: &str) -> Result<String, crate::proxy::materializer::MaterializeError> {
        Ok(format!("local://{key}"))
    }
    async fn delete(&self, _key: &str) -> Result<(), crate::proxy::materializer::MaterializeError> {
        Ok(())
    }
}

async fn state(mode: Mode, master_key: Option<&str>) -> Arc<AppState> {
    use crate::config::{AppConfig, CacheGlobalConfig};
    let db = Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let router = Arc::new(crate::proxy::router::ProxyRouter::new(
        Arc::new(crate::proxy::registry::ProviderRegistry::new()),
        Arc::new(crate::proxy::cache::GenerationCache::new(&CacheGlobalConfig::default())),
        Arc::new(AppConfig::default()),
        Arc::new(crate::proxy::storage::LocalStore),
    ));
    let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    models.pop();
    models.push("models");
    Arc::new(AppState {
        router,
        db,
        master_key: master_key.map(str::to_string),
        registry: Arc::new(crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap()),
        materializer: Arc::new(crate::proxy::materializer::Materializer::new(Arc::new(NoopStorage), reqwest::Client::new())),
        rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
        in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
        oauth: crate::auth::oauth::OAuthConfig::default(),
        mode,
        secrets_key: None,
        dev: crate::config::DevFlags::default(),
        allow_password: true,
    })
}

fn key(scopes: &[Scope]) -> KeyContext {
    KeyContext {
        key_id: Some(Uuid::new_v4()),
        scopes: scopes.to_vec(),
        rpm_limit: None,
        quota_remaining: None,
        webhook_url: None,
        user: None,
        permissions: vec![],
        session_id: None,
        org_id: Some(DEFAULT_ORG_ID.to_string()),
        app_id: Some(DEFAULT_APP_ID.to_string()),
        oauth_grant_id: None,
    }
}

/// The master key's context (and dev mode's): no key, no user, no session.
fn master(org: bool) -> KeyContext {
    KeyContext {
        key_id: None,
        scopes: vec![Scope::Generate, Scope::Read, Scope::Admin],
        org_id: org.then(|| DEFAULT_ORG_ID.to_string()),
        app_id: org.then(|| DEFAULT_APP_ID.to_string()),
        ..key(&[])
    }
}

fn caller(ctx: KeyContext, kind: CallerKind, surface: Surface) -> Caller {
    Caller { ctx, kind, authorization: None, surface, toolsets: None }
}

/// A person with `role` in the default org, signed in through OAuth with a
/// grant holding `scopes`.
fn oauth(role: crate::types::Role, scopes: &[Scope]) -> KeyContext {
    KeyContext {
        key_id: None,
        user: Some(crate::api::middleware::UserContext { user_id: "u1".into(), email: "u1@example.com".into(), role }),
        permissions: crate::auth::permissions::permissions_for(role).to_vec(),
        oauth_grant_id: Some("grant-1".into()),
        ..key(scopes)
    }
}

#[tokio::test]
async fn oauth_principals_see_what_their_grant_allows() {
    use crate::types::Role;
    let single = state(Mode::SingleTenant, Some("mk")).await;
    let hosted = state(Mode::Hosted, Some("mk")).await;
    let admin = |ctx: KeyContext| caller(ctx, CallerKind::OAuth, Surface::Admin);

    let reader = admin(oauth(Role::Owner, &[Scope::Read]));
    for read in ["get_org", "list_members", "get_spend_report", "list_orgs"] {
        assert!(sees(&reader, &hosted, read), "{read}");
    }
    for write in ["update_org", "set_org_limits", "revoke_api_key", "remove_member"] {
        assert!(!sees(&reader, &hosted, write), "{write} needs admin:write");
    }
    assert!(sees(&reader, &single, "list_users"), "a global read");
    assert!(!sees(&reader, &single, "delete_user"), "a global write needs admin:write");

    let writer = admin(oauth(Role::Owner, &[Scope::Admin]));
    assert!(sees(&writer, &hosted, "get_org"), "admin covers reads, as the API's role checks have it");
    assert!(sees(&writer, &hosted, "update_org"));
    assert!(sees(&writer, &single, "delete_user"));

    let member = admin(oauth(Role::Member, &[Scope::Read, Scope::Admin]));
    assert!(!sees(&member, &hosted, "update_org"), "the role still decides");
}

fn sees(caller: &Caller, state: &AppState, tool: &str) -> bool {
    let def = TOOLS.iter().find(|d| d.name == tool).unwrap_or_else(|| panic!("no tool {tool}"));
    visible(def, caller, state)
}

fn visible_names(caller: &Caller, state: &AppState) -> Vec<&'static str> {
    TOOLS.iter().filter(|d| visible(d, caller, state)).map(|d| d.name).collect()
}

#[tokio::test]
async fn gates_match_principals() {
    let single = state(Mode::SingleTenant, Some("mk")).await;
    let hosted = state(Mode::Hosted, Some("mk")).await;
    use CallerKind::*;
    use Scope::*;
    let ai = |ctx: KeyContext, kind| caller(ctx, kind, Surface::Ai);
    let admin = |ctx: KeyContext, kind| caller(ctx, kind, Surface::Admin);

    // A `generate` key is an AI-only key: the AI tools, catalog included, and
    // none of the management reads.
    assert!(sees(&ai(key(&[Generate]), ApiKey), &single, "generate_image"));
    assert!(sees(&ai(key(&[Generate]), ApiKey), &single, "list_models"));
    assert!(!sees(&admin(key(&[Generate]), ApiKey), &single, "list_request_logs"));
    assert!(!sees(&admin(key(&[Generate]), ApiKey), &single, "get_spend_report"));

    // `read` reads, everywhere, and cannot generate.
    assert!(sees(&ai(key(&[Read]), ApiKey), &single, "list_models"));
    assert!(sees(&admin(key(&[Read]), ApiKey), &single, "get_spend_report"));
    assert!(!sees(&ai(key(&[Read]), ApiKey), &single, "generate_image"));
    assert!(!sees(&admin(key(&[Read]), ApiKey), &single, "set_org_limits"));

    // `admin` alone: require_org lets admin read too, check_scope(Read) does not.
    assert!(sees(&admin(key(&[Admin]), ApiKey), &single, "set_org_limits"));
    assert!(sees(&admin(key(&[Admin]), ApiKey), &single, "get_org_limits"));
    assert!(!sees(&admin(key(&[Admin]), ApiKey), &single, "list_request_logs"));
    assert!(sees(&admin(key(&[Generate, Read, Admin]), ApiKey), &single, "create_api_key"));

    // Org, app and member management needs a signed-in user: no key sees it.
    for tool in ["list_orgs", "list_members"] {
        assert!(!sees(&admin(key(&[Generate, Read, Admin]), ApiKey), &single, tool), "{tool}");
    }

    // Hosted master key: the platform admin, bound to no tenant.
    let platform = admin(master(false), MasterKey);
    assert!(sees(&platform, &hosted, "list_gateway_models"));
    assert!(sees(&platform, &hosted, "list_users"));
    let on_ai = visible_names(&ai(master(false), MasterKey), &hosted);
    assert!(!on_ai.is_empty());
    for name in &on_ai {
        let def = TOOLS.iter().find(|d| d.name == *name).unwrap();
        assert_eq!(def.toolset, "catalog", "{name} needs a tenant, which the hosted master key lacks");
    }

    // Single-tenant master key: no permissions, so the global user routes refuse it.
    assert!(!sees(&admin(master(true), MasterKey), &single, "list_users"));
    assert!(sees(&admin(master(true), MasterKey), &single, "list_gateway_models"));

    // Dev mode carries the same context as the single-tenant master key.
    assert!(sees(&ai(master(true), Dev), &single, "generate_image"));

    // A tool lives on its own endpoint only, and ?toolsets= narrows the list.
    assert!(!sees(&admin(key(&[Generate]), ApiKey), &single, "generate_image"));
    let mut images_only = ai(key(&[Generate, Read]), ApiKey);
    images_only.toolsets = Some(vec!["images".into()]);
    assert_eq!(visible_names(&images_only, &single), vec!["estimate_image_cost", "generate_image", "segment_image"]);
}

// ─── Input schemas from the OpenAPI document ──────────────────────────────────

use crate::mcp::schema::{build, ArgPlan, BodyPlan};
use serde_json::{json, Map, Value};

fn openapi_doc() -> Value {
    use utoipa::OpenApi;
    serde_json::to_value(crate::api::openapi::ApiDoc::openapi()).unwrap()
}

fn tool_schema(doc: &Value, name: &str) -> (Map<String, Value>, ArgPlan) {
    let def = TOOLS.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("no tool {name}"));
    build(doc, def).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn schemas_have_no_refs_and_object_roots() {
    let doc = openapi_doc();
    for def in TOOLS {
        let (schema, _) = build(&doc, def).unwrap_or_else(|e| panic!("{}: {e}", def.name));
        let text = serde_json::to_string(&schema).unwrap();
        assert!(!text.contains("\"$ref\""), "{} still has a $ref", def.name);
        assert_eq!(schema.get("type"), Some(&json!("object")), "{}", def.name);
        for keyword in ["allOf", "oneOf", "anyOf"] {
            assert!(!schema.contains_key(keyword), "{} has a root {keyword}", def.name);
        }
    }
}

#[test]
fn schemas_validate_against_2020_12() {
    let doc = openapi_doc();
    for def in TOOLS {
        let (schema, _) = build(&doc, def).unwrap();
        let mut schema = Value::Object(schema);
        schema["$schema"] = json!("https://json-schema.org/draft/2020-12/schema");
        if let Err(e) = jsonschema::meta::validate(&schema) {
            panic!("{} is not valid JSON Schema 2020-12: {e}", def.name);
        }
        if let Err(e) = jsonschema::draft202012::new(&schema) {
            panic!("{} does not compile as JSON Schema 2020-12: {e}", def.name);
        }
    }
}

#[test]
fn property_names_are_portable() {
    let portable = |n: &str| (1..=64).contains(&n.len()) && n.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    let doc = openapi_doc();
    for def in TOOLS {
        let (schema, _) = build(&doc, def).unwrap();
        for name in schema["properties"].as_object().unwrap().keys() {
            assert!(portable(name), "{}: property {name:?} would make Claude Code drop the tool", def.name);
        }
    }
    assert!(!portable("Idempotency Key"), "the check itself must reject spaces");
}

#[test]
fn generate_image_flattens_body() {
    let doc = openapi_doc();
    let (schema, plan) = tool_schema(&doc, "generate_image");
    let props = schema["properties"].as_object().unwrap();
    for p in ["model", "prompt", "n", "idempotency_key", "end_user", "tags", "preview"] {
        assert!(props.contains_key(p), "generate_image lacks {p}");
    }
    // Exactly what the OpenAPI schema requires: BaseGenerationRequest needs only `model`.
    assert_eq!(schema["required"], json!(["model"]));
    // A nullable field (`["string","null"]` in OpenAPI) reaches clients as plain `string`.
    assert_eq!(props["aspect_ratio"]["type"], json!("string"));
    match &plan.body {
        BodyPlan::Flattened(fields) => {
            assert!(fields.contains(&"prompt".to_string()));
            assert!(!fields.contains(&"idempotency_key".to_string()), "a header is not a body field");
        }
        other => panic!("generate_image body should be flattened, got {other:?}"),
    }
    let headers: Vec<(&str, &str)> = plan.headers.iter().map(|(a, h)| (a.as_str(), *h)).collect();
    assert!(headers.contains(&("idempotency_key", "idempotency-key")), "{headers:?}");
    assert!(headers.contains(&("end_user", "x-litegen-end-user")), "{headers:?}");
    assert!(headers.contains(&("tags", "x-litegen-tags")), "{headers:?}");
    assert_eq!(plan.local, vec!["preview".to_string()], "preview is MCP's own argument, never sent to the API");
}

#[test]
fn nullable_refs_collapse_to_the_referenced_schema() {
    let doc = openapi_doc();
    let (schema, _) = tool_schema(&doc, "generate_image");
    let props = schema["properties"].as_object().unwrap();
    // OpenAPI: `oneOf: [{"type":"null"}, {"$ref": "ImageBackground"}]`.
    assert!(props["background"].get("oneOf").is_none(), "{}", props["background"]);
    assert_eq!(props["background"]["enum"], json!(["transparent", "opaque"]));
    assert_eq!(props["expand"]["type"], json!("object"));
    // A client that validates arguments must accept a documented value.
    let validator = jsonschema::draft202012::new(&Value::Object(schema.clone())).unwrap();
    let args = json!({ "model": "mock/image-gen", "background": "transparent", "output_format": "png" });
    assert!(validator.is_valid(&args), "{:?}", validator.iter_errors(&args).map(|e| e.to_string()).collect::<Vec<_>>());
    assert!(!validator.is_valid(&json!({ "model": "mock/image-gen", "background": 42 })));
}

/// An empty schema accepts everything: in a `oneOf` it makes every value that
/// matches another member match two, so the union rejects all valid values.
fn empty_union_members(value: &Value, path: &str, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "oneOf" || key == "anyOf" {
                    for (i, member) in child.as_array().into_iter().flatten().enumerate() {
                        if member.as_object().is_some_and(Map::is_empty) {
                            found.push(format!("{path}.{key}[{i}]"));
                        }
                    }
                }
                empty_union_members(child, &format!("{path}.{key}"), found);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                empty_union_members(item, &format!("{path}[{i}]"), found);
            }
        }
        _ => {}
    }
}

#[test]
fn no_union_has_an_empty_member() {
    let doc = openapi_doc();
    let mut found = Vec::new();
    for def in TOOLS {
        let (schema, _) = build(&doc, def).unwrap();
        empty_union_members(&Value::Object(schema), def.name, &mut found);
    }
    assert!(found.is_empty(), "{} empty union members, e.g. {:?}", found.len(), &found[..found.len().min(5)]);
}

#[test]
fn catch_all_ids_say_they_may_contain_slashes() {
    let doc = openapi_doc();
    let (schema, plan) = tool_schema(&doc, "get_model");
    assert_eq!(plan.path, vec!["id".to_string()]);
    let description = schema["properties"]["id"]["description"].as_str().unwrap_or("");
    assert!(description.contains("may contain '/'"), "{description:?}");
}

#[test]
fn convert_takes_url_or_base64() {
    let doc = openapi_doc();
    let (schema, plan) = tool_schema(&doc, "convert_3d_model");
    let props = schema["properties"].as_object().unwrap();
    assert!(props.contains_key("source_url") && props.contains_key("data_base64"), "{props:?}");
    assert!(plan.binary_in);
    assert!(plan.binary_out, "the converted mesh comes back as bytes");
    let (_, json_tool) = tool_schema(&doc, "list_models");
    assert!(!json_tool.binary_in && !json_tool.binary_out);
}

#[test]
fn ai_surface_schema_budget() {
    let doc = openapi_doc();
    let total: usize = TOOLS
        .iter()
        .filter(|d| d.surfaces.includes(Surface::Ai))
        .map(|d| serde_json::to_string(&build(&doc, d).unwrap().0).unwrap().len())
        .sum();
    assert!(total < 90_000, "the AI tools' schemas total {total} characters");
}

// ─── Dispatch: tool arguments → the API request ───────────────────────────────

use crate::mcp::dispatch::build_request;
use crate::mcp::registry::SpecKind;

fn dev_caller(surface: Surface) -> Caller {
    caller(master(true), CallerKind::Dev, surface)
}

fn request_for(tool: &str, args: Value) -> axum::http::Request<axum::body::Body> {
    let registry = crate::mcp::tool_registry();
    let spec = registry.get(tool).unwrap_or_else(|| panic!("no tool {tool}"));
    let SpecKind::Http(plan) = &spec.kind else { panic!("{tool} is not an HTTP tool") };
    let surface = if spec.def.surfaces.includes(Surface::Ai) { Surface::Ai } else { Surface::Admin };
    build_request(spec, plan, &dev_caller(surface), args.as_object().unwrap()).unwrap_or_else(|e| panic!("{tool}: {e}"))
}

async fn body_json(req: axum::http::Request<axum::body::Body>) -> Value {
    let bytes = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

#[test]
fn build_request_keeps_slash_in_catch_all() {
    let req = request_for("get_model", json!({ "id": "fal/flux-pro" }));
    assert_eq!(req.method(), axum::http::Method::GET);
    assert_eq!(req.uri().path(), "/v1/models/fal/flux-pro");
}

#[test]
fn build_request_encodes_regular_params() {
    let req = request_for("get_end_user", json!({ "end_user_id": "a b/c" }));
    assert_eq!(req.uri().path(), "/v1/end-users/a%20b%2Fc");
}

#[test]
fn build_request_names_a_missing_path_parameter() {
    let registry = crate::mcp::tool_registry();
    let spec = registry.get("get_end_user").unwrap();
    let SpecKind::Http(plan) = &spec.kind else { unreachable!() };
    let err = build_request(spec, plan, &dev_caller(Surface::Admin), &Map::new()).unwrap_err();
    assert!(err.contains("end_user_id"), "{err}");
}

#[tokio::test]
async fn tags_always_include_mcp() {
    let req = request_for("generate_image", json!({ "model": "m", "prompt": "p", "tags": ["x"], "end_user": "cust-1", "idempotency_key": "k1", "preview": false }));
    assert_eq!(req.headers()["x-litegen-tags"], "mcp,x");
    assert_eq!(req.headers()["x-litegen-end-user"], "cust-1");
    assert_eq!(req.headers()["idempotency-key"], "k1");
    assert_eq!(req.uri().path(), "/v1/images/generations");
    let body = body_json(req).await;
    assert_eq!(body, json!({ "model": "m", "prompt": "p" }), "headers and MCP's own arguments stay out of the body");

    let req = request_for("generate_image", json!({ "model": "m", "prompt": "p" }));
    assert_eq!(req.headers()["x-litegen-tags"], "mcp");
}

#[tokio::test]
async fn unknown_arguments_reach_the_api_for_body_tools() {
    let body = body_json(request_for("generate_image", json!({ "model": "m", "prompt": "p", "not_a_field": 1 }))).await;
    assert_eq!(body["not_a_field"], 1, "the API, not MCP, decides what an unknown field means");
    let registry = crate::mcp::tool_registry();
    let spec = registry.get("list_end_users").unwrap();
    let SpecKind::Http(plan) = &spec.kind else { unreachable!() };
    let err = build_request(spec, plan, &dev_caller(Surface::Admin), json!({ "nope": 1 }).as_object().unwrap()).unwrap_err();
    assert!(err.contains("nope"), "{err}");
}

#[test]
fn query_parameters_go_in_the_query_string() {
    let req = request_for("list_end_users", json!({ "org_id": "org 1" }));
    assert_eq!(req.uri().path(), "/v1/end-users");
    assert_eq!(req.uri().query(), Some("org_id=org%201"));
}

// ─── Results that fit what clients accept ─────────────────────────────────────

use crate::mcp::content::fit;

#[test]
fn fit_leaves_small_results_alone() {
    let value = json!({ "data": [1, 2, 3] });
    assert_eq!(fit("list_generations", value.clone(), 100_000), value);
}

#[test]
fn fit_halves_the_largest_list_and_says_so() {
    let big: Vec<Value> = (0..1000).map(|i| json!({ "id": i, "pad": "x".repeat(50) })).collect();
    let fitted = fit("list_request_logs", json!({ "data": big, "total": 1000 }), 20_000);
    assert!(serde_json::to_string(&fitted).unwrap().len() <= 20_000);
    let shown = fitted["data"].as_array().unwrap().len();
    assert!(shown > 0 && shown < 1000);
    assert_eq!(fitted["_truncated"]["shown"], shown);
    assert_eq!(fitted["_truncated"]["total"], 1000);
    assert!(fitted["_truncated"]["hint"].as_str().unwrap().contains("page"));
}

#[test]
fn fit_compacts_list_models_first() {
    let model = json!({ "id": "a/b", "name": "B", "provider": "a", "media_type": "image", "status": "active", "is_available": true, "tags": ["t"], "description": "d".repeat(2000), "pricing": { "x": 1 } });
    let fitted = fit("list_models", json!({ "object": "list", "data": vec![model; 100] }), 100_000);
    let first = &fitted["data"][0];
    assert_eq!(first["id"], "a/b");
    assert!(first.get("description").is_none() && first.get("pricing").is_none());
    assert!(fitted["_compact"].as_str().unwrap().contains("get_model"));
}

/// `fetch_image` checks a preview URL with `validate_public_url` and then
/// fetches it; a client that followed redirects would let a public host
/// 3xx-redirect the fetch into a private address (util::ssrf's contract).
#[tokio::test]
async fn preview_client_does_not_follow_redirects() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    let target = format!("{}/private", server.uri());
    Mock::given(path("/public"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", target.as_str()))
        .mount(&server)
        .await;
    Mock::given(path("/private")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"secret".to_vec())).mount(&server).await;

    let resp = crate::mcp::preview_client().get(format!("{}/public", server.uri())).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 302, "the preview client followed a redirect");
}

// ─── wait_for_generation ──────────────────────────────────────────────────────

#[test]
fn status_paths() {
    use crate::mcp::wait::status_path;
    let maker = caller(key(&[Scope::Generate]), CallerKind::ApiKey, Surface::Ai);
    assert_eq!(status_path("litegen-vid-1234", &maker), "/v1/videos/litegen-vid-1234");
    assert_eq!(status_path("litegen-3d-abcd", &maker), "/v1/models3d/litegen-3d-abcd");
    assert_eq!(status_path("litegen-img-9", &maker), "/v1/generations/litegen-img-9");
    assert_eq!(status_path("a b/c", &maker), "/v1/generations/a%20b%2Fc", "an id is one path segment");

    // Only `generate` may move a job along; any other key reads its record.
    let reader = caller(key(&[Scope::Read]), CallerKind::ApiKey, Surface::Ai);
    assert_eq!(status_path("litegen-vid-1234", &reader), "/v1/generations/litegen-vid-1234");
    assert_eq!(status_path("litegen-3d-abcd", &reader), "/v1/generations/litegen-3d-abcd");
    assert_eq!(status_path("litegen-vid-1234", &dev_caller(Surface::Ai)), "/v1/videos/litegen-vid-1234");
}

#[test]
fn waiting_slows_down_and_heeds_the_api() {
    use crate::mcp::dispatch::HttpReply;
    use crate::mcp::wait::next_delay;
    use std::time::Duration;
    let reply = |status: u16, headers: &[(&str, &str)]| {
        let mut map = axum::http::HeaderMap::new();
        for (name, value) in headers {
            map.insert(axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(), value.parse().unwrap());
        }
        HttpReply { status: axum::http::StatusCode::from_u16(status).unwrap(), headers: map, body: Bytes::new() }
    };
    let ok = reply(200, &[]);
    assert_eq!(next_delay(1, &ok), Duration::from_secs(2));
    assert_eq!(next_delay(2, &ok), Duration::from_secs(3));
    assert_eq!(next_delay(3, &ok), Duration::from_millis(4500));
    assert_eq!(next_delay(200, &ok), Duration::from_secs(10), "at most ten seconds apart");

    // A 429 or 503 is sat out for as long as it asks, never less than the schedule.
    assert_eq!(next_delay(1, &reply(429, &[("retry-after", "7")])), Duration::from_secs(7));
    assert_eq!(next_delay(1, &reply(503, &[("retry-after", "1")])), Duration::from_secs(2));
    assert_eq!(next_delay(1, &reply(429, &[("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT")])), Duration::from_secs(2), "a date is ignored");

    // A rate-limited key's reads spend at most half its requests per minute.
    assert_eq!(next_delay(1, &reply(200, &[("x-ratelimit-limit", "6")])), Duration::from_secs(20));
    assert_eq!(next_delay(1, &reply(200, &[("x-ratelimit-limit", "6000")])), Duration::from_secs(2));
    assert_eq!(next_delay(1, &reply(200, &[("x-ratelimit-limit", "0")])), Duration::from_secs(2), "0 is no limit");
}

#[test]
fn a_stored_record_links_its_files() {
    use crate::mcp::content::links;
    let link_json = |tool: &str, record: Value| -> Vec<Value> {
        links(tool, &record).iter().map(|b| serde_json::to_value(b).unwrap()).collect()
    };

    let video = json!({
        "media_type": "video",
        "result_url": "https://cdn.example/v.mp4",
        "metadata": { "assets": [{ "url": "https://cdn.example/v.mp4", "format": "mp4", "kind": "video", "size_bytes": 10 }] }
    });
    let linked = link_json("get_generation", video);
    assert_eq!(linked.len(), 1, "the asset, not the same file twice: {linked:?}");
    assert_eq!(linked[0]["mimeType"], "video/mp4");

    // A record with no asset list still links its result.
    let image = json!({ "media_type": "image", "result_url": "https://cdn.example/i.png" });
    let linked = link_json("wait_for_generation", image);
    assert_eq!(linked.len(), 1, "{linked:?}");
    assert_eq!(linked[0]["uri"], "https://cdn.example/i.png");
    assert_eq!(linked[0]["mimeType"], "image/png");
}

// ─── The generated tool reference ─────────────────────────────────────────────

#[test]
fn tools_reference_is_current() {
    let registry = crate::mcp::registry::Registry::build(&openapi_doc()).unwrap();
    let rendered = crate::mcp::registry::render_reference(&registry);
    let committed = include_str!("../../../docs/mcp/tools.md");
    assert!(
        rendered == committed,
        "docs/mcp/tools.md is stale: from litegen-core, run `cargo run --example mcp_tools_doc > ../docs/mcp/tools.md`"
    );
}

#[test]
fn tools_reference_covers_every_tool() {
    let registry = crate::mcp::registry::Registry::build(&openapi_doc()).unwrap();
    let rendered = crate::mcp::registry::render_reference(&registry);
    for spec in registry.specs() {
        assert!(rendered.contains(&format!("\n### `{}`\n", spec.def.name)), "{} has no section", spec.def.name);
    }
    assert!(rendered.contains("`POST /v1/images/generations`"));
    assert!(rendered.contains("platform admin"), "gates are described in words");
}
