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
use crate::shared::events::SharedEvent;
use crate::types::ErrorResponse;

fn admin(state: &AppState, ctx: Option<Extension<KeyContext>>) -> Result<(KeyContext, &dyn ManagementStore), Response> {
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
    let mut m: ModelSchema = serde_json::from_value(body).map_err(|e| bad_request("invalid_model", &format!("invalid model schema: {e}")))?;
    // `GET /v1/models/{id}` serves `litegen_params`, which the server
    // computes: a served schema posted back carries it, read and ignored as
    // `status` is (dashboard-server review #2). The loader still refuses it
    // in a catalog row, where it can only be a mistake.
    m.litegen_params.clear();
    Ok(m)
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
    runtime::announce(state, SharedEvent::Reload).await;
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
    runtime::announce(&state, SharedEvent::Reload).await;
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
            runtime::announce(&state, SharedEvent::Reload).await;
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

/// What the audit log keeps of an extra header's value. Migration 027 writes
/// the same string over the values that entries kept before 2026-11-14.
const REDACTED: &str = runtime::REDACTED;

/// What a provider's audit entry keeps of its settings: each extra header's
/// name, never its value, since a gateway's own auth travels in them
/// (`cf-aig-authorization`, `Helicone-Auth`) and the provider view shows names
/// only. An api_base, and each URL in the options, shows `[redacted]` for its
/// user info, as the provider view does. Everything else as set. The audit
/// log goes further than the provider view: backups and CSV exports keep its
/// entries, and in single-tenant mode the default org's Owners and Admins
/// read the gateway's changes. (In hosted mode the master key's changes to
/// the gateway are filed under no org, which only the master key reads.)
fn audited_settings(settings: &ProviderSettings) -> Option<serde_json::Value> {
    let mut kept = settings.clone();
    if let Some(headers) = kept.extra_headers.as_mut() {
        headers.values_mut().for_each(|v| *v = REDACTED.to_string());
    }
    if let Some(base) = kept.api_base.as_mut() {
        *base = runtime::shown_api_base(base);
    }
    if let Some(options) = kept.options.as_mut() {
        *options = runtime::shown_options(options);
    }
    serde_json::to_value(&kept).ok()
}

/// What a gateway settings entry keeps of its settings: `[redacted]` for the
/// alert webhook URL, since a URL can carry its secret in the path (Slack,
/// Discord) or in the host (Pipedream, an AWS Lambda function URL), and the
/// alert POST is unsigned, so the URL is all that guards the receiver. An
/// empty or absent URL stays as it is, so the entry still says whether one is
/// set. `GET /v1/admin/settings` shows the whole URL to the gateway's admins
/// only (the audit log's wider readers: see [`audited_settings`]). Everything
/// else as set. Migration 028 does the same to the entries kept before
/// 2026-11-14.
fn audited_gateway_settings(settings: &GatewaySettings) -> Option<serde_json::Value> {
    let mut kept = settings.clone();
    if let Some(url) = kept.alert_webhook_url.as_mut().filter(|u| !u.is_empty()) {
        *url = REDACTED.to_string();
    }
    serde_json::to_value(&kept).ok()
}

/// PUT /v1/admin/providers/{name} — Set a provider's keys, api_base, model
/// mapping, headers, options or enabled flag at runtime. Secret fields left
/// out keep their stored value; every unset field falls back to env/YAML.
/// In-flight jobs finish on the instance they started with. To change some
/// fields and keep the rest, use PATCH.
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
    if let Err(r) = check_sent(body.settings.api_base.as_deref(), body.settings.extra_headers.as_ref()) { return r; }
    let incoming = keys_sent(body.api_key, body.api_keys, body.key_id, body.key_secret);
    let (existing, previous, stored) = match opened_row(&state, store, &name).await { Ok(o) => o, Err(r) => return r };
    let settings = match meant_settings(body.settings, &in_force_urls(&state, &name, &stored)) {
        Ok(s) => s,
        Err(e) => return bad_request("invalid_provider_config", &e),
    };
    store_provider(&state, &ctx, store, &name, existing.as_ref(), &incoming.or(&previous), &settings).await
}

