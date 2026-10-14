//! Management API handlers (LiteLLM parity): limits and budgets, end users,
//! spend reports, and the `/v1/admin/*` runtime configuration surface.
//!
//! Authorization, in one place:
//! - **Gateway admin** (`/v1/admin/*`): the platform admin (master key). In
//!   single-tenant mode also an owner/admin session or an `admin`-scope key.
//! - **Org-scoped** (limits, end users, spend): the platform admin for any org;
//!   a session user by membership role; an API key only inside its own org,
//!   with the `admin` scope to write and `read` to read.

pub mod admin;
pub mod end_users;
pub mod limits;
pub mod spend;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::middleware::{is_platform_admin, AppState, KeyContext, Scope};
use crate::auth::permissions::{role_has, Permission};
use crate::management::store::ManagementStore;

/// Response for management DELETE endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeletedResponse {
    pub deleted: bool,
}

pub(crate) fn err(status: StatusCode, kind: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": { "message": message, "type": kind, "code": status.as_u16() } })),
    )
        .into_response()
}

pub(crate) fn forbidden(message: &str) -> Response {
    err(StatusCode::FORBIDDEN, "forbidden", message)
}

pub(crate) fn bad_request(kind: &str, message: &str) -> Response {
    err(StatusCode::BAD_REQUEST, kind, message)
}

pub(crate) fn not_found(kind: &str, message: &str) -> Response {
    err(StatusCode::NOT_FOUND, kind, message)
}

pub(crate) fn internal(detail: &str) -> Response {
    tracing::error!(detail, "management handler internal error");
    err(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "internal server error")
}

pub(crate) fn unauthenticated() -> Response {
    err(StatusCode::UNAUTHORIZED, "unauthorized", "authentication required")
}

/// The management store, or 501 when the database backend has none.
pub(crate) fn store(state: &AppState) -> Result<&dyn ManagementStore, Response> {
    state.db.management().ok_or_else(|| {
        err(StatusCode::NOT_IMPLEMENTED, "not_implemented", "this database backend does not support the management API")
    })
}

pub(crate) fn ctx_of(ctx: Option<axum::Extension<KeyContext>>) -> Result<KeyContext, Response> {
    ctx.map(|axum::Extension(c)| c).ok_or_else(unauthenticated)
}

fn has_scope(ctx: &KeyContext, scope: Scope) -> bool {
    ctx.scopes.contains(&scope)
}

/// Gate for `/v1/admin/*`.
pub(crate) fn require_gateway_admin(state: &AppState, ctx: &KeyContext) -> Result<(), Response> {
    if is_platform_admin(ctx) {
        return Ok(());
    }
    if state.mode == crate::config::Mode::SingleTenant {
        if ctx.user.is_some() {
            if ctx.permissions.contains(&Permission::SystemConfig) {
                return Ok(());
            }
        } else if ctx.key_id.is_some() && has_scope(ctx, Scope::Admin) {
            return Ok(());
        }
    }
    Err(forbidden("gateway admin required"))
}

/// Access level an org-scoped endpoint needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    Read,
    Write,
}

/// Gate for an org-scoped resource. `perm` is the membership permission a
/// session user needs for this access level.
pub(crate) async fn require_org(
    state: &AppState,
    ctx: &KeyContext,
    org_id: &str,
    access: Access,
    perm: Permission,
) -> Result<(), Response> {
    if is_platform_admin(ctx) {
        return Ok(());
    }
    if let Some(user) = &ctx.user {
        return match state.db.get_membership(org_id, &user.user_id).await {
            Ok(Some(role)) if role_has(role, perm) => Ok(()),
            Ok(Some(_)) => Err(forbidden("insufficient role for this organization")),
            Ok(None) => Err(forbidden("not a member of this organization")),
            Err(e) => Err(internal(&e.to_string())),
        };
    }
    if ctx.key_id.is_some() {
        if ctx.org_id.as_deref() != Some(org_id) {
            return Err(forbidden("this API key belongs to a different organization"));
        }
        let scope = if access == Access::Write { Scope::Admin } else { Scope::Read };
        if !has_scope(ctx, scope) && !has_scope(ctx, Scope::Admin) {
            return Err(forbidden(if access == Access::Write { "admin scope required" } else { "read scope required" }));
        }
        return Ok(());
    }
    Err(forbidden("organization access denied"))
}

/// The org a request acts on: the caller's active org, or — for the platform
/// admin only — an explicit `org_id`.
pub(crate) fn active_org(ctx: &KeyContext, explicit: Option<&str>) -> Result<String, Response> {
    if let Some(o) = explicit.filter(|s| !s.is_empty()) {
        if is_platform_admin(ctx) || ctx.org_id.as_deref() == Some(o) {
            return Ok(o.to_string());
        }
        return Err(forbidden("org_id may only name your own organization"));
    }
    ctx.org_id.clone().ok_or_else(|| forbidden("no active organization"))
}

