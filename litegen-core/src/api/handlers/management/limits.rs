//! Limits (budgets, model allowlists, rpm and parallel caps) for keys, orgs,
//! apps and org members, plus key block/unblock.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use super::{audit, bad_request, ctx_of, forbidden, internal, not_found, require_org, store, Access, DeletedResponse};
use crate::api::middleware::{is_platform_admin, AppState, KeyContext, Scope};
use crate::auth::permissions::Permission;
use crate::management::budget;
use crate::management::enforce::load_current;
use crate::management::store::ManagementStore;
use crate::management::types::*;
use crate::types::ErrorResponse;

// ─── Shared limits logic ─────────────────────────────────────────────────────

/// Validate a `PUT …/limits` body. Allowlisted models must exist.
pub(crate) fn validate_input(state: &AppState, input: &LimitsInput) -> Result<(), Response> {
    let money = |name: &str, v: Option<f64>| -> Result<(), Response> {
        match v {
            Some(x) if !x.is_finite() || x < 0.0 => {
                Err(bad_request("invalid_limits", &format!("{name} must be a non-negative number")))
            }
            _ => Ok(()),
        }
    };
    money("max_budget_usd", input.max_budget_usd)?;
    money("soft_budget_usd", input.soft_budget_usd)?;
    if let (Some(soft), Some(max)) = (input.soft_budget_usd, input.max_budget_usd) {
        if soft > max {
            return Err(bad_request("invalid_limits", "soft_budget_usd must not exceed max_budget_usd"));
        }
    }
    if let Some(d) = &input.budget_duration {
        budget::parse_duration(d).map_err(|e| bad_request("invalid_limits", &e))?;
    }
    if input.max_parallel_requests == Some(0) {
        return Err(bad_request("invalid_limits", "max_parallel_requests must be at least 1"));
    }
    if input.rpm_limit == Some(0) {
        return Err(bad_request("invalid_limits", "rpm_limit must be at least 1"));
    }
    if let Some(m) = &input.metadata {
        if !m.is_object() {
            return Err(bad_request("invalid_limits", "metadata must be a JSON object"));
        }
    }
    if let Some(models) = &input.allowed_models {
        for id in models {
            if state.registry.resolve(id).is_none() && !state.registry.is_catalog(id) {
                return Err(bad_request("unknown_model", &format!("allowed_models: unknown model '{id}'")));
            }
        }
    }
    Ok(())
}

/// Merge `input` into the existing row (or a new one). Spend survives an
/// edit; a changed `budget_duration` starts a fresh window from now.
pub(crate) fn merged_limits(
    existing: Option<EntityLimits>,
    entity_type: EntityType,
    entity_id: &str,
    org_id: Option<String>,
    input: LimitsInput,
) -> EntityLimits {
    let now = Utc::now();
    let (spend, mut reset_at, mut alerted, created_at, old_duration) = match &existing {
        Some(e) => (e.spend_usd, e.budget_reset_at, e.soft_budget_alerted_at, e.created_at, e.budget_duration.clone()),
        None => (0.0, None, None, now, None),
    };
    if existing.is_none() || old_duration != input.budget_duration {
        reset_at = budget::initial_reset_at(input.budget_duration.as_deref(), now).unwrap_or(None);
    }
    // Raising the soft budget above current spend re-arms the alert.
    if input.soft_budget_usd.map_or(true, |soft| spend < soft) {
        alerted = None;
    }
    EntityLimits {
        entity_type,
        entity_id: entity_id.to_string(),
        org_id,
        max_budget_usd: input.max_budget_usd,
        soft_budget_usd: input.soft_budget_usd,
        budget_duration: input.budget_duration,
        spend_usd: spend,
        budget_reset_at: reset_at,
        soft_budget_alerted_at: alerted,
        allowed_models: input.allowed_models,
        max_parallel_requests: input.max_parallel_requests,
        rpm_limit: input.rpm_limit,
        metadata: input.metadata,
        created_at,
        updated_at: now,
    }
}

