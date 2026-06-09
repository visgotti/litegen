//! Dashboard-facing CRUD for Organizations, Applications, Members, and BYO
//! provider credentials. Every handler is session-authenticated and authorized
//! against the caller's MEMBERSHIP role in the org named in the request PATH
//! (not the active-org header), so a header active-org can never be used to act
//! on a different organization.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::api::middleware::{AppState, KeyContext};
use crate::auth::permissions::{role_has, Permission};
use crate::auth::tokens::generate_session_token;
use crate::db::DatabaseStore;
use crate::types::{
    Application, Invitation, Organization, OrganizationMember, ProviderCredentialInfo, Role,
};
use crate::util::slug::{slugify, unique_org_slug};

// ─── Error helpers (mirror users.rs `err`) ─────────────────────────────────────

fn err(code: StatusCode, error_code: &str, message: &str) -> Response {
    (
        code,
        Json(json!({
            "error": {
                "code": error_code,
                "message": message,
                "type": "request_error"
            }
        })),
    )
        .into_response()
}

fn forbidden(message: &str) -> Response {
    err(StatusCode::FORBIDDEN, "forbidden", message)
}

fn internal_error(message: &str) -> Response {
    err(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
}

// ─── Slug helpers ──────────────────────────────────────────────────────────────
// `slugify` and `unique_org_slug` live in `crate::util::slug` and are shared
// with `auth_password.rs`. Only the per-org app-slug helper remains here
// because it scopes uniqueness to a single org via `list_apps_for_org`.

/// App slugs are unique within an org. Append `-2`, `-3`, … until free.
///
/// NOTE (Phase-1 TOCTOU): slug selection is check-then-insert; under concurrent
/// app creation two requests could pick the same candidate and the second will
/// hit the DB UNIQUE constraint as a 500. Acceptable for Phase 1.
async fn unique_app_slug(
    db: &dyn DatabaseStore,
    org_id: &str,
    base: &str,
) -> Result<String, sqlx::Error> {
    let existing: std::collections::HashSet<String> = db
        .list_apps_for_org(org_id)
        .await?
        .into_iter()
        .map(|a| a.slug)
        .collect();
    if !existing.contains(base) {
        return Ok(base.to_string());
    }
    for n in 2..10000 {
        let cand = format!("{base}-{n}");
        if !existing.contains(&cand) {
            return Ok(cand);
        }
    }
    Ok(format!("{base}-{}", uuid::Uuid::new_v4()))
}

// ─── Authorization helper ──────────────────────────────────────────────────────

/// Returns the caller's user id if they are a member of `org_id` with `perm`;
/// otherwise an error `Response` (403/500). Bearer API keys (no session user)
/// can never manage orgs.
async fn require_member_perm(
    state: &AppState,
    ctx: &KeyContext,
    org_id: &str,
    perm: Permission,
) -> Result<String, Response> {
    let user = ctx
        .user
        .as_ref()
        .ok_or_else(|| forbidden("session required"))?;
    match state.db.get_membership(org_id, &user.user_id).await {
        Ok(Some(role)) if role_has(role, perm) => Ok(user.user_id.clone()),
        Ok(Some(_)) => Err(forbidden("insufficient role for this organization")),
        Ok(None) => Err(forbidden("not a member of this organization")),
        Err(_) => Err(internal_error("membership lookup failed")),
    }
}

/// Resolve the app's owning org id, or an error Response (404/500).
async fn org_for_app(state: &AppState, app_id: &str) -> Result<(Application, String), Response> {
    match state.db.get_application(app_id).await {
        Ok(Some(app)) => {
            let org_id = app.org_id.clone();
            Ok((app, org_id))
        }
        Ok(None) => Err(err(StatusCode::NOT_FOUND, "app_not_found", "Application not found")),
        Err(_) => Err(internal_error("application lookup failed")),
    }
}

// ─── Views / request bodies ─────────────────────────────────────────────────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OrgView {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub plan: String,
    pub status: String,
}

