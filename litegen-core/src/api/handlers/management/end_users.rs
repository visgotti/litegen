//! End users (LiteLLM "customers"): the callers of a tenant's own product,
//! named per request with the `x-litegen-end-user` header. Scoped to the
//! caller's active organization.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use serde::Deserialize;

use super::limits::{do_delete, do_get, do_put, do_reset};
use super::{active_org, audit, bad_request, ctx_of, internal, not_found, require_org, store, Access, DeletedResponse};
use crate::api::middleware::{AppState, KeyContext};
use crate::auth::permissions::Permission;
use crate::management::enforce::load_current;
use crate::management::store::ManagementStore;
use crate::management::types::*;
use crate::types::ErrorResponse;

const MAX_ID_LEN: usize = 256;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct OrgQuery {
    /// Platform admin only: act on another org.
    pub org_id: Option<String>,
}

/// Resolve the org and check access, returning (ctx, store, org).
async fn scope<'a>(
    state: &'a AppState,
    ctx: Option<Extension<KeyContext>>,
    explicit_org: Option<&str>,
    access: Access,
) -> Result<(KeyContext, &'a dyn ManagementStore, String), Response> {
    let ctx = ctx_of(ctx)?;
    let store = store(state)?;
    let org = active_org(&ctx, explicit_org)?;
    let perm = if access == Access::Write { Permission::OrgWrite } else { Permission::OrgRead };
    require_org(state, &ctx, &org, access, perm).await?;
    Ok((ctx, store, org))
}

async fn view(store: &dyn ManagementStore, user: EndUser) -> Result<EndUserView, sqlx::Error> {
    let limits = load_current(store, EntityType::EndUser, &end_user_entity_id(&user.org_id, &user.id)).await?;
    Ok(EndUserView { end_user: user, limits })
}

async fn existing(store: &dyn ManagementStore, org: &str, id: &str) -> Result<EndUser, Response> {
    match store.get_end_user(org, id).await {
        Ok(Some(u)) => Ok(u),
        Ok(None) => Err(not_found("end_user_not_found", &format!("end user '{id}' not found"))),
        Err(e) => Err(internal(&e.to_string())),
    }
}

/// GET /v1/end-users — End users seen in, or created for, the organization.
#[utoipa::path(
    get,
    path = "/v1/end-users",
    params(OrgQuery),
    responses((status = 200, description = "End users", body = EndUserListResponse)),
    tag = "End Users"
)]
pub async fn list_end_users(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Query(q): Query<OrgQuery>) -> Response {
    let (_ctx, store, org) = match scope(&state, ctx, q.org_id.as_deref(), Access::Read).await { Ok(v) => v, Err(r) => return r };
    let users = match store.list_end_users(&org).await { Ok(u) => u, Err(e) => return internal(&e.to_string()) };
    let mut data = Vec::with_capacity(users.len());
    for u in users {
        match view(store, u).await {
            Ok(v) => data.push(v),
            Err(e) => return internal(&e.to_string()),
        }
    }
    (StatusCode::OK, Json(EndUserListResponse { data })).into_response()
}

/// POST /v1/end-users — Register an end user ahead of its first request,
/// optionally with limits.
#[utoipa::path(
    post,
    path = "/v1/end-users",
    params(OrgQuery),
    request_body = CreateEndUserRequest,
    responses(
        (status = 201, description = "End user created", body = EndUserView),
        (status = 400, description = "Invalid end user", body = ErrorResponse),
        (status = 409, description = "Already exists", body = ErrorResponse),
    ),
    tag = "End Users"
)]
pub async fn create_end_user(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Query(q): Query<OrgQuery>,
    Json(body): Json<CreateEndUserRequest>,
) -> Response {
    let (ctx, store, org) = match scope(&state, ctx, q.org_id.as_deref(), Access::Write).await { Ok(v) => v, Err(r) => return r };
    let id = body.id.trim().to_string();
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return bad_request("invalid_end_user", "id must be 1-256 characters");
    }
    if body.metadata.as_ref().is_some_and(|m| !m.is_object()) {
        return bad_request("invalid_end_user", "metadata must be a JSON object");
    }
    if let Some(limits) = &body.limits {
        if let Err(r) = super::limits::validate_input(&state, limits) { return r; }
    }
    match store.get_end_user(&org, &id).await {
        Ok(Some(_)) => return super::err(StatusCode::CONFLICT, "end_user_exists", &format!("end user '{id}' already exists")),
        Ok(None) => {}
        Err(e) => return internal(&e.to_string()),
    }
    let now = Utc::now();
    let user = EndUser {
        id: id.clone(),
        org_id: org.clone(),
        alias: body.alias,
        blocked: body.blocked,
        metadata: body.metadata,
        created_at: now,
        updated_at: now,
        last_seen_at: None,
    };
    if let Err(e) = store.upsert_end_user(&user).await {
        return internal(&e.to_string());
    }
    if let Some(input) = body.limits {
        let entity_id = end_user_entity_id(&org, &id);
        let limits = super::limits::merged_limits(None, EntityType::EndUser, &entity_id, Some(org.clone()), input);
        if let Err(e) = store.upsert_limits(&limits).await {
            return internal(&e.to_string());
        }
    }
    audit(&state, &ctx, Some(&org), "end_user.create", "end_user", &id, serde_json::to_value(&user).ok());
    match view(store, user).await {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => internal(&e.to_string()),
    }
}

