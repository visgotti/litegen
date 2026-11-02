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
    assert_eq!(TOOLS.len(), 112);
    assert_eq!(EXCLUDED.len(), 20);
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
    for tool in ["list_orgs", "create_org"] {
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