/// PATCH /v1/admin/providers/{name} — Change some of a provider's runtime
/// config and keep the rest. A field sent is set, `null` clears it (it
/// falls back to env/YAML), and a field left out keeps its stored value.
/// A key left out or `null` keeps its stored value, and one sent as `""`
/// removes it (the env/YAML key applies again). A map (`model_mapping`,
/// `extra_headers`) or `options` sent replaces the stored one whole.
/// In-flight jobs finish on the instance they started with.
#[utoipa::path(
    operation_id = "admin_patch_provider",
    patch,
    path = "/v1/admin/providers/{name}",
    params(("name" = String, Path, description = "Provider adapter name, e.g. openai")),
    request_body = PatchProviderConfigRequest,
    responses(
        (status = 200, description = "Provider config applied", body = ProviderConfigView),
        (status = 400, description = "Invalid config", body = ErrorResponse),
        (status = 404, description = "Unknown provider", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn patch_provider(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(name): Path<String>,
    Json(body): Json<PatchProviderConfigRequest>,
) -> Response {
    let (ctx, store) = admin_or_return!(state, ctx);
    if let Err(r) = known_or_404(&name) { return r; }
    let sent = body.sent_settings();
    if let Err(r) = check_sent(sent.api_base.as_deref(), sent.extra_headers.as_ref()) { return r; }
    let (existing, previous, stored) = match opened_row(&state, store, &name).await { Ok(o) => o, Err(r) => return r };
    let sent = match meant_settings(sent, &in_force_urls(&state, &name, &stored)) {
        Ok(s) => s,
        Err(e) => return bad_request("invalid_provider_config", &e),
    };
    let settings = body.applied_to(stored, sent);
    store_provider(&state, &ctx, store, &name, existing.as_ref(), &patched_keys(&body, &previous), &settings).await
}

/// The keys a PATCH leaves: one sent with a value replaces the stored one,
/// one sent as `""` (or blank) is removed, so the env/YAML key applies again,
/// and one left out or `null` keeps its stored value.
fn patched_keys(body: &PatchProviderConfigRequest, stored: &ProviderSecrets) -> ProviderSecrets {
    fn pick(sent: &Option<String>, stored: &Option<String>) -> Option<String> {
        match sent.as_deref() {
            Some(v) if v.trim().is_empty() => None,
            Some(v) => Some(v.to_string()),
            None => stored.clone(),
        }
    }
    ProviderSecrets {
        api_key: pick(&body.api_key, &stored.api_key),
        api_keys: pick(&body.api_keys, &stored.api_keys),
        key_id: pick(&body.key_id, &stored.key_id),
        key_secret: pick(&body.key_secret, &stored.key_secret),
    }
}

/// A provider's runtime row, if it has one, with its keys and its settings
/// as set (credentials opened).
type OpenedRow = (Option<StoredProviderConfig>, ProviderSecrets, ProviderSettings);

/// Provider `name`'s [`OpenedRow`].
async fn opened_row(state: &AppState, store: &dyn ManagementStore, name: &str) -> Result<OpenedRow, Response> {
    let existing = store.get_provider_config(name).await.map_err(|e| internal(&e.to_string()))?;
    let (keys, settings) = match &existing {
        Some(s) => runtime::open_config(state, s).map_err(|e| internal(&e))?,
        None => Default::default(),
    };
    Ok((existing, keys, settings))
}

/// The URLs in force for provider `name`, which a save may send back as the
/// view shows them: its runtime config's (`stored`, credentials opened),
/// else its env/YAML config's.
fn in_force_urls(state: &AppState, name: &str, stored: &ProviderSettings) -> ProviderSettings {
    let env = runtime::env_config(state, name);
    ProviderSettings {
        api_base: stored.api_base.clone().or_else(|| env.as_ref().and_then(|e| e.api_base.clone())),
        options: stored.options.clone().or_else(|| env.and_then(|e| e.options)),
        ..Default::default()
    }
}

/// The settings a save meant. The provider view shows the user info of an
/// api_base, and of each URL in the options, as `[redacted]`, and the
/// dashboard sends the api_base it shows back with every save: a URL sent
/// back exactly as shown stands for the one in force (`in_force`). Any other
/// URL that keeps `[redacted]` for its user info would send it to the proxy
/// as the user, so the save is refused, naming the field.
fn meant_settings(mut sent: ProviderSettings, in_force: &ProviderSettings) -> Result<ProviderSettings, String> {
    if let Some(base) = sent.api_base.as_mut() {
        meant_url(base, in_force.api_base.as_deref(), "api_base")?;
    }
    if let Some(options) = sent.options.as_mut() {
        meant_option_urls(options, in_force.options.as_ref(), "options")?;
    }
    Ok(sent)
}

/// `sent` as [`meant_settings`] reads it: the URL in force when `sent` is
/// that URL as the view shows it; an error naming `field` when it keeps
/// `[redacted]` for its user info otherwise.
fn meant_url(sent: &mut String, in_force: Option<&str>, field: &str) -> Result<(), String> {
    match in_force.filter(|b| *b != sent.as_str() && runtime::shown_api_base(b) == *sent) {
        Some(b) => *sent = b.to_string(),
        None if sent.contains(&runtime::redacted_user_info()) => {
            return Err(format!(
                "{field} has `[redacted]` for its user info: send the user info itself, or the URL exactly as the provider view shows it to keep the one in force"
            ));
        }
        None => {}
    }
    Ok(())
}

/// [`meant_url`] for each URL in `sent` options, against the option at the
/// same place in `in_force`; `path` names it (`options.platform_api_base`).
fn meant_option_urls(sent: &mut serde_json::Value, in_force: Option<&serde_json::Value>, path: &str) -> Result<(), String> {
    use serde_json::Value;
    match sent {
        Value::String(s) => meant_url(s, in_force.and_then(Value::as_str), path),
        Value::Array(items) => items
            .iter_mut()
            .enumerate()
            .try_for_each(|(i, item)| meant_option_urls(item, in_force.and_then(|v| v.get(i)), &format!("{path}[{i}]"))),
        Value::Object(fields) => fields
            .iter_mut()
            .try_for_each(|(k, v)| meant_option_urls(v, in_force.and_then(|f| f.get(k)), &format!("{path}.{k}"))),
        _ => Ok(()),
    }
}

/// The checks a provider save makes on what it sends: an http(s) api_base,
/// and extra headers litegen can send.
fn check_sent(api_base: Option<&str>, headers: Option<&std::collections::HashMap<String, String>>) -> Result<(), Response> {
    if let Some(base) = api_base {
        let ok = reqwest::Url::parse(base).map(|u| matches!(u.scheme(), "http" | "https")).unwrap_or(false);
        if !ok {
            return Err(bad_request("invalid_provider_config", "api_base must be an http(s) URL"));
        }
    }
    if let Some(headers) = headers {
        crate::providers::check_extra_headers(headers).map_err(|e| bad_request("invalid_provider_config", &e))?;
    }
    Ok(())
}

/// The keys a provider save sends: one left out or blank keeps its stored value.
fn keys_sent(api_key: Option<String>, api_keys: Option<String>, key_id: Option<String>, key_secret: Option<String>) -> ProviderSecrets {
    let blank = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    ProviderSecrets { api_key: blank(api_key), api_keys: blank(api_keys), key_id: blank(key_id), key_secret: blank(key_secret) }
}

/// Store provider `name`'s runtime config as `secrets` and `settings` (its
/// row before: `existing`), its credentials as `encrypt_provider_credentials`
/// says, then put it in force and answer with its view.
async fn store_provider(
    state: &AppState,
    ctx: &KeyContext,
    store: &dyn ManagementStore,
    name: &str,
    existing: Option<&StoredProviderConfig>,
    secrets: &ProviderSecrets,
    settings: &ProviderSettings,
) -> Response {
    let encrypt = runtime::encrypts_credentials(&state.router.overrides().gateway);
    let sealed = match runtime::seal_config(state.secrets_key.as_ref(), encrypt, secrets, settings) {
        Ok(s) => s,
        Err(runtime::SealError::KeyMissing) => {
            return bad_request("secrets_key_missing", "storing provider secrets needs LITEGEN__SECRETS_KEY configured");
        }
        Err(runtime::SealError::Failed(e)) => return internal(&e),
    };
    let hint_source = secrets.api_key.clone().or_else(|| secrets.api_keys.clone()).or_else(|| secrets.key_id.clone());
    let now = Utc::now();
    let row = StoredProviderConfig {
        provider: name.to_string(),
        ciphertext: sealed.ciphertext,
        nonce: sealed.nonce,
        display_hint: hint_source.as_deref().and_then(runtime::key_hint),
        settings_json: sealed.settings_json,
        created_at: existing.map(|e| e.created_at).unwrap_or(now),
        updated_at: now,
    };
    if let Err(e) = store.upsert_provider_config(&row).await {
        return internal(&e.to_string());
    }
    // This replica may not have heard of a switch of the setting yet, and the
    // switch's own pass may have read the rows before this write: read the
    // stored setting after it, and store the row again if it changed.
    match runtime::load_gateway_settings(store).await {
        Ok(gateway) if runtime::encrypts_credentials(&gateway) != encrypt => {
            if let Err(e) = runtime::store_configs_as(state, store, !encrypt, Some(name)).await {
                tracing::warn!(provider = %name, error = %e, "could not store this runtime provider config again as encrypt_provider_credentials now says; the next save of the gateway settings does");
            }
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(provider = %name, error = %e, "could not read encrypt_provider_credentials after a provider save; its row stays as saved until the next save of the gateway settings"),
    }
    runtime::announce(state, SharedEvent::Reload).await;
    if let Err(e) = runtime::apply_provider(state, store, name).await {
        return internal(&e);
    }
    audit(state, ctx, None, "provider.configure", "provider", name, audited_settings(settings));
    match runtime::provider_view(state, store, name).await {
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
    runtime::announce(&state, SharedEvent::Reload).await;
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
    apply_routes(&state.router, Some(body.routes.iter().map(runtime::route_spec_to_config).collect()));
    runtime::announce(&state, SharedEvent::Reload).await;
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
    apply_routes(&state.router, None);
    runtime::announce(&state, SharedEvent::Reload).await;
    audit(&state, &ctx, None, "routes.reset", "routes", "model_routes", None);
    (StatusCode::OK, Json(routes_doc(&state))).into_response()
}

/// Put `routes` in force (`None`: litegen.yaml's again) and keep every other
/// override as it is: changed under the overrides' lock, so a settings save
/// at the same instant on another thread isn't undone (FU-LOCK).
fn apply_routes(router: &crate::proxy::router::ProxyRouter, routes: Option<Vec<crate::config::ModelRouteConfig>>) {
    router.update_overrides(|o| o.model_routes = routes);
}

// ─── Gateway settings ────────────────────────────────────────────────────────

async fn settings_view(state: &AppState) -> GatewaySettingsView {
    let gateway = state.router.overrides().gateway;
    let access = crate::access::effective(&state.router.config.storage_access, &gateway, None);
    let s = state.router.settings();
    GatewaySettingsView {
        settings: gateway,
        effective_cost_markup_percent: state.router.cost_markup_percent(),
        effective_storage_access: access.into(),
        effective_cache: s.cache.into(),
        cache_entries: state.router.cache.entry_count().await,
        effective_retries: s.retries.into(),
        effective_vendor_timeout_seconds: s.vendor_timeout.as_secs(),
        effective_circuit_breaker: s.breaker.into(),
        limits: GatewaySettingLimits::ALLOWED,
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
    (StatusCode::OK, Json(settings_view(&state).await)).into_response()
}

/// POST /v1/admin/revoke-links — end every link litegen signed, gateway-wide.
#[utoipa::path(
    operation_id = "admin_revoke_links",
    post,
    path = "/v1/admin/revoke-links",
    responses(
        (status = 200, description = "Every litegen link issued before `revoked_at` no longer works. A bucket's own presigned links work until they expire.", body = crate::access::LinksRevoked),
        (status = 403, description = "Forbidden", body = ErrorResponse),
    ),
    tag = "Gateway"
)]
pub async fn revoke_links(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
) -> Response {
    let (ctx, _store) = admin_or_return!(state, ctx);
    let at = crate::access::revocation_time();
    if let Err(e) = state.db.revoke_links("gateway", "", at).await {
        return internal(&e.to_string());
    }
    audit(&state, &ctx, None, "links.revoke", "settings", "gateway", Some(serde_json::json!({ "revoked_at": at })));
    (StatusCode::OK, Json(crate::access::LinksRevoked { revoked_at: at })).into_response()
}

/// PUT /v1/admin/settings — Replace the gateway settings (markup, budget alert
/// webhook, default end-user budget). An omitted field is cleared, except
/// `encrypt_provider_credentials`, which keeps its stored value.
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
    Json(mut body): Json<GatewaySettings>,
) -> Response {
    let (ctx, store) = admin_or_return!(state, ctx);
    if let Err(msg) = runtime::validate_gateway_settings(&body) {
        return bad_request("invalid_settings", &msg);
    }
    // Unlike the other fields, `encrypt_provider_credentials` left out keeps
    // its stored value: a client written before it existed must not turn the
    // encryption off and store every credential in plain text again. `null`
    // or `false` turns it off, which is stored as left out.
    if body.encrypt_provider_credentials.is_none() {
        match runtime::load_gateway_settings(store).await {
            Ok(stored) => body.encrypt_provider_credentials = stored.encrypt_provider_credentials,
            Err(e) => return internal(&e.to_string()),
        }
    }
    let encrypt = runtime::encrypts_credentials(&body);
    body.encrypt_provider_credentials = encrypt.then_some(Some(true));
    if encrypt && state.secrets_key.is_none() {
        return bad_request("secrets_key_missing", "encrypt_provider_credentials needs LITEGEN__SECRETS_KEY configured");
    }
    // What this replica held before the save, to say when the setting switches.
    let was_encrypting = runtime::encrypts_credentials(&state.router.overrides().gateway);
    let json = match serde_json::to_string(&body) { Ok(j) => j, Err(e) => return internal(&e.to_string()) };
    // Read before the row is written: once it is, a refresh on this replica
    // can apply it at any moment, and the cache would then look as if it had
    // been off all along.
    let cache_was_on = state.router.settings().cache.enabled;
    if let Err(e) = store.set_setting(runtime::SETTINGS_KEY, &json).await {
        return internal(&e.to_string());
    }
    // Now that the setting is stored, every provider row goes the way it
    // says (`runtime::store_configs_as`), on this replica and to the end even
    // if the client goes away: a pass stopped half way would leave rows an
    // older version can't read before a rollback.
    let pass = state.clone();
    let stored_again = runtime::finish_even_if_dropped_with(async move {
        match pass.db.management() {
            Some(store) => runtime::store_configs_after_save(&pass, store, encrypt).await,
            None => Ok((encrypt, Vec::new())),
        }
    })
    .await;
    let cache_now_on = crate::management::settings::effective(&state.router.config, &body).cache.enabled;
    if cache_was_on != cache_now_on {
        // Turning the gateway cache off or on empties it everywhere: this
        // replica's tiers, and the shared tier with the epoch raised, before
        // the other replicas are told to reload, so none of them serves what
        // was cached before the switch. Off: nothing is served from it.
        // On: nothing an earlier switch-off failed to remove from Redis, nor
        // what routes with their own cache settings stored meanwhile. Off
        // switches first, so no new answer is stored during the clear. On
        // clears first and switches after it. The row is saved already,
        // though, so a refresh during the clear can switch a replica on
        // early, this one included, and it can then serve an answer from
        // before the switch until the clear is done there (another replica:
        // until it hears of it). That window is accepted.
        //
        // The save answers 200 whatever the clear did: the switch must work
        // while Redis is down, and the database carries it to every replica.
        // A clear that failed is logged (once a minute). A task of its own,
        // so a client that goes away cannot leave the clear half done and the
        // others untold.
        let switching = state.clone();
        let gateway = body.clone();
        runtime::finish_even_if_dropped(async move {
            if cache_now_on {
                let _ = switching.router.cache.clear().await;
                apply_gateway_settings(&switching.router, gateway);
            } else {
                apply_gateway_settings(&switching.router, gateway);
                let _ = switching.router.cache.clear().await;
            }
            runtime::announce(&switching, SharedEvent::Reload).await;
        })
        .await;
    } else {
        apply_gateway_settings(&state.router, body.clone());
        runtime::announce(&state, SharedEvent::Reload).await;
    }
    audit(&state, &ctx, None, "settings.update", "settings", "gateway", audited_gateway_settings(&body));
    match stored_again {
        // `stored`: the setting the rows were stored for, which is another
        // save's if it switched the setting again meanwhile.
        Some(Ok((stored, providers))) => {
            if was_encrypting != stored || !providers.is_empty() {
                tracing::info!(
                    encrypt_provider_credentials = stored,
                    configs_stored_again = providers.len(),
                    ?providers,
                    "runtime provider configs stored again as encrypt_provider_credentials says"
                );
                let action = if stored { "provider.encrypt_credentials" } else { "provider.decrypt_credentials" };
                let after = serde_json::json!({ "encrypt_provider_credentials": stored, "configs_stored_again": providers.len(), "providers": providers });
                audit(&state, &ctx, None, action, "provider", "*", Some(after));
            }
        }
        Some(Err(e)) => {
            return internal(&format!(
                "the settings were saved, but the runtime provider configs could not all be stored again as encrypt_provider_credentials says ({e}): save the settings again"
            ));
        }
        None => return internal("the server is stopping: the runtime provider configs may not all be stored as encrypt_provider_credentials says; save the settings again"),
    }
    (StatusCode::OK, Json(settings_view(&state).await)).into_response()
}

/// Put `gateway` in force and keep every other override as it is: changed
/// under the overrides' lock, so a route saved at the same instant on another
/// thread, or applied by a refresh, isn't undone (FU-LOCK).
fn apply_gateway_settings(router: &crate::proxy::router::ProxyRouter, gateway: GatewaySettings) {
    router.update_overrides(|o| o.gateway = gateway);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::router::ProxyRouter;
    use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

    fn router() -> ProxyRouter {
        ProxyRouter::new(
            Arc::new(crate::proxy::registry::ProviderRegistry::new()),
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(crate::config::AppConfig::default()),
            Arc::new(crate::proxy::storage::LocalStore),
        )
    }

    /// The routes saved in round `n`: one route, for model `m{n}`.
    fn routes(n: u32) -> Option<Vec<crate::config::ModelRouteConfig>> {
        Some(vec![crate::config::ModelRouteConfig { model: format!("m{n}"), deployments: Vec::new(), strategy: None, cache: None }])
    }

    /// The round whose routes are in force, 0 before any.
    fn routes_round(router: &ProxyRouter) -> u32 {
        router.model_routes().first().and_then(|r| r.model.strip_prefix('m')?.parse().ok()).unwrap_or(0)
    }

    /// N1 (leftovers 2026-11-14): the audit copy of a provider's settings
    /// names each extra header and holds none of their values, which can be
    /// a gateway's auth. Every other field is kept as set.
    #[test]
    fn audited_settings_keep_header_names_and_drop_their_values() {
        let settings = ProviderSettings {
            api_base: Some("https://gateway.example/v1".into()),
            model_mapping: Some([("openai/gpt-image-2".to_string(), "my-deployment".to_string())].into()),
            extra_headers: Some([
                ("cf-aig-authorization".to_string(), "Bearer cf-token-123".to_string()),
                ("OpenAI-Organization".to_string(), "org-123".to_string()),
            ].into()),
            options: Some(serde_json::json!({ "platform_api_base": "https://gateway.example/fal-platform" })),
            region: Some("us-east-1".into()),
            enabled: Some(true),
        };
        assert_eq!(
            audited_settings(&settings),
            Some(serde_json::json!({
                "api_base": "https://gateway.example/v1",
                "model_mapping": { "openai/gpt-image-2": "my-deployment" },
                "extra_headers": { "cf-aig-authorization": "[redacted]", "OpenAI-Organization": "[redacted]" },
                "options": { "platform_api_base": "https://gateway.example/fal-platform" },
                "region": "us-east-1",
                "enabled": true,
            }))
        );
    }

    /// provider-secrets (2026-11-15): the audit copy shows an api_base's user
    /// info as `[redacted]`, as the provider view does, since it is a
    /// credential (a proxy's basic auth, a token as the user name). An
    /// api_base without any is kept as set.
    #[test]
    fn audited_settings_show_an_api_bases_user_info_as_redacted() {
        for (base, kept) in [
            ("https://proxy-user:proxy-pass@gateway.example/v1", "https://[redacted]@gateway.example/v1"),
            ("https://token-as-user@gateway.example/v1?team=a", "https://[redacted]@gateway.example/v1?team=a"),
            ("https://:pass-only@gateway.example:8443/", "https://[redacted]@gateway.example:8443/"),
            ("https://gateway.example/v1", "https://gateway.example/v1"),
        ] {
            let settings = ProviderSettings { api_base: Some(base.into()), ..Default::default() };
            assert_eq!(audited_settings(&settings).expect("settings serialize")["api_base"], kept, "{base}");
        }
    }

    /// provider-config (2026-11-15): a URL in `options` (fal's
    /// `platform_api_base` and `sync_api_base`, Bedrock's `s3_api_base`) can
    /// carry a credential in its user info as an api_base can, and the audit
    /// copy kept it whole. It shows `[redacted]` for the user info, wherever
    /// the URL sits in the options; everything else is kept as set.
    #[test]
    fn audited_settings_show_the_user_info_of_option_urls_as_redacted() {
        let options = serde_json::json!({
            "platform_api_base": "https://fal-user:fal-pass-7720@proxy.example/fal-platform",
            "sync_api_base": "https://sync.example/",
            "s3_output_uri": "s3://bucket/prefix",
            "nested": { "urls": ["https://token-7720@hooks.example/a", "plain text"] },
            "retries": 3,
        });
        let settings = ProviderSettings { options: Some(options), ..Default::default() };
        assert_eq!(
            audited_settings(&settings).expect("settings serialize")["options"],
            serde_json::json!({
                "platform_api_base": "https://[redacted]@proxy.example/fal-platform",
                "sync_api_base": "https://sync.example/",
                "s3_output_uri": "s3://bucket/prefix",
                "nested": { "urls": ["https://[redacted]@hooks.example/a", "plain text"] },
                "retries": 3,
            })
        );
    }

    /// N1 follow-up: the audit copy of the gateway settings shows the alert
    /// webhook URL as `[redacted]`, whatever its form, since the secret can be
    /// in the path or in the host. An empty or absent URL stays as it is, so
    /// the entry still says whether one is set. Every other field is kept.
    #[test]
    fn audited_gateway_settings_redact_the_alert_webhook_url() {
        let with = |url: Option<&str>| GatewaySettings { alert_webhook_url: url.map(str::to_string), cost_markup_percent: Some(12.5), ..Default::default() };
        for (url, kept) in [
            (Some("https://hooks.slack.com/services/T0001/B0002/secret-1"), serde_json::json!("[redacted]")),
            (Some("https://eo3secret77.m.pipedream.net/hook"), serde_json::json!("[redacted]")),
            (Some("HTTPS://User:Pass@Hooks.Example.COM:443/alerts?token=abc#frag"), serde_json::json!("[redacted]")),
            (Some("not a url"), serde_json::json!("[redacted]")),
            (Some(""), serde_json::json!("")),
            (None, serde_json::Value::Null),
        ] {
            let audited = audited_gateway_settings(&with(url)).expect("settings serialize");
            assert_eq!(audited["alert_webhook_url"], kept, "{url:?}");
            assert_eq!(audited["cost_markup_percent"], 12.5, "{url:?}: the rest is kept");
        }
    }

    /// FU-LOCK (follow-ups 2026-11-13): a route save and a settings save at
    /// the same instant, on two worker threads, both land. Each read the
    /// overrides, changed its own field and wrote the whole back, so whichever
    /// wrote last could put back the other's old value. Each writer here saves
    /// its own field in rounds 1, 2, 3 and so on, and nothing else changes it,
    /// so after its round `n` it reads `n` or more unless the other writer put
    /// an older copy back.
    #[test]
    fn a_route_save_and_a_settings_save_at_once_both_land() {
        const ROUNDS: u32 = 20_000;
        let router = router();
        let lost = AtomicU32::new(0);
        std::thread::scope(|s| {
            s.spawn(|| {
                for n in 1..=ROUNDS {
                    apply_routes(&router, routes(n));
                    if routes_round(&router) < n {
                        lost.fetch_add(1, Relaxed);
                    }
                }
            });
            s.spawn(|| {
                for n in 1..=ROUNDS {
                    apply_gateway_settings(&router, GatewaySettings { cost_markup_percent: Some(f64::from(n)), ..Default::default() });
                    if router.cost_markup_percent() < f64::from(n) {
                        lost.fetch_add(1, Relaxed);
                    }
                }
            });
        });
        let last = (routes_round(&router), router.cost_markup_percent());
        assert!(
            lost.load(Relaxed) == 0 && last == (ROUNDS, f64::from(ROUNDS)),
            "one save undid the other's: {} of {} saves seen lost, and the last left (routes round, markup) = {last:?}",
            lost.load(Relaxed),
            2 * ROUNDS
        );
    }
}