impl From<Organization> for OrgView {
    fn from(o: Organization) -> Self {
        Self {
            id: o.id,
            name: o.name,
            slug: o.slug,
            plan: o.plan,
            status: o.status,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OrgSummary {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub role: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MemberView {
    pub org_id: String,
    pub user_id: String,
    pub email: String,
    pub role: String,
    pub created_at: String,
}

impl From<OrganizationMember> for MemberView {
    fn from(m: OrganizationMember) -> Self {
        Self {
            org_id: m.org_id,
            user_id: m.user_id,
            email: m.email,
            role: m.role.as_str().to_string(),
            created_at: m.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateOrgRequest {
    pub name: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateOrgRequest {
    pub name: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateAppRequest {
    pub name: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateAppRequest {
    pub name: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AddMemberRequest {
    pub email: String,
    pub role: Role,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateMemberRequest {
    pub role: Role,
}

/// Renamed in the OpenAPI schema to `OrgTransferOwnerRequest` to avoid a
/// component name collision with `users::TransferOwnerRequest`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[schema(as = OrgTransferOwnerRequest)]
pub struct TransferOwnerRequest {
    pub new_owner_user_id: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateProviderCredentialRequest {
    pub provider: String,
    /// The provider's secret fields, e.g. `{"api_key":"sk-..."}`.
    pub credentials: serde_json::Value,
}

// ─── POST /v1/orgs ───────────────────────────────────────────────────────────

/// POST /v1/orgs — Create an organization owned by the caller (any session user).
#[utoipa::path(
    post,
    path = "/v1/orgs",
    request_body = CreateOrgRequest,
    responses(
        (status = 200, description = "Organization created", body = OrgView),
        (status = 401, description = "Session required", body = crate::types::ErrorResponse),
        (status = 500, description = "DB error", body = crate::types::ErrorResponse),
    ),
    tag = "Organizations"
)]
pub async fn create_org(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Json(body): Json<CreateOrgRequest>,
) -> Response {
    let user = match ctx.user.as_ref() {
        Some(u) => u,
        None => return err(StatusCode::UNAUTHORIZED, "session_required", "Session required"),
    };

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "invalid_name", "Organization name is required");
    }

    let slug = match unique_org_slug(state.db.as_ref(), &slugify(&name)).await {
        Ok(s) => s,
        Err(e) => return internal_error(&e.to_string()),
    };
    let now = chrono::Utc::now();
    let org = Organization {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        slug,
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    // NOTE (Phase-1 accepted limitation): the org → member → app creation below
    // is a non-atomic sequence of DB calls. A mid-sequence failure (e.g. crash
    // after create_organization but before add_org_member) can orphan rows.
    // A transactional `create_tenant` DB helper is a Phase-2 follow-up.
    if let Err(e) = state.db.create_organization(&org).await {
        return internal_error(&e.to_string());
    }
    if let Err(e) = state.db.add_org_member(&org.id, &user.user_id, Role::Owner).await {
        return internal_error(&e.to_string());
    }
    // Default application for the new org.
    let app = Application {
        id: uuid::Uuid::new_v4().to_string(),
        org_id: org.id.clone(),
        name: "Default".into(),
        slug: "default".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    if let Err(e) = state.db.create_application(&app).await {
        return internal_error(&e.to_string());
    }

    (StatusCode::OK, Json(OrgView::from(org))).into_response()
}

// ─── GET /v1/orgs ──────────────────────────────────────────────────────────────

/// GET /v1/orgs — List the orgs the caller belongs to (any session user).
#[utoipa::path(
    get,
    path = "/v1/orgs",
    responses(
        (status = 200, description = "Organizations the caller belongs to", body = Vec<OrgSummary>),
        (status = 401, description = "Session required", body = crate::types::ErrorResponse),
    ),
    tag = "Organizations"
)]
pub async fn list_orgs(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
) -> Response {
    let user = match ctx.user.as_ref() {
        Some(u) => u,
        None => return err(StatusCode::UNAUTHORIZED, "session_required", "Session required"),
    };
    match state.db.list_orgs_for_user(&user.user_id).await {
        Ok(orgs) => {
            let out: Vec<OrgSummary> = orgs
                .into_iter()
                .map(|(o, role)| OrgSummary {
                    id: o.id,
                    name: o.name,
                    slug: o.slug,
                    role: role.as_str().to_string(),
                })
                .collect();
            (StatusCode::OK, Json(out)).into_response()
        }
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── GET /v1/orgs/{id} ───────────────────────────────────────────────────────

/// GET /v1/orgs/{id} — Get an organization (requires org:read membership).
#[utoipa::path(
    get,
    path = "/v1/orgs/{id}",
    params(("id" = String, Path, description = "Organization ID")),
    responses(
        (status = 200, description = "Organization", body = OrgView),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Not found", body = crate::types::ErrorResponse),
    ),
    tag = "Organizations"
)]
pub async fn get_org(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &id, Permission::OrgRead).await {
        return resp;
    }
    match state.db.get_organization(&id).await {
        Ok(Some(org)) => (StatusCode::OK, Json(OrgView::from(org))).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "org_not_found", "Organization not found"),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── PATCH /v1/orgs/{id} ─────────────────────────────────────────────────────

/// PATCH /v1/orgs/{id} — Rename an organization (requires org:write membership).
#[utoipa::path(
    patch,
    path = "/v1/orgs/{id}",
    params(("id" = String, Path, description = "Organization ID")),
    request_body = UpdateOrgRequest,
    responses(
        (status = 200, description = "Updated organization", body = OrgView),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Not found", body = crate::types::ErrorResponse),
    ),
    tag = "Organizations"
)]
pub async fn patch_org(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
    Json(body): Json<UpdateOrgRequest>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &id, Permission::OrgWrite).await {
        return resp;
    }
    let name = body.name.trim();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "invalid_name", "Organization name is required");
    }
    match state.db.update_organization(&id, Some(name)).await {
        Ok(Some(org)) => (StatusCode::OK, Json(OrgView::from(org))).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "org_not_found", "Organization not found"),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── DELETE /v1/orgs/{id} ────────────────────────────────────────────────────

/// DELETE /v1/orgs/{id} — Delete an organization (requires org:delete membership).
#[utoipa::path(
    delete,
    path = "/v1/orgs/{id}",
    params(("id" = String, Path, description = "Organization ID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Not found", body = crate::types::ErrorResponse),
    ),
    tag = "Organizations"
)]
pub async fn delete_org(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &id, Permission::OrgDelete).await {
        return resp;
    }
    match state.db.delete_organization(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "org_not_found", "Organization not found"),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── GET /v1/orgs/{id}/members ──────────────────────────────────────────────

/// GET /v1/orgs/{id}/members — List org members (requires member:read membership).
#[utoipa::path(
    get,
    path = "/v1/orgs/{id}/members",
    params(("id" = String, Path, description = "Organization ID")),
    responses(
        (status = 200, description = "Members", body = Vec<MemberView>),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Organizations"
)]
pub async fn list_members(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &id, Permission::MemberRead).await {
        return resp;
    }
    match state.db.list_org_members(&id).await {
        Ok(members) => {
            let out: Vec<MemberView> = members.into_iter().map(MemberView::from).collect();
            (StatusCode::OK, Json(out)).into_response()
        }
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── POST /v1/orgs/{id}/members ─────────────────────────────────────────────

/// POST /v1/orgs/{id}/members — Invite a member to this org (requires member:invite).
#[utoipa::path(
    post,
    path = "/v1/orgs/{id}/members",
    params(("id" = String, Path, description = "Organization ID")),
    request_body = AddMemberRequest,
    responses(
        (status = 200, description = "Invitation created"),
        (status = 400, description = "Cannot invite owner directly", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Organizations"
)]
pub async fn invite_member(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
    Json(body): Json<AddMemberRequest>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &id, Permission::MemberInvite).await {
        return resp;
    }
    if body.role == Role::Owner {
        return err(
            StatusCode::BAD_REQUEST,
            "cannot_invite_owner",
            "Cannot invite an Owner directly. Use transfer-owner.",
        );
    }

    let email = body.email.trim().to_lowercase();
    let token = generate_session_token();
    let inv = Invitation {
        id: uuid::Uuid::new_v4().to_string(),
        email: email.clone(),
        role: body.role,
        token: token.clone(),
        invited_by: ctx.user.as_ref().map(|u| u.user_id.clone()),
        org_id: id.clone(),
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        used_at: None,
        created_at: chrono::Utc::now(),
    };
    if let Err(e) = state.db.create_invitation(&inv).await {
        return internal_error(&e.to_string());
    }

    let mut payload = json!({
        "id": inv.id,
        "email": email,
        "role": inv.role.as_str(),
        "org_id": id,
        "expires_at": inv.expires_at.to_rfc3339(),
    });
    if state.dev.expose_invite_tokens {
        payload["_dev_token"] = json!(token);
    }
    (StatusCode::OK, Json(payload)).into_response()
}

// ─── PATCH /v1/orgs/{id}/members/{user_id} ──────────────────────────────────

/// PATCH /v1/orgs/{id}/members/{user_id} — Change a member's role (requires member:write).
#[utoipa::path(
    patch,
    path = "/v1/orgs/{id}/members/{user_id}",
    params(
        ("id" = String, Path, description = "Organization ID"),
        ("user_id" = String, Path, description = "Member user ID"),
    ),
    request_body = UpdateMemberRequest,
    responses(
        (status = 200, description = "Updated"),
        (status = 400, description = "Cannot change owner via this endpoint", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Member not found", body = crate::types::ErrorResponse),
    ),
    tag = "Organizations"
)]
pub async fn patch_member(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path((id, user_id)): Path<(String, String)>,
    Json(body): Json<UpdateMemberRequest>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &id, Permission::MemberWrite).await {
        return resp;
    }
    // Cannot promote-to or demote-from Owner here; use transfer-owner instead.
    if body.role == Role::Owner {
        return err(
            StatusCode::BAD_REQUEST,
            "cannot_set_owner",
            "Cannot assign the Owner role here. Use transfer-owner.",
        );
    }
    let current = match state.db.get_membership(&id, &user_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return err(StatusCode::NOT_FOUND, "member_not_found", "Member not found"),
        Err(e) => return internal_error(&e.to_string()),
    };
    if current == Role::Owner {
        return err(
            StatusCode::BAD_REQUEST,
            "cannot_demote_owner",
            "Cannot change the Owner's role. Use transfer-owner.",
        );
    }
    match state.db.update_member_role(&id, &user_id, body.role).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── DELETE /v1/orgs/{id}/members/{user_id} ─────────────────────────────────

/// DELETE /v1/orgs/{id}/members/{user_id} — Remove a member (requires member:remove).
#[utoipa::path(
    delete,
    path = "/v1/orgs/{id}/members/{user_id}",
    params(
        ("id" = String, Path, description = "Organization ID"),
        ("user_id" = String, Path, description = "Member user ID"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 400, description = "Cannot remove owner", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Member not found", body = crate::types::ErrorResponse),
    ),
    tag = "Organizations"
)]
pub async fn remove_member(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path((id, user_id)): Path<(String, String)>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &id, Permission::MemberRemove).await {
        return resp;
    }
    match state.db.get_membership(&id, &user_id).await {
        Ok(Some(Role::Owner)) => {
            return err(
                StatusCode::BAD_REQUEST,
                "cannot_remove_owner",
                "Cannot remove the Owner. Transfer ownership first.",
            );
        }
        Ok(Some(_)) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, "member_not_found", "Member not found"),
        Err(e) => return internal_error(&e.to_string()),
    }
    match state.db.remove_org_member(&id, &user_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── POST /v1/orgs/{id}/transfer-owner ──────────────────────────────────────

/// POST /v1/orgs/{id}/transfer-owner — Transfer ownership (requires org:transfer_owner).
#[utoipa::path(
    post,
    path = "/v1/orgs/{id}/transfer-owner",
    params(("id" = String, Path, description = "Organization ID")),
    request_body = TransferOwnerRequest,
    responses(
        (status = 204, description = "Ownership transferred"),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Target not a member", body = crate::types::ErrorResponse),
    ),
    tag = "Organizations"
)]
pub async fn transfer_owner(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
    Json(body): Json<TransferOwnerRequest>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &id, Permission::OrgTransferOwner).await {
        return resp;
    }
    match state.db.transfer_org_owner(&id, &body.new_owner_user_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        // RowNotFound means the target user is not a member of this org.
        Err(sqlx::Error::RowNotFound) => err(
            StatusCode::NOT_FOUND,
            "not_a_member",
            "Target user is not a member of this organization",
        ),
        // Any other DB error is an internal fault.
        Err(_) => internal_error("transfer ownership failed"),
    }
}

