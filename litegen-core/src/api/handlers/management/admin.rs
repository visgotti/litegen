//! `/v1/admin/*`: runtime model, provider, routing and gateway configuration
//! (LiteLLM `/model/*`, `/credentials`, router settings, `/config/update`).
//! Gateway-admin only; every change applies to the next request without a
//! restart and is persisted for the next boot.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;

use super::{audit, bad_request, ctx_of, internal, not_found, require_gateway_admin, store, DeletedResponse};
use crate::api::middleware::{AppState, KeyContext};
use crate::capabilities::schema::ModelSchema;
use crate::management::runtime;
use crate::management::store::ManagementStore;
use crate::management::types::*;
use crate::types::ErrorResponse;

fn admin<'a>(state: &'a AppState, ctx: Option<Extension<KeyContext>>) -> Result<(KeyContext, &'a dyn ManagementStore), Response> {
    let ctx = ctx_of(ctx)?;
    require_gateway_admin(state, &ctx)?;
    let store = store(state)?;
    Ok((ctx, store))
}

macro_rules! admin_or_return {
    ($state:ident, $ctx:ident) => {
        match admin(&$state, $ctx) {
            Ok(v) => v,
            Err(r) => return r,
        }
    };
}

// ─── Models ──────────────────────────────────────────────────────────────────

fn model_view(state: &AppState, id: &str) -> Option<AdminModelView> {
    let (schema, source) = match state.registry.get(id) {
        Some(m) => (m.clone(), ModelSource::Catalog),
        None => (state.registry.runtime_model(id)?, ModelSource::Runtime),
    };
    Some(AdminModelView { id: id.to_string(), source, disabled: state.registry.is_disabled(id), schema })
}