/// GET /v1/end-users/{end_user_id}
#[utoipa::path(
    get,
    path = "/v1/end-users/{end_user_id}",
    params(("end_user_id" = String, Path, description = "End user id"), OrgQuery),
    responses(
        (status = 200, description = "End user", body = EndUserView),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "End Users"
)]
pub async fn get_end_user(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Query(q): Query<OrgQuery>,
) -> Response {
    let (_ctx, store, org) = match scope(&state, ctx, q.org_id.as_deref(), Access::Read).await { Ok(v) => v, Err(r) => return r };
    let user = match existing(store, &org, &id).await { Ok(u) => u, Err(r) => return r };
    match view(store, user).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => internal(&e.to_string()),
    }
}

/// PATCH /v1/end-users/{end_user_id} — Change alias, blocked or metadata.
#[utoipa::path(
    patch,
    path = "/v1/end-users/{end_user_id}",
    params(("end_user_id" = String, Path, description = "End user id"), OrgQuery),
    request_body = UpdateEndUserRequest,
    responses(
        (status = 200, description = "Updated end user", body = EndUserView),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "End Users"
)]
pub async fn update_end_user(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Query(q): Query<OrgQuery>,
    Json(body): Json<UpdateEndUserRequest>,
) -> Response {
    let (ctx, store, org) = match scope(&state, ctx, q.org_id.as_deref(), Access::Write).await { Ok(v) => v, Err(r) => return r };
    let mut user = match existing(store, &org, &id).await { Ok(u) => u, Err(r) => return r };
    if body.metadata.as_ref().is_some_and(|m| !m.is_object()) {
        return bad_request("invalid_end_user", "metadata must be a JSON object");
    }
    if let Some(a) = body.alias { user.alias = Some(a).filter(|s| !s.is_empty()); }
    if let Some(b) = body.blocked { user.blocked = b; }
    if let Some(m) = body.metadata { user.metadata = Some(m); }
    user.updated_at = Utc::now();
    if let Err(e) = store.upsert_end_user(&user).await {
        return internal(&e.to_string());
    }
    audit(&state, &ctx, Some(&org), "end_user.update", "end_user", &id, serde_json::to_value(&user).ok());
    match view(store, user).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => internal(&e.to_string()),
    }
}

/// DELETE /v1/end-users/{end_user_id} — Remove the end user and its limits.
/// Its spend history stays in the request logs.
#[utoipa::path(
    delete,
    path = "/v1/end-users/{end_user_id}",
    params(("end_user_id" = String, Path, description = "End user id"), OrgQuery),
    responses(
        (status = 200, description = "Deleted", body = DeletedResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "End Users"
)]
pub async fn delete_end_user(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Query(q): Query<OrgQuery>,
) -> Response {
    let (ctx, store, org) = match scope(&state, ctx, q.org_id.as_deref(), Access::Write).await { Ok(v) => v, Err(r) => return r };
    match store.delete_end_user(&org, &id).await {
        Ok(true) => {
            let _ = store.delete_limits(EntityType::EndUser, &end_user_entity_id(&org, &id)).await;
            audit(&state, &ctx, Some(&org), "end_user.delete", "end_user", &id, None);
            (StatusCode::OK, Json(DeletedResponse { deleted: true })).into_response()
        }
        Ok(false) => not_found("end_user_not_found", &format!("end user '{id}' not found")),
        Err(e) => internal(&e.to_string()),
    }
}

async fn set_blocked(state: &AppState, ctx: Option<Extension<KeyContext>>, id: String, org: Option<String>, blocked: bool) -> Response {
    let (ctx, store, org) = match scope(state, ctx, org.as_deref(), Access::Write).await { Ok(v) => v, Err(r) => return r };
    let now = Utc::now();
    // Blocking an id never seen before registers it, so it is refused from its first request.
    let mut user = match store.get_end_user(&org, &id).await {
        Ok(Some(u)) => u,
        Ok(None) => EndUser { id: id.clone(), org_id: org.clone(), alias: None, blocked, metadata: None, created_at: now, updated_at: now, last_seen_at: None },
        Err(e) => return internal(&e.to_string()),
    };
    user.blocked = blocked;
    user.updated_at = now;
    if let Err(e) = store.upsert_end_user(&user).await {
        return internal(&e.to_string());
    }
    audit(state, &ctx, Some(&org), if blocked { "end_user.block" } else { "end_user.unblock" }, "end_user", &id, None);
    match view(store, user).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => internal(&e.to_string()),
    }
}