pub(crate) async fn do_get(store: &dyn ManagementStore, t: EntityType, id: &str) -> Response {
    match load_current(store, t, id).await {
        Ok(Some(l)) => (StatusCode::OK, Json(l)).into_response(),
        Ok(None) => not_found("limits_not_found", &format!("no limits set for this {}", t.as_str())),
        Err(e) => internal(&e.to_string()),
    }
}

pub(crate) async fn do_put(
    state: &AppState,
    ctx: &KeyContext,
    store: &dyn ManagementStore,
    t: EntityType,
    id: &str,
    org_id: Option<String>,
    input: LimitsInput,
) -> Response {
    if let Err(r) = validate_input(state, &input) {
        return r;
    }
    let existing = match store.get_limits(t, id).await {
        Ok(e) => e,
        Err(e) => return internal(&e.to_string()),
    };
    let limits = merged_limits(existing, t, id, org_id.clone(), input);
    if let Err(e) = store.upsert_limits(&limits).await {
        return internal(&e.to_string());
    }
    audit(state, ctx, org_id.as_deref(), "limits.update", &format!("{}_limits", t.as_str()), id, serde_json::to_value(&limits).ok());
    (StatusCode::OK, Json(limits)).into_response()
}

pub(crate) async fn do_delete(
    state: &AppState,
    ctx: &KeyContext,
    store: &dyn ManagementStore,
    t: EntityType,
    id: &str,
    org_id: Option<&str>,
) -> Response {
    match store.delete_limits(t, id).await {
        Ok(true) => {
            audit(state, ctx, org_id, "limits.delete", &format!("{}_limits", t.as_str()), id, None);
            (StatusCode::OK, Json(DeletedResponse { deleted: true })).into_response()
        }
        Ok(false) => not_found("limits_not_found", &format!("no limits set for this {}", t.as_str())),
        Err(e) => internal(&e.to_string()),
    }
}

/// Zero spend and start a fresh window from now.
pub(crate) async fn do_reset(
    state: &AppState,
    ctx: &KeyContext,
    store: &dyn ManagementStore,
    t: EntityType,
    id: &str,
    org_id: Option<&str>,
) -> Response {
    let mut limits = match store.get_limits(t, id).await {
        Ok(Some(l)) => l,
        Ok(None) => return not_found("limits_not_found", &format!("no limits set for this {}", t.as_str())),
        Err(e) => return internal(&e.to_string()),
    };
    let now = Utc::now();
    limits.spend_usd = 0.0;
    limits.soft_budget_alerted_at = None;
    limits.budget_reset_at = budget::initial_reset_at(limits.budget_duration.as_deref(), now).unwrap_or(None);
    limits.updated_at = now;
    if let Err(e) = store.upsert_limits(&limits).await {
        return internal(&e.to_string());
    }
    audit(state, ctx, org_id, "limits.reset_spend", &format!("{}_limits", t.as_str()), id, None);
    (StatusCode::OK, Json(limits)).into_response()
}

// ─── Listing ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListLimitsQuery {
    /// key | org | app | member | end_user
    pub entity_type: Option<String>,
    /// Platform admin only: list another org's limits.
    pub org_id: Option<String>,
}