/// GET /v1/admin/models — Every model (catalog and runtime, enabled or not).
#[utoipa::path(
    operation_id = "admin_list_models",
    get,
    path = "/v1/admin/models",
    responses(
        (status = 200, description = "All models with management state", body = AdminModelListResponse),
        (status = 403, description = "Gateway admin required", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn list_models(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>) -> Response {
    let _ = admin_or_return!(state, ctx);
    let mut ids: Vec<String> = state.registry.all().map(|m| m.id.clone()).collect();
    ids.extend(state.registry.runtime_models().into_iter().map(|m| m.id));
    ids.sort();
    let data = ids.iter().filter_map(|id| model_view(&state, id)).collect();
    (StatusCode::OK, Json(AdminModelListResponse { data })).into_response()
}

/// GET /v1/admin/models/{id}
#[utoipa::path(
    operation_id = "admin_get_model",
    get,
    path = "/v1/admin/models/{id}",
    params(("id" = String, Path, description = "Model id (may contain slashes)")),
    responses(
        (status = 200, description = "Model with management state", body = AdminModelView),
        (status = 404, description = "Unknown model", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn get_model(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(id): Path<String>) -> Response {
    let _ = admin_or_return!(state, ctx);
    match model_view(&state, &id) {
        Some(v) => (StatusCode::OK, Json(v)).into_response(),
        None => not_found("model_not_found", &format!("model '{id}' not found")),
    }
}

fn parse_model(body: serde_json::Value) -> Result<ModelSchema, Response> {
    serde_json::from_value(body).map_err(|e| bad_request("invalid_model", &format!("invalid model schema: {e}")))
}

async fn save_model(state: &AppState, ctx: &KeyContext, store: &dyn ManagementStore, m: ModelSchema, created: bool) -> Response {
    if let Err(msg) = runtime::validate_runtime_model(state, &m) {
        return bad_request("invalid_model", &msg);
    }
    let json = match serde_json::to_string(&m) { Ok(j) => j, Err(e) => return internal(&e.to_string()) };
    if let Err(e) = store.upsert_runtime_model(&m.id, &json, Some(&super::actor_label(ctx))).await {
        return internal(&e.to_string());
    }
    let id = m.id.clone();
    state.registry.upsert_runtime(m);
    audit(state, ctx, None, if created { "model.create" } else { "model.update" }, "model", &id, None);
    let status = if created { StatusCode::CREATED } else { StatusCode::OK };
    match model_view(state, &id) {
        Some(v) => (status, Json(v)).into_response(),
        None => internal("saved model vanished from the registry"),
    }
}

/// POST /v1/admin/models — Add a model at runtime (LiteLLM `/model/new`).
///
/// The body is a full model schema, the same shape as a `models/*.yaml` row,
/// and passes the same validation. The vendor model called is the part of the
/// id after `<provider>/` (or the provider's `model_mapping`).
#[utoipa::path(
    operation_id = "admin_create_model",
    post,
    path = "/v1/admin/models",
    request_body = crate::capabilities::ModelSchema,
    responses(
        (status = 201, description = "Model created", body = AdminModelView),
        (status = 400, description = "Invalid model", body = ErrorResponse),
        (status = 409, description = "Model id already exists", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn create_model(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let (ctx, store) = admin_or_return!(state, ctx);
    let m = match parse_model(body) { Ok(m) => m, Err(r) => return r };
    if state.registry.is_catalog(&m.id) || state.registry.runtime_model(&m.id).is_some() {
        return super::err(StatusCode::CONFLICT, "model_exists", &format!("model '{}' already exists", m.id));
    }
    save_model(&state, &ctx, store, m, true).await
}

/// PUT /v1/admin/models/{id} — Replace a runtime model's schema (LiteLLM `/model/update`).
#[utoipa::path(
    operation_id = "admin_replace_model",
    put,
    path = "/v1/admin/models/{id}",
    params(("id" = String, Path, description = "Model id (may contain slashes)")),
    request_body = crate::capabilities::ModelSchema,
    responses(
        (status = 200, description = "Model replaced", body = AdminModelView),
        (status = 400, description = "Invalid model, or id mismatch", body = ErrorResponse),
        (status = 404, description = "No such runtime model", body = ErrorResponse),
        (status = 409, description = "Catalog models are read-only", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn replace_model(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let (ctx, store) = admin_or_return!(state, ctx);
    if state.registry.is_catalog(&id) {
        return super::err(StatusCode::CONFLICT, "catalog_model_read_only", "catalog models are defined in models/*.yaml; disable it with PATCH instead");
    }
    if state.registry.runtime_model(&id).is_none() {
        return not_found("model_not_found", &format!("runtime model '{id}' not found"));
    }
    let m = match parse_model(body) { Ok(m) => m, Err(r) => return r };
    if m.id != id {
        return bad_request("invalid_model", "the body's id must match the path; delete and re-create to rename");
    }
    save_model(&state, &ctx, store, m, false).await
}

/// PATCH /v1/admin/models/{id} — Enable or disable any model platform-wide
/// (LiteLLM `/model/block`). Disabled models disappear from `/v1/models` and
/// generation requests for them answer 404 `model_not_found`.
#[utoipa::path(
    operation_id = "admin_patch_model",
    patch,
    path = "/v1/admin/models/{id}",
    params(("id" = String, Path, description = "Model id (may contain slashes)")),
    request_body = PatchModelStateRequest,
    responses(
        (status = 200, description = "Model state updated", body = AdminModelView),
        (status = 404, description = "Unknown model", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn patch_model(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Json(body): Json<PatchModelStateRequest>,
) -> Response {
    let (ctx, store) = admin_or_return!(state, ctx);
    if model_view(&state, &id).is_none() {
        return not_found("model_not_found", &format!("model '{id}' not found"));
    }
    if let Err(e) = store.set_model_state(&id, body.disabled).await {
        return internal(&e.to_string());
    }
    state.registry.set_disabled(&id, body.disabled);
    audit(&state, &ctx, None, if body.disabled { "model.disable" } else { "model.enable" }, "model", &id, None);
    match model_view(&state, &id) {
        Some(v) => (StatusCode::OK, Json(v)).into_response(),
        None => internal("model vanished from the registry"),
    }
}

/// DELETE /v1/admin/models/{id} — Remove a runtime model (LiteLLM `/model/delete`).
#[utoipa::path(
    operation_id = "admin_delete_model",
    delete,
    path = "/v1/admin/models/{id}",
    params(("id" = String, Path, description = "Model id (may contain slashes)")),
    responses(
        (status = 200, description = "Deleted", body = DeletedResponse),
        (status = 404, description = "No such runtime model", body = ErrorResponse),
        (status = 409, description = "Catalog models are read-only", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn delete_model(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(id): Path<String>) -> Response {
    let (ctx, store) = admin_or_return!(state, ctx);
    if state.registry.is_catalog(&id) {
        return super::err(StatusCode::CONFLICT, "catalog_model_read_only", "catalog models are defined in models/*.yaml; disable it with PATCH instead");
    }
    match store.delete_runtime_model(&id).await {
        Ok(true) => {
            state.registry.remove_runtime(&id);
            audit(&state, &ctx, None, "model.delete", "model", &id, None);
            (StatusCode::OK, Json(DeletedResponse { deleted: true })).into_response()
        }
        Ok(false) => not_found("model_not_found", &format!("runtime model '{id}' not found")),
        Err(e) => internal(&e.to_string()),
    }
}

// ─── Providers ───────────────────────────────────────────────────────────────

fn known_or_404(name: &str) -> Result<(), Response> {
    if runtime::known_provider(name) {
        Ok(())
    } else {
        Err(not_found("provider_not_found", &format!("unknown provider '{name}'")))
    }
}

/// GET /v1/admin/providers — Every provider adapter with its effective config. Secrets are masked.
#[utoipa::path(
    operation_id = "admin_list_providers",
    get,
    path = "/v1/admin/providers",
    responses(
        (status = 200, description = "Provider configs", body = ProviderConfigListResponse),
        (status = 403, description = "Gateway admin required", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn list_providers(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>) -> Response {
    let (_ctx, store) = admin_or_return!(state, ctx);
    let mut data = Vec::new();
    for name in runtime::all_provider_names() {
        match runtime::provider_view(&state, store, name).await {
            Ok(v) => data.push(v),
            Err(e) => return internal(&e),
        }
    }
    (StatusCode::OK, Json(ProviderConfigListResponse { data })).into_response()
}

/// GET /v1/admin/providers/{name}
#[utoipa::path(
    operation_id = "admin_get_provider",
    get,
    path = "/v1/admin/providers/{name}",
    params(("name" = String, Path, description = "Provider adapter name, e.g. openai")),
    responses(
        (status = 200, description = "Provider config", body = ProviderConfigView),
        (status = 404, description = "Unknown provider", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn get_provider(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(name): Path<String>) -> Response {
    let (_ctx, store) = admin_or_return!(state, ctx);
    if let Err(r) = known_or_404(&name) { return r; }
    match runtime::provider_view(&state, store, &name).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => internal(&e),
    }
}

/// PUT /v1/admin/providers/{name} — Set a provider's keys, api_base, model
/// mapping, headers, options or enabled flag at runtime. Secret fields left
/// out keep their stored value; every unset field falls back to env/YAML.
/// In-flight jobs finish on the instance they started with.
#[utoipa::path(
    operation_id = "admin_put_provider",
    put,
    path = "/v1/admin/providers/{name}",
    params(("name" = String, Path, description = "Provider adapter name, e.g. openai")),
    request_body = PutProviderConfigRequest,
    responses(
        (status = 200, description = "Provider config applied", body = ProviderConfigView),
        (status = 400, description = "Invalid config", body = ErrorResponse),
        (status = 404, description = "Unknown provider", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn put_provider(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(name): Path<String>,
    Json(body): Json<PutProviderConfigRequest>,
) -> Response {
    let (ctx, store) = admin_or_return!(state, ctx);
    if let Err(r) = known_or_404(&name) { return r; }
    if let Some(base) = &body.settings.api_base {
        let ok = reqwest::Url::parse(base).map(|u| matches!(u.scheme(), "http" | "https")).unwrap_or(false);
        if !ok {
            return bad_request("invalid_provider_config", "api_base must be an http(s) URL");
        }
    }
    let blank = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    let incoming = ProviderSecrets {
        api_key: blank(body.api_key),
        api_keys: blank(body.api_keys),
        key_id: blank(body.key_id),
        key_secret: blank(body.key_secret),
    };
    let existing = match store.get_provider_config(&name).await { Ok(e) => e, Err(e) => return internal(&e.to_string()) };
    let previous = match &existing {
        Some(s) => match runtime::decrypt_secrets(&state, s) {
            Ok(p) => p,
            Err(e) => return internal(&e),
        },
        None => ProviderSecrets::default(),
    };
    let secrets = incoming.clone().or(&previous);
    let (ciphertext, nonce) = if secrets.is_empty() {
        (None, None)
    } else {
        let Some(key) = state.secrets_key.as_ref() else {
            return bad_request("secrets_key_missing", "storing provider secrets needs LITEGEN__SECRETS_KEY configured");
        };
        let plain = match serde_json::to_vec(&secrets) { Ok(p) => p, Err(e) => return internal(&e.to_string()) };
        match crate::auth::secrets::encrypt(key, &plain) {
            Ok((c, n)) => (Some(c), Some(n)),
            Err(e) => return internal(&e),
        }
    };
    let hint_source = secrets.api_key.clone().or_else(|| secrets.api_keys.clone()).or_else(|| secrets.key_id.clone());
    let now = Utc::now();
    let row = StoredProviderConfig {
        provider: name.clone(),
        ciphertext,
        nonce,
        display_hint: hint_source.as_deref().and_then(runtime::key_hint),
        settings_json: serde_json::to_string(&body.settings).unwrap_or_else(|_| "{}".into()),
        created_at: existing.as_ref().map(|e| e.created_at).unwrap_or(now),
        updated_at: now,
    };
    if let Err(e) = store.upsert_provider_config(&row).await {
        return internal(&e.to_string());
    }
    if let Err(e) = runtime::apply_provider(&state, store, &name).await {
        return internal(&e);
    }
    audit(&state, &ctx, None, "provider.configure", "provider", &name, serde_json::to_value(&body.settings).ok());
    match runtime::provider_view(&state, store, &name).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => internal(&e),
    }
}

/// DELETE /v1/admin/providers/{name} — Drop the runtime config; the provider
/// reverts to its env/YAML config, or goes out of service if it has none.
#[utoipa::path(
    operation_id = "admin_delete_provider",
    delete,
    path = "/v1/admin/providers/{name}",
    params(("name" = String, Path, description = "Provider adapter name, e.g. openai")),
    responses(
        (status = 200, description = "Runtime config removed", body = ProviderConfigView),
        (status = 404, description = "Unknown provider or no runtime config", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn delete_provider(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(name): Path<String>) -> Response {
    let (ctx, store) = admin_or_return!(state, ctx);
    if let Err(r) = known_or_404(&name) { return r; }
    match store.delete_provider_config(&name).await {
        Ok(true) => {}
        Ok(false) => return not_found("provider_config_not_found", &format!("no runtime config for '{name}'")),
        Err(e) => return internal(&e.to_string()),
    }
    if let Err(e) = runtime::apply_provider(&state, store, &name).await {
        return internal(&e);
    }
    audit(&state, &ctx, None, "provider.reset", "provider", &name, None);
    match runtime::provider_view(&state, store, &name).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => internal(&e),
    }
}

// ─── Routes ──────────────────────────────────────────────────────────────────

fn routes_doc(state: &AppState) -> RoutesDocument {
    let overrides = state.router.overrides();
    let source = if overrides.model_routes.is_some() { "runtime" } else { "config" };
    RoutesDocument {
        source: Some(source.to_string()),
        routes: state.router.model_routes().iter().map(runtime::route_config_to_spec).collect(),
    }
}

/// GET /v1/admin/routes — Model routes in effect (fallbacks, load balancing, caching).
#[utoipa::path(
    operation_id = "admin_get_routes",
    get,
    path = "/v1/admin/routes",
    responses((status = 200, description = "Routes in effect", body = RoutesDocument)),
    tag = "Gateway"
)]
pub async fn get_routes(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>) -> Response {
    let _ = admin_or_return!(state, ctx);
    (StatusCode::OK, Json(routes_doc(&state))).into_response()
}

/// PUT /v1/admin/routes — Replace every model route at runtime. Order matters:
/// the first route whose pattern matches a model wins.
#[utoipa::path(
    operation_id = "admin_put_routes",
    put,
    path = "/v1/admin/routes",
    request_body = RoutesDocument,
    responses(
        (status = 200, description = "Routes in effect", body = RoutesDocument),
        (status = 400, description = "Invalid routes", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn put_routes(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Json(body): Json<RoutesDocument>,
) -> Response {
    let (ctx, store) = admin_or_return!(state, ctx);
    if let Err(msg) = runtime::validate_routes(&body.routes) {
        return bad_request("invalid_routes", &msg);
    }
    let json = match serde_json::to_string(&body.routes) { Ok(j) => j, Err(e) => return internal(&e.to_string()) };
    if let Err(e) = store.set_setting(runtime::ROUTES_KEY, &json).await {
        return internal(&e.to_string());
    }
    let mut o = state.router.overrides();
    o.model_routes = Some(body.routes.iter().map(runtime::route_spec_to_config).collect());
    state.router.set_overrides(o);
    audit(&state, &ctx, None, "routes.update", "routes", "model_routes", serde_json::to_value(&body.routes).ok());
    (StatusCode::OK, Json(routes_doc(&state))).into_response()
}

/// DELETE /v1/admin/routes — Drop the runtime routes and go back to litegen.yaml's.
#[utoipa::path(
    operation_id = "admin_delete_routes",
    delete,
    path = "/v1/admin/routes",
    responses((status = 200, description = "Routes in effect", body = RoutesDocument)),
    tag = "Gateway"
)]
pub async fn delete_routes(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>) -> Response {
    let (ctx, store) = admin_or_return!(state, ctx);
    if let Err(e) = store.delete_setting(runtime::ROUTES_KEY).await {
        return internal(&e.to_string());
    }
    let mut o = state.router.overrides();
    o.model_routes = None;
    state.router.set_overrides(o);
    audit(&state, &ctx, None, "routes.reset", "routes", "model_routes", None);
    (StatusCode::OK, Json(routes_doc(&state))).into_response()
}

// ─── Gateway settings ────────────────────────────────────────────────────────

fn settings_view(state: &AppState) -> GatewaySettingsView {
    GatewaySettingsView {
        settings: state.router.overrides().gateway,
        effective_cost_markup_percent: state.router.cost_markup_percent(),
    }
}

/// GET /v1/admin/settings
#[utoipa::path(
    operation_id = "admin_get_settings",
    get,
    path = "/v1/admin/settings",
    responses((status = 200, description = "Gateway settings", body = GatewaySettingsView)),
    tag = "Gateway"
)]
pub async fn get_settings(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>) -> Response {
    let _ = admin_or_return!(state, ctx);
    (StatusCode::OK, Json(settings_view(&state))).into_response()
}

/// PUT /v1/admin/settings — Replace the gateway settings (markup, budget alert
/// webhook, default end-user budget). An omitted field is cleared.
#[utoipa::path(
    operation_id = "admin_put_settings",
    put,
    path = "/v1/admin/settings",
    request_body = GatewaySettings,
    responses(
        (status = 200, description = "Gateway settings", body = GatewaySettingsView),
        (status = 400, description = "Invalid settings", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn put_settings(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Json(body): Json<GatewaySettings>,
) -> Response {
    let (ctx, store) = admin_or_return!(state, ctx);
    if let Err(msg) = runtime::validate_gateway_settings(&body) {
        return bad_request("invalid_settings", &msg);
    }
    let json = match serde_json::to_string(&body) { Ok(j) => j, Err(e) => return internal(&e.to_string()) };
    if let Err(e) = store.set_setting(runtime::SETTINGS_KEY, &json).await {
        return internal(&e.to_string());
    }
    let mut o = state.router.overrides();
    o.gateway = body.clone();
    state.router.set_overrides(o);
    audit(&state, &ctx, None, "settings.update", "settings", "gateway", serde_json::to_value(&body).ok());
    (StatusCode::OK, Json(settings_view(&state))).into_response()
}