// ─── GET /v1/orgs/{id}/apps ─────────────────────────────────────────────────

/// GET /v1/orgs/{id}/apps — List applications in an org (requires app:read).
#[utoipa::path(
    get,
    path = "/v1/orgs/{id}/apps",
    params(("id" = String, Path, description = "Organization ID")),
    responses(
        (status = 200, description = "Applications", body = Vec<crate::types::Application>),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn list_apps(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &id, Permission::AppRead).await {
        return resp;
    }
    match state.db.list_apps_for_org(&id).await {
        Ok(apps) => (StatusCode::OK, Json(apps)).into_response(),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── POST /v1/orgs/{id}/apps ────────────────────────────────────────────────

/// POST /v1/orgs/{id}/apps — Create an application (requires app:write).
#[utoipa::path(
    post,
    path = "/v1/orgs/{id}/apps",
    params(("id" = String, Path, description = "Organization ID")),
    request_body = CreateAppRequest,
    responses(
        (status = 200, description = "Application created", body = crate::types::Application),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn create_app(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
    Json(body): Json<CreateAppRequest>,
) -> Response {
    if let Err(resp) = require_member_perm(&state, &ctx, &id, Permission::AppWrite).await {
        return resp;
    }
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "invalid_name", "Application name is required");
    }
    let slug = match unique_app_slug(state.db.as_ref(), &id, &slugify(&name)).await {
        Ok(s) => s,
        Err(e) => return internal_error(&e.to_string()),
    };
    let now = chrono::Utc::now();
    let app = Application {
        id: uuid::Uuid::new_v4().to_string(),
        org_id: id.clone(),
        name,
        slug,
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    if let Err(e) = state.db.create_application(&app).await {
        return internal_error(&e.to_string());
    }
    (StatusCode::OK, Json(app)).into_response()
}

// ─── GET /v1/apps/{app_id} ───────────────────────────────────────────────────

/// GET /v1/apps/{app_id} — Get an application; authorizes via its org (app:read).
#[utoipa::path(
    get,
    path = "/v1/apps/{app_id}",
    params(("app_id" = String, Path, description = "Application ID")),
    responses(
        (status = 200, description = "Application", body = crate::types::Application),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Not found", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn get_app(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
) -> Response {
    let (app, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::AppRead).await {
        return resp;
    }
    (StatusCode::OK, Json(app)).into_response()
}

// ─── PATCH /v1/apps/{app_id} ─────────────────────────────────────────────────

/// PATCH /v1/apps/{app_id} — Rename an application (app:write via its org).
#[utoipa::path(
    patch,
    path = "/v1/apps/{app_id}",
    params(("app_id" = String, Path, description = "Application ID")),
    request_body = UpdateAppRequest,
    responses(
        (status = 200, description = "Updated application", body = crate::types::Application),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Not found", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn patch_app(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
    Json(body): Json<UpdateAppRequest>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::AppWrite).await {
        return resp;
    }
    let name = body.name.trim();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "invalid_name", "Application name is required");
    }
    match state.db.update_application(&app_id, Some(name)).await {
        Ok(Some(app)) => (StatusCode::OK, Json(app)).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "app_not_found", "Application not found"),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── DELETE /v1/apps/{app_id} ────────────────────────────────────────────────