/// GET /v1/limits — Every limits row in the caller's organization.
#[utoipa::path(
    get,
    path = "/v1/limits",
    params(ListLimitsQuery),
    responses(
        (status = 200, description = "Limits rows", body = LimitsListResponse),
        (status = 400, description = "Bad entity_type", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn list_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Query(q): Query<ListLimitsQuery>,
) -> Response {
    let ctx = match ctx_of(ctx) { Ok(c) => c, Err(r) => return r };
    let store = match store(&state) { Ok(s) => s, Err(r) => return r };
    let entity_type = match q.entity_type.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => match EntityType::parse(s) {
            Some(t) => Some(t),
            None => return bad_request("invalid_entity_type", "entity_type must be key, org, app, member or end_user"),
        },
    };
    let org = match super::active_org(&ctx, q.org_id.as_deref()) { Ok(o) => o, Err(r) => return r };
    if let Err(r) = require_org(&state, &ctx, &org, Access::Read, Permission::OrgRead).await {
        return r;
    }
    match store.list_limits(Some(&org), entity_type).await {
        Ok(rows) => {
            let mut data = Vec::with_capacity(rows.len());
            for r in rows {
                // Roll lapsed windows so the listing shows current spend.
                match load_current(store, r.entity_type, &r.entity_id).await {
                    Ok(Some(l)) => data.push(l),
                    Ok(None) => {}
                    Err(e) => return internal(&e.to_string()),
                }
            }
            (StatusCode::OK, Json(LimitsListResponse { data })).into_response()
        }
        Err(e) => internal(&e.to_string()),
    }
}

/// GET /v1/limits/me — The limits that apply to the calling key or session:
/// its key, org, app and member rows (LiteLLM's `/key/info` for the caller).
#[utoipa::path(
    get,
    path = "/v1/limits/me",
    responses(
        (status = 200, description = "Limits applying to the caller", body = LimitsListResponse),
    ),
    tag = "Budgets"
)]
pub async fn my_limits(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>) -> Response {
    let ctx = match ctx_of(ctx) { Ok(c) => c, Err(r) => return r };
    let store = match store(&state) { Ok(s) => s, Err(r) => return r };
    let mut user_id = ctx.user.as_ref().map(|u| u.user_id.clone());
    if user_id.is_none() {
        if let Some(k) = ctx.key_id {
            user_id = match state.db.get_api_key(&k).await {
                Ok(k) => k.and_then(|k| k.owner_user_id),
                Err(e) => return internal(&e.to_string()),
            };
        }
    }
    let mut wanted: Vec<(EntityType, String)> = Vec::new();
    if let Some(k) = ctx.key_id { wanted.push((EntityType::Key, k.to_string())); }
    if let Some(o) = &ctx.org_id {
        wanted.push((EntityType::Org, o.clone()));
        if let Some(u) = &user_id { wanted.push((EntityType::Member, member_entity_id(o, u))); }
    }
    if let Some(a) = &ctx.app_id { wanted.push((EntityType::App, a.clone())); }
    let mut data = Vec::new();
    for (t, id) in wanted {
        match load_current(store, t, &id).await {
            Ok(Some(l)) => data.push(l),
            Ok(None) => {}
            Err(e) => return internal(&e.to_string()),
        }
    }
    (StatusCode::OK, Json(LimitsListResponse { data })).into_response()
}

// ─── Keys ────────────────────────────────────────────────────────────────────

/// Load a key and check the caller may manage it. Sessions need the any/own
/// key permission; API keys need the admin scope in the key's org.
async fn authorize_key(state: &AppState, ctx: &KeyContext, id: &str, access: Access) -> Result<crate::types::ApiKey, Response> {
    let uuid = Uuid::parse_str(id).map_err(|_| not_found("key_not_found", "API key not found"))?;
    let key = match state.db.get_api_key(&uuid).await {
        Ok(Some(k)) => k,
        Ok(None) => return Err(not_found("key_not_found", "API key not found")),
        Err(e) => return Err(internal(&e.to_string())),
    };
    if is_platform_admin(ctx) {
        return Ok(key);
    }
    // Cross-org keys look absent, as on the other key endpoints.
    if key.org_id.is_some() && key.org_id != ctx.org_id {
        return Err(not_found("key_not_found", "API key not found"));
    }
    if let Some(user) = &ctx.user {
        let (any, own) = match access {
            Access::Read => (Permission::KeyReadAny, Permission::KeyReadOwn),
            Access::Write => (Permission::KeyWriteAny, Permission::KeyWriteOwn),
        };
        let owns = key.owner_user_id.as_deref() == Some(user.user_id.as_str());
        if ctx.permissions.contains(&any) || (owns && ctx.permissions.contains(&own)) {
            return Ok(key);
        }
        return Err(forbidden(&format!("Permission '{}' required", any.as_str())));
    }
    if ctx.scopes.contains(&Scope::Admin) {
        return Ok(key);
    }
    Err(forbidden("admin scope required"))
}