/// POST /v1/end-users/{end_user_id}/block — Refuse this end user's requests (403 `end_user_blocked`).
#[utoipa::path(
    post,
    path = "/v1/end-users/{end_user_id}/block",
    params(("end_user_id" = String, Path, description = "End user id"), OrgQuery),
    responses((status = 200, description = "Blocked", body = EndUserView)),
    tag = "End Users"
)]
pub async fn block_end_user(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Query(q): Query<OrgQuery>,
) -> Response {
    set_blocked(&state, ctx, id, q.org_id, true).await
}

/// POST /v1/end-users/{end_user_id}/unblock
#[utoipa::path(
    post,
    path = "/v1/end-users/{end_user_id}/unblock",
    params(("end_user_id" = String, Path, description = "End user id"), OrgQuery),
    responses((status = 200, description = "Unblocked", body = EndUserView)),
    tag = "End Users"
)]
pub async fn unblock_end_user(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Query(q): Query<OrgQuery>,
) -> Response {
    set_blocked(&state, ctx, id, q.org_id, false).await
}

/// GET /v1/end-users/{end_user_id}/limits
#[utoipa::path(
    get,
    path = "/v1/end-users/{end_user_id}/limits",
    params(("end_user_id" = String, Path, description = "End user id"), OrgQuery),
    responses(
        (status = 200, description = "End-user limits", body = EntityLimits),
        (status = 404, description = "No limits set", body = ErrorResponse),
    ),
    tag = "End Users"
)]
pub async fn get_end_user_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Query(q): Query<OrgQuery>,
) -> Response {
    let (_ctx, store, org) = match scope(&state, ctx, q.org_id.as_deref(), Access::Read).await { Ok(v) => v, Err(r) => return r };
    do_get(store, EntityType::EndUser, &end_user_entity_id(&org, &id)).await
}

/// PUT /v1/end-users/{end_user_id}/limits — An end user's budget and caps.
/// Registers the end user if it was never seen.
#[utoipa::path(
    put,
    path = "/v1/end-users/{end_user_id}/limits",
    params(("end_user_id" = String, Path, description = "End user id"), OrgQuery),
    request_body = LimitsInput,
    responses(
        (status = 200, description = "Updated limits", body = EntityLimits),
        (status = 400, description = "Invalid limits", body = ErrorResponse),
    ),
    tag = "End Users"
)]
pub async fn put_end_user_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Query(q): Query<OrgQuery>,
    Json(input): Json<LimitsInput>,
) -> Response {
    let (ctx, store, org) = match scope(&state, ctx, q.org_id.as_deref(), Access::Write).await { Ok(v) => v, Err(r) => return r };
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return bad_request("invalid_end_user", "id must be 1-256 characters");
    }
    if let Err(e) = store.touch_end_user(&org, &id, Utc::now()).await {
        return internal(&e.to_string());
    }
    do_put(&state, &ctx, store, EntityType::EndUser, &end_user_entity_id(&org, &id), Some(org.clone()), input).await
}

/// DELETE /v1/end-users/{end_user_id}/limits
#[utoipa::path(
    delete,
    path = "/v1/end-users/{end_user_id}/limits",
    params(("end_user_id" = String, Path, description = "End user id"), OrgQuery),
    responses(
        (status = 200, description = "Limits removed", body = DeletedResponse),
        (status = 404, description = "No limits set", body = ErrorResponse),
    ),
    tag = "End Users"
)]
pub async fn delete_end_user_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Query(q): Query<OrgQuery>,
) -> Response {
    let (ctx, store, org) = match scope(&state, ctx, q.org_id.as_deref(), Access::Write).await { Ok(v) => v, Err(r) => return r };
    do_delete(&state, &ctx, store, EntityType::EndUser, &end_user_entity_id(&org, &id), Some(&org)).await
}

/// POST /v1/end-users/{end_user_id}/limits/reset
#[utoipa::path(
    post,
    path = "/v1/end-users/{end_user_id}/limits/reset",
    params(("end_user_id" = String, Path, description = "End user id"), OrgQuery),
    responses(
        (status = 200, description = "Limits with spend reset", body = EntityLimits),
        (status = 404, description = "No limits set", body = ErrorResponse),
    ),
    tag = "End Users"
)]
pub async fn reset_end_user_limits(
    State(state): State<Arc<AppState>>,
    ctx: Option<Extension<KeyContext>>,
    Path(id): Path<String>,
    Query(q): Query<OrgQuery>,
) -> Response {
    let (ctx, store, org) = match scope(&state, ctx, q.org_id.as_deref(), Access::Write).await { Ok(v) => v, Err(r) => return r };
    do_reset(&state, &ctx, store, EntityType::EndUser, &end_user_entity_id(&org, &id), Some(&org)).await
}