/// DELETE /v1/apps/{app_id} — Delete an application (app:delete via its org).
#[utoipa::path(
    delete,
    path = "/v1/apps/{app_id}",
    params(("app_id" = String, Path, description = "Application ID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Not found", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn delete_app(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::AppDelete).await {
        return resp;
    }
    match state.db.delete_application(&app_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "app_not_found", "Application not found"),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── GET /v1/apps/{app_id}/provider-credentials ─────────────────────────────

/// GET /v1/apps/{app_id}/provider-credentials — List BYO credentials (provider_cred:read).
/// Never returns plaintext — only `ProviderCredentialInfo`.
#[utoipa::path(
    get,
    path = "/v1/apps/{app_id}/provider-credentials",
    params(("app_id" = String, Path, description = "Application ID")),
    responses(
        (status = 200, description = "Stored credentials (no plaintext)", body = Vec<crate::types::ProviderCredentialInfo>),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn list_provider_credentials(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::ProviderCredRead).await
    {
        return resp;
    }
    match state.db.list_provider_credentials(&app_id).await {
        Ok(creds) => (StatusCode::OK, Json(creds)).into_response(),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── POST /v1/apps/{app_id}/provider-credentials ────────────────────────────

/// POST /v1/apps/{app_id}/provider-credentials — Store a BYO credential (provider_cred:write).
/// Encrypts the credential JSON; returns `ProviderCredentialInfo` (no plaintext).
#[utoipa::path(
    post,
    path = "/v1/apps/{app_id}/provider-credentials",
    params(("app_id" = String, Path, description = "Application ID")),
    request_body = CreateProviderCredentialRequest,
    responses(
        (status = 200, description = "Credential stored (no plaintext)", body = crate::types::ProviderCredentialInfo),
        (status = 400, description = "Secrets key not configured", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn create_provider_credential(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
    Json(body): Json<CreateProviderCredentialRequest>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) =
        require_member_perm(&state, &ctx, &org_id, Permission::ProviderCredWrite).await
    {
        return resp;
    }

    let secrets_key = match state.secrets_key {
        Some(k) => k,
        None => {
            return err(
                StatusCode::BAD_REQUEST,
                "secrets_not_configured",
                "Provider credentials require a configured secrets key",
            );
        }
    };

    let provider = body.provider.trim().to_string();
    if provider.is_empty() {
        return err(StatusCode::BAD_REQUEST, "invalid_provider", "provider is required");
    }

    // Derive a safe, non-secret display hint that also reflects pool size.
    let display_hint = derive_display_hint(&body.credentials);

    let plaintext = match serde_json::to_vec(&body.credentials) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, "invalid_credentials", &e.to_string()),
    };
    let (ciphertext, nonce) = match crate::auth::secrets::encrypt(&secrets_key, &plaintext) {
        Ok(v) => v,
        Err(e) => return internal_error(&e),
    };

    if let Err(e) = state
        .db
        .upsert_provider_credential(&app_id, &provider, &ciphertext, &nonce, display_hint.as_deref())
        .await
    {
        return internal_error(&e.to_string());
    }

    let info = ProviderCredentialInfo {
        provider,
        display_hint,
        created_at: chrono::Utc::now(),
    };
    (StatusCode::OK, Json(info)).into_response()
}

/// Derive a safe, non-secret display hint from a credential blob. Shows the last
/// four characters of the first key and, for pools, how many more there are —
/// e.g. `…1234`, `…1234 (+2 more)`. Handles bearer (`api_key`/`api_keys`) and
/// signing (`key_id`/`credential_sets`) shapes. Returns `None` when nothing
/// suitable is present.
fn derive_display_hint(creds: &serde_json::Value) -> Option<String> {
    let last4 = |s: &str| (s.len() >= 4).then(|| format!("…{}", &s[s.len() - 4..]));
    let with_count = |hint: String, n: usize| {
        if n > 1 { format!("{hint} (+{} more)", n - 1) } else { hint }
    };
    let first_str = |arr: &[serde_json::Value], field: &str| -> Option<String> {
        arr.iter()
            .filter_map(|e| e.get(field).and_then(|v| v.as_str()))
            .find(|s| !s.is_empty())
            .map(str::to_string)
    };

    // Weighted pools first (these are what the dashboard submits).
    if let Some(arr) = creds.get("api_keys").and_then(|v| v.as_array()) {
        if let Some(first) = first_str(arr, "key") {
            return last4(&first).map(|h| with_count(h, arr.len()));
        }
    }
    if let Some(arr) = creds.get("credential_sets").and_then(|v| v.as_array()) {
        if let Some(first) = first_str(arr, "key_id") {
            return last4(&first).map(|h| with_count(h, arr.len()));
        }
    }
    // Single-credential fallbacks.
    creds
        .get("api_key")
        .or_else(|| creds.get("key_id"))
        .and_then(|v| v.as_str())
        .and_then(last4)
}

// ─── DELETE /v1/apps/{app_id}/provider-credentials/{provider} ───────────────

/// DELETE /v1/apps/{app_id}/provider-credentials/{provider} — Delete a credential (provider_cred:delete).
#[utoipa::path(
    delete,
    path = "/v1/apps/{app_id}/provider-credentials/{provider}",
    params(
        ("app_id" = String, Path, description = "Application ID"),
        ("provider" = String, Path, description = "Provider name"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Not found", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn delete_provider_credential(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path((app_id, provider)): Path<(String, String)>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) =
        require_member_perm(&state, &ctx, &org_id, Permission::ProviderCredDelete).await
    {
        return resp;
    }
    match state.db.delete_provider_credential(&app_id, &provider).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "credential_not_found", "Credential not found"),
        Err(e) => internal_error(&e.to_string()),
    }
}

// ─── Per-app BYO storage config ──────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PutAppStorageRequest {
    #[serde(default)]
    pub backend: Option<String>,
    pub bucket_name: String,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub endpoint_url: Option<String>,
    #[serde(default)]
    pub custom_public_url: Option<String>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    /// Write-only. Provide WITH `secret_access_key` to set/rotate; omit BOTH to keep existing.
    #[serde(default)]
    pub access_key_id: Option<String>,
    /// Write-only.
    #[serde(default)]
    pub secret_access_key: Option<String>,
}

fn app_storage_info_from_row(row: crate::types::AppStorageRow) -> crate::types::AppStorageInfo {
    crate::types::AppStorageInfo {
        configured: true,
        backend: Some(row.backend),
        bucket_name: Some(row.bucket_name),
        region: Some(row.region),
        endpoint_url: row.endpoint_url,
        custom_public_url: row.custom_public_url,
        path_prefix: row.path_prefix,
        access_key_id_hint: row.access_key_id_hint,
        updated_at: Some(row.updated_at),
    }
}

/// GET /v1/apps/{app_id}/storage — read BYO storage config (storage_cred:read). No secret.
#[utoipa::path(
    get,
    path = "/v1/apps/{app_id}/storage",
    params(("app_id" = String, Path, description = "Application ID")),
    responses(
        (status = 200, description = "Storage config (no secret)", body = crate::types::AppStorageInfo),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn get_app_storage(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::StorageCredRead).await {
        return resp;
    }
    match state.db.get_app_storage(&app_id).await {
        Ok(Some(row)) => (StatusCode::OK, Json(app_storage_info_from_row(row))).into_response(),
        Ok(None) => (
            StatusCode::OK,
            Json(crate::types::AppStorageInfo {
                configured: false,
                backend: None,
                bucket_name: None,
                region: None,
                endpoint_url: None,
                custom_public_url: None,
                path_prefix: None,
                access_key_id_hint: None,
                updated_at: None,
            }),
        )
            .into_response(),
        Err(e) => internal_error(&e.to_string()),
    }
}

/// PUT /v1/apps/{app_id}/storage — upsert BYO storage config (storage_cred:write).
#[utoipa::path(
    put,
    path = "/v1/apps/{app_id}/storage",
    params(("app_id" = String, Path, description = "Application ID")),
    request_body = PutAppStorageRequest,
    responses(
        (status = 200, description = "Stored (no secret)", body = crate::types::AppStorageInfo),
        (status = 400, description = "Bad request / secrets key unavailable", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn put_app_storage(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
    Json(body): Json<PutAppStorageRequest>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::StorageCredWrite).await {
        return resp;
    }

    let secrets_key = match state.secrets_key {
        Some(k) => k,
        None => {
            return err(
                StatusCode::BAD_REQUEST,
                "secrets_not_configured",
                "Storage credentials require a configured secrets key",
            );
        }
    };

    let bucket_name = body.bucket_name.trim().to_string();
    if bucket_name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "invalid_bucket", "bucket_name is required");
    }
    let backend = body
        .backend
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("s3")
        .to_string();
    let region = body
        .region
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("us-east-1")
        .to_string();

    let ak = body.access_key_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let sk = body.secret_access_key.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (secret_ciphertext, secret_nonce, access_key_id_hint) = match (ak, sk) {
        (Some(ak), Some(sk)) => {
            let plaintext = match serde_json::to_vec(
                &serde_json::json!({ "access_key_id": ak, "secret_access_key": sk }),
            ) {
                Ok(v) => v,
                Err(e) => return internal_error(&e.to_string()),
            };
            let (ct, nonce) = match crate::auth::secrets::encrypt(&secrets_key, &plaintext) {
                Ok(v) => v,
                Err(e) => return internal_error(&e),
            };
            let hint = if ak.len() >= 4 {
                Some(format!("…{}", &ak[ak.len() - 4..]))
            } else {
                None
            };
            (ct, nonce, hint)
        }
        (None, None) => match state.db.get_app_storage(&app_id).await {
            Ok(Some(existing)) => {
                (existing.secret_ciphertext, existing.secret_nonce, existing.access_key_id_hint)
            }
            Ok(None) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    "credentials_required",
                    "access_key_id and secret_access_key are required for a new storage config",
                );
            }
            Err(e) => return internal_error(&e.to_string()),
        },
        _ => {
            return err(
                StatusCode::BAD_REQUEST,
                "credentials_incomplete",
                "provide both access_key_id and secret_access_key, or neither",
            );
        }
    };

    let input = crate::types::AppStorageUpsert {
        app_id: app_id.clone(),
        backend,
        bucket_name,
        region,
        endpoint_url: body.endpoint_url.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
        custom_public_url: body.custom_public_url.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
        path_prefix: body.path_prefix.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
        access_key_id_hint,
        secret_ciphertext,
        secret_nonce,
    };
    if let Err(e) = state.db.upsert_app_storage(&input).await {
        return internal_error(&e.to_string());
    }

    match state.db.get_app_storage(&app_id).await {
        Ok(Some(row)) => (StatusCode::OK, Json(app_storage_info_from_row(row))).into_response(),
        Ok(None) => internal_error("storage config vanished after write"),
        Err(e) => internal_error(&e.to_string()),
    }
}

/// DELETE /v1/apps/{app_id}/storage — remove BYO storage config (storage_cred:delete).
#[utoipa::path(
    delete,
    path = "/v1/apps/{app_id}/storage",
    params(("app_id" = String, Path, description = "Application ID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "Not found", body = crate::types::ErrorResponse),
    ),
    tag = "Applications"
)]
pub async fn delete_app_storage(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(app_id): Path<String>,
) -> Response {
    let (_, org_id) = match org_for_app(&state, &app_id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_member_perm(&state, &ctx, &org_id, Permission::StorageCredDelete).await
    {
        return resp;
    }
    match state.db.delete_app_storage(&app_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "storage_not_found", "Storage config not found"),
        Err(e) => internal_error(&e.to_string()),
    }
}

#[cfg(test)]
mod tests;