macro_rules! key_handler {
    ($state:ident, $ctx:ident, $id:ident, $access:expr, |$store:ident, $key:ident| $body:expr) => {{
        let $ctx = match ctx_of($ctx) { Ok(c) => c, Err(r) => return r };
        let $store = match store(&$state) { Ok(s) => s, Err(r) => return r };
        let $key = match authorize_key(&$state, &$ctx, &$id, $access).await { Ok(k) => k, Err(r) => return r };
        $body
    }};
}

/// GET /v1/keys/{id}/limits
#[utoipa::path(
    get,
    path = "/v1/keys/{id}/limits",
    params(("id" = String, Path, description = "API key id")),
    responses(
        (status = 200, description = "Key limits", body = EntityLimits),
        (status = 404, description = "Key or limits not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn get_key_limits(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(id): Path<String>) -> Response {
    key_handler!(state, ctx, id, Access::Read, |store, key| do_get(store, EntityType::Key, &key.id.to_string()).await)
}

/// PUT /v1/keys/{id}/limits — Set the key's budget window, soft budget, model
/// allowlist, rpm and parallel caps. Independent of the key's lifetime
/// `token_quota`; both apply.
#[utoipa::path(
    put,
    path = "/v1/keys/{id}/limits",
    params(("id" = String, Path, description = "API key id")),
    request_body = LimitsInput,
    responses(
        (status = 200, description = "Updated key limits", body = EntityLimits),
        (status = 400, description = "Invalid limits", body = ErrorResponse),
        (status = 404, description = "Key not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn put_key_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Json(input): Json<LimitsInput>,
) -> Response {
    key_handler!(state, ctx, id, Access::Write, |store, key| {
        do_put(&state, &ctx, store, EntityType::Key, &key.id.to_string(), key.org_id.clone(), input).await
    })
}

/// DELETE /v1/keys/{id}/limits
#[utoipa::path(
    delete,
    path = "/v1/keys/{id}/limits",
    params(("id" = String, Path, description = "API key id")),
    responses(
        (status = 200, description = "Limits removed", body = DeletedResponse),
        (status = 404, description = "Key or limits not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn delete_key_limits(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(id): Path<String>) -> Response {
    key_handler!(state, ctx, id, Access::Write, |store, key| {
        do_delete(&state, &ctx, store, EntityType::Key, &key.id.to_string(), key.org_id.as_deref()).await
    })
}

/// POST /v1/keys/{id}/limits/reset — Zero the key's window spend.
#[utoipa::path(
    post,
    path = "/v1/keys/{id}/limits/reset",
    params(("id" = String, Path, description = "API key id")),
    responses(
        (status = 200, description = "Limits with spend reset", body = EntityLimits),
        (status = 404, description = "Key or limits not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn reset_key_limits(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(id): Path<String>) -> Response {
    key_handler!(state, ctx, id, Access::Write, |store, key| {
        do_reset(&state, &ctx, store, EntityType::Key, &key.id.to_string(), key.org_id.as_deref()).await
    })
}

async fn set_key_active(state: &AppState, ctx: &KeyContext, key: crate::types::ApiKey, active: bool) -> Response {
    let req = crate::types::UpdateApiKeyRequest {
        name: None,
        token_quota: None,
        rpm_limit: None,
        scopes: None,
        webhook_url: None,
        expires_at: None,
        is_active: Some(active),
    };
    match state.db.update_api_key(&key.id, &req).await {
        Ok(Some(k)) => {
            audit(state, ctx, k.org_id.as_deref(), if active { "key.unblock" } else { "key.block" }, "api_key", &k.id.to_string(), None);
            (StatusCode::OK, Json(serde_json::json!({ "id": k.id, "is_active": k.is_active }))).into_response()
        }
        Ok(None) => not_found("key_not_found", "API key not found"),
        Err(e) => internal(&e.to_string()),
    }
}

/// POST /v1/keys/{id}/block — Refuse every request made with this key until unblocked.
#[utoipa::path(
    post,
    path = "/v1/keys/{id}/block",
    params(("id" = String, Path, description = "API key id")),
    responses(
        (status = 200, description = "Key blocked", body = KeyActiveResponse),
        (status = 404, description = "Key not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn block_key(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(id): Path<String>) -> Response {
    key_handler!(state, ctx, id, Access::Write, |_store, key| set_key_active(&state, &ctx, key, false).await)
}

/// POST /v1/keys/{id}/unblock
#[utoipa::path(
    post,
    path = "/v1/keys/{id}/unblock",
    params(("id" = String, Path, description = "API key id")),
    responses(
        (status = 200, description = "Key unblocked", body = KeyActiveResponse),
        (status = 404, description = "Key not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn unblock_key(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(id): Path<String>) -> Response {
    key_handler!(state, ctx, id, Access::Write, |_store, key| set_key_active(&state, &ctx, key, true).await)
}

/// Response for key block/unblock.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct KeyActiveResponse {
    pub id: Uuid,
    pub is_active: bool,
}

// ─── Orgs, members, apps ─────────────────────────────────────────────────────

macro_rules! org_handler {
    ($state:ident, $ctx:ident, $org:expr, $access:expr, $perm:expr, |$store:ident| $body:expr) => {{
        let $ctx = match ctx_of($ctx) { Ok(c) => c, Err(r) => return r };
        let $store = match store(&$state) { Ok(s) => s, Err(r) => return r };
        if let Err(r) = require_org(&$state, &$ctx, $org, $access, $perm).await {
            return r;
        }
        $body
    }};
}

async fn org_exists(state: &AppState, org_id: &str) -> Result<(), Response> {
    match state.db.get_organization(org_id).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(not_found("org_not_found", "Organization not found")),
        Err(e) => Err(internal(&e.to_string())),
    }
}

/// GET /v1/orgs/{id}/limits
#[utoipa::path(
    get,
    path = "/v1/orgs/{id}/limits",
    params(("id" = String, Path, description = "Organization id")),
    responses(
        (status = 200, description = "Org limits", body = EntityLimits),
        (status = 404, description = "No limits set", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn get_org_limits(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(id): Path<String>) -> Response {
    org_handler!(state, ctx, &id, Access::Read, Permission::OrgRead, |store| do_get(store, EntityType::Org, &id).await)
}

/// PUT /v1/orgs/{id}/limits — Budget and caps shared by everything in the org.
#[utoipa::path(
    put,
    path = "/v1/orgs/{id}/limits",
    params(("id" = String, Path, description = "Organization id")),
    request_body = LimitsInput,
    responses(
        (status = 200, description = "Updated org limits", body = EntityLimits),
        (status = 400, description = "Invalid limits", body = ErrorResponse),
        (status = 404, description = "Org not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn put_org_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Json(input): Json<LimitsInput>,
) -> Response {
    org_handler!(state, ctx, &id, Access::Write, Permission::OrgWrite, |store| {
        if let Err(r) = org_exists(&state, &id).await { return r; }
        do_put(&state, &ctx, store, EntityType::Org, &id, Some(id.clone()), input).await
    })
}

/// DELETE /v1/orgs/{id}/limits
#[utoipa::path(
    delete,
    path = "/v1/orgs/{id}/limits",
    params(("id" = String, Path, description = "Organization id")),
    responses(
        (status = 200, description = "Limits removed", body = DeletedResponse),
        (status = 404, description = "No limits set", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn delete_org_limits(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(id): Path<String>) -> Response {
    org_handler!(state, ctx, &id, Access::Write, Permission::OrgWrite, |store| {
        do_delete(&state, &ctx, store, EntityType::Org, &id, Some(&id)).await
    })
}

/// POST /v1/orgs/{id}/limits/reset
#[utoipa::path(
    post,
    path = "/v1/orgs/{id}/limits/reset",
    params(("id" = String, Path, description = "Organization id")),
    responses(
        (status = 200, description = "Limits with spend reset", body = EntityLimits),
        (status = 404, description = "No limits set", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn reset_org_limits(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(id): Path<String>) -> Response {
    org_handler!(state, ctx, &id, Access::Write, Permission::OrgWrite, |store| {
        do_reset(&state, &ctx, store, EntityType::Org, &id, Some(&id)).await
    })
}

async fn member_exists(state: &AppState, org_id: &str, user_id: &str) -> Result<(), Response> {
    match state.db.get_membership(org_id, user_id).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(not_found("member_not_found", "Member not found")),
        Err(e) => Err(internal(&e.to_string())),
    }
}

/// GET /v1/orgs/{id}/members/{user_id}/limits
#[utoipa::path(
    get,
    path = "/v1/orgs/{id}/members/{user_id}/limits",
    params(("id" = String, Path, description = "Organization id"), ("user_id" = String, Path, description = "Member user id")),
    responses(
        (status = 200, description = "Member limits", body = EntityLimits),
        (status = 404, description = "No limits set", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn get_member_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path((id, user_id)): Path<(String, String)>,
) -> Response {
    org_handler!(state, ctx, &id, Access::Read, Permission::MemberRead, |store| {
        do_get(store, EntityType::Member, &member_entity_id(&id, &user_id)).await
    })
}

/// PUT /v1/orgs/{id}/members/{user_id}/limits — A member's budget inside the
/// org, covering their session generations and every key they own.
#[utoipa::path(
    put,
    path = "/v1/orgs/{id}/members/{user_id}/limits",
    params(("id" = String, Path, description = "Organization id"), ("user_id" = String, Path, description = "Member user id")),
    request_body = LimitsInput,
    responses(
        (status = 200, description = "Updated member limits", body = EntityLimits),
        (status = 400, description = "Invalid limits", body = ErrorResponse),
        (status = 404, description = "Member not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn put_member_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path((id, user_id)): Path<(String, String)>,
    Json(input): Json<LimitsInput>,
) -> Response {
    org_handler!(state, ctx, &id, Access::Write, Permission::MemberWrite, |store| {
        if let Err(r) = member_exists(&state, &id, &user_id).await { return r; }
        do_put(&state, &ctx, store, EntityType::Member, &member_entity_id(&id, &user_id), Some(id.clone()), input).await
    })
}

/// DELETE /v1/orgs/{id}/members/{user_id}/limits
#[utoipa::path(
    delete,
    path = "/v1/orgs/{id}/members/{user_id}/limits",
    params(("id" = String, Path, description = "Organization id"), ("user_id" = String, Path, description = "Member user id")),
    responses(
        (status = 200, description = "Limits removed", body = DeletedResponse),
        (status = 404, description = "No limits set", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn delete_member_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path((id, user_id)): Path<(String, String)>,
) -> Response {
    org_handler!(state, ctx, &id, Access::Write, Permission::MemberWrite, |store| {
        do_delete(&state, &ctx, store, EntityType::Member, &member_entity_id(&id, &user_id), Some(&id)).await
    })
}

/// POST /v1/orgs/{id}/members/{user_id}/limits/reset
#[utoipa::path(
    post,
    path = "/v1/orgs/{id}/members/{user_id}/limits/reset",
    params(("id" = String, Path, description = "Organization id"), ("user_id" = String, Path, description = "Member user id")),
    responses(
        (status = 200, description = "Limits with spend reset", body = EntityLimits),
        (status = 404, description = "No limits set", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn reset_member_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path((id, user_id)): Path<(String, String)>,
) -> Response {
    org_handler!(state, ctx, &id, Access::Write, Permission::MemberWrite, |store| {
        do_reset(&state, &ctx, store, EntityType::Member, &member_entity_id(&id, &user_id), Some(&id)).await
    })
}

async fn app_org(state: &AppState, app_id: &str) -> Result<String, Response> {
    match state.db.get_application(app_id).await {
        Ok(Some(a)) => Ok(a.org_id),
        Ok(None) => Err(not_found("app_not_found", "Application not found")),
        Err(e) => Err(internal(&e.to_string())),
    }
}

macro_rules! app_handler {
    ($state:ident, $ctx:ident, $app:ident, $access:expr, $perm:expr, |$store:ident, $org:ident| $body:expr) => {{
        let $org = match app_org(&$state, &$app).await { Ok(o) => o, Err(r) => return r };
        org_handler!($state, $ctx, &$org, $access, $perm, |$store| $body)
    }};
}

/// GET /v1/apps/{app_id}/limits
#[utoipa::path(
    get,
    path = "/v1/apps/{app_id}/limits",
    params(("app_id" = String, Path, description = "Application id")),
    responses(
        (status = 200, description = "App limits", body = EntityLimits),
        (status = 404, description = "App or limits not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn get_app_limits(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(app_id): Path<String>) -> Response {
    app_handler!(state, ctx, app_id, Access::Read, Permission::AppRead, |store, _org| do_get(store, EntityType::App, &app_id).await)
}

/// PUT /v1/apps/{app_id}/limits — Budget and caps for one application (LiteLLM team budget).
#[utoipa::path(
    put,
    path = "/v1/apps/{app_id}/limits",
    params(("app_id" = String, Path, description = "Application id")),
    request_body = LimitsInput,
    responses(
        (status = 200, description = "Updated app limits", body = EntityLimits),
        (status = 400, description = "Invalid limits", body = ErrorResponse),
        (status = 404, description = "App not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn put_app_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(app_id): Path<String>,
    Json(input): Json<LimitsInput>,
) -> Response {
    app_handler!(state, ctx, app_id, Access::Write, Permission::AppWrite, |store, org| {
        do_put(&state, &ctx, store, EntityType::App, &app_id, Some(org.clone()), input).await
    })
}

/// DELETE /v1/apps/{app_id}/limits
#[utoipa::path(
    delete,
    path = "/v1/apps/{app_id}/limits",
    params(("app_id" = String, Path, description = "Application id")),
    responses(
        (status = 200, description = "Limits removed", body = DeletedResponse),
        (status = 404, description = "App or limits not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn delete_app_limits(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(app_id): Path<String>) -> Response {
    app_handler!(state, ctx, app_id, Access::Write, Permission::AppWrite, |store, org| {
        do_delete(&state, &ctx, store, EntityType::App, &app_id, Some(&org)).await
    })
}

/// POST /v1/apps/{app_id}/limits/reset
#[utoipa::path(
    post,
    path = "/v1/apps/{app_id}/limits/reset",
    params(("app_id" = String, Path, description = "Application id")),
    responses(
        (status = 200, description = "Limits with spend reset", body = EntityLimits),
        (status = 404, description = "App or limits not found", body = ErrorResponse),
    ),
    tag = "Budgets"
)]
pub async fn reset_app_limits(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Path(app_id): Path<String>) -> Response {
    app_handler!(state, ctx, app_id, Access::Write, Permission::AppWrite, |store, org| {
        do_reset(&state, &ctx, store, EntityType::App, &app_id, Some(&org)).await
    })
}