pub(crate) fn actor_label(ctx: &KeyContext) -> String {
    ctx.user
        .as_ref()
        .map(|u| u.email.clone())
        .or_else(|| ctx.key_id.map(|k| k.to_string()))
        .unwrap_or_else(|| "master-key".to_string())
}

/// Record a management mutation in the audit log (fire-and-forget).
pub(crate) fn audit(
    state: &AppState,
    ctx: &KeyContext,
    org_id: Option<&str>,
    action: &str,
    target_type: &str,
    target_id: &str,
    after: Option<serde_json::Value>,
) {
    let db = state.db.clone();
    let entry = crate::types::AuditLogEntry {
        id: format!("audit-{}", uuid::Uuid::new_v4()),
        actor_key_id: ctx.key_id.map(|k| k.to_string()),
        actor_label: actor_label(ctx),
        action: action.to_string(),
        target_type: target_type.to_string(),
        target_id: target_id.to_string(),
        before_json: None,
        after_json: after.map(|v| v.to_string()),
        created_at: chrono::Utc::now(),
        org_id: org_id.map(str::to_string).or_else(|| ctx.org_id.clone()),
    };
    tokio::spawn(async move {
        if let Err(e) = db.insert_audit_log(&entry).await {
            tracing::warn!(error = %e, "failed to audit management action");
        }
    });
}

/// Every management route. Merged into the authenticated router, so the auth
/// middleware has already put a `KeyContext` on each request.
pub fn routes(state: Arc<AppState>) -> axum::Router<Arc<AppState>> {
    use axum::routing::{get, post};
    use crate::api::middleware::csrf_middleware;

    axum::Router::new()
        // Limits / budgets
        .route("/v1/limits", get(limits::list_limits))
        .route("/v1/limits/me", get(limits::my_limits))
        .route(
            "/v1/keys/{id}/limits",
            get(limits::get_key_limits).put(limits::put_key_limits).delete(limits::delete_key_limits),
        )
        .route("/v1/keys/{id}/limits/reset", post(limits::reset_key_limits))
        .route("/v1/keys/{id}/block", post(limits::block_key))
        .route("/v1/keys/{id}/unblock", post(limits::unblock_key))
        .route(
            "/v1/orgs/{id}/limits",
            get(limits::get_org_limits).put(limits::put_org_limits).delete(limits::delete_org_limits),
        )
        .route("/v1/orgs/{id}/limits/reset", post(limits::reset_org_limits))
        .route(
            "/v1/orgs/{id}/members/{user_id}/limits",
            get(limits::get_member_limits).put(limits::put_member_limits).delete(limits::delete_member_limits),
        )
        .route("/v1/orgs/{id}/members/{user_id}/limits/reset", post(limits::reset_member_limits))
        .route(
            "/v1/apps/{app_id}/limits",
            get(limits::get_app_limits).put(limits::put_app_limits).delete(limits::delete_app_limits),
        )
        .route("/v1/apps/{app_id}/limits/reset", post(limits::reset_app_limits))
        // End users
        .route("/v1/end-users", get(end_users::list_end_users).post(end_users::create_end_user))
        .route(
            "/v1/end-users/{end_user_id}",
            get(end_users::get_end_user).patch(end_users::update_end_user).delete(end_users::delete_end_user),
        )
        .route("/v1/end-users/{end_user_id}/block", post(end_users::block_end_user))
        .route("/v1/end-users/{end_user_id}/unblock", post(end_users::unblock_end_user))
        .route(
            "/v1/end-users/{end_user_id}/limits",
            get(end_users::get_end_user_limits)
                .put(end_users::put_end_user_limits)
                .delete(end_users::delete_end_user_limits),
        )
        .route("/v1/end-users/{end_user_id}/limits/reset", post(end_users::reset_end_user_limits))
        // Spend
        .route("/v1/spend/report", get(spend::spend_report))
        .route("/v1/spend/logs", get(spend::spend_logs))
        // Gateway admin
        .route("/v1/admin/models", get(admin::list_models).post(admin::create_model))
        .route(
            "/v1/admin/models/{*id}",
            get(admin::get_model).put(admin::replace_model).patch(admin::patch_model).delete(admin::delete_model),
        )
        .route("/v1/admin/providers", get(admin::list_providers))
        .route(
            "/v1/admin/providers/{name}",
            get(admin::get_provider).put(admin::put_provider).delete(admin::delete_provider),
        )
        .route("/v1/admin/routes", get(admin::get_routes).put(admin::put_routes).delete(admin::delete_routes))
        .route("/v1/admin/settings", get(admin::get_settings).put(admin::put_settings))
        .layer(axum::middleware::from_fn_with_state(state, csrf_middleware))
}
