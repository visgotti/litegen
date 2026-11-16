use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

use crate::api::middleware::{is_platform_admin, person_holds, AppState, KeyContext, DEFAULT_ORG_ID};
use crate::auth::permissions::Permission;
use crate::auth::tokens::generate_session_token;
use crate::config::Mode;
use crate::types::{Invitation, Role, User};

// ─── Helpers ────────────────────────────────────────────────────────────────

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

/// A 500 that keeps its class (`error_code`: `db_error`, `hash_error`, ...)
/// but not its text (SEC-DBERR): the detail goes to the server log, the
/// caller gets "internal server error". A database error names tables and
/// constraints.
fn internal(error_code: &str, detail: &dyn std::fmt::Display) -> Response {
    tracing::error!(error = %detail, code = error_code, "internal error");
    err(StatusCode::INTERNAL_SERVER_ERROR, error_code, "internal server error")
}

fn forbidden_perm(perm: &str) -> Response {
    err(
        StatusCode::FORBIDDEN,
        "forbidden_permission",
        &format!("Permission '{}' required", perm),
    )
}

/// Authorize the global, NON-org-scoped user-management endpoints
/// (`GET /v1/users`, `PATCH`/`DELETE /v1/users/{id}`,
/// `POST /v1/users/transfer-owner`). These operate on the platform-wide
/// `users` table.
///
/// In hosted (multi-tenant) mode every tenant is the Owner of its own org and
/// therefore carries `User*Any`/`SystemTransferOwner` permissions. If these
/// endpoints only checked `ctx.permissions` (as they previously did), any
/// signed-up tenant could read, modify, deactivate, or transfer ownership of
/// users belonging to *other* tenants. So in hosted mode they are restricted
/// to the platform admin (master key); tenants manage their own members via
/// `/v1/orgs/{id}/members`.
///
/// In single-tenant mode the deployment's owner and admins manage users
/// directly, gated by the permission of their role in the default org; a
/// role in any other org carries none of it. The master key does too, as the
/// deployment's superuser (README), though it holds no role.
fn authorize_global_user_admin(
    state: &AppState,
    ctx: &KeyContext,
    perm: Permission,
) -> Result<(), Response> {
    if may_manage_users(state, ctx, perm) {
        return Ok(());
    }
    match state.mode {
        Mode::Hosted => Err(forbidden_perm("platform_admin")),
        Mode::SingleTenant => Err(forbidden_perm(perm.as_str())),
    }
}

/// Whether [`authorize_global_user_admin`] lets `ctx` through with `perm`;
/// the MCP tools for these endpoints are shown by the same rule. The master
/// key passes in either mode. Each `perm` is a deployment permission, which a
/// person holds only from the default org (`person_holds`, what
/// `GET /v1/auth/me` serves).
pub(crate) fn may_manage_users(state: &AppState, ctx: &KeyContext, perm: Permission) -> bool {
    match state.mode {
        Mode::Hosted => is_platform_admin(ctx),
        Mode::SingleTenant => is_platform_admin(ctx) || person_holds(state.mode, ctx, perm),
    }
}

/// Public user view returned from user management endpoints.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PublicUser {
    pub id: String,
    pub email: String,
    pub role: String,
    pub is_active: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

impl From<User> for PublicUser {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            email: u.email,
            role: u.role.as_str().to_string(),
            is_active: u.is_active,
            created_at: u.created_at.to_rfc3339(),
            last_login_at: u.last_login_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// Spawn a fire-and-forget audit log insert, under the org the caller acts
/// in. The hosted master key acts in none: its changes to accounts are filed
/// under no org (`api::audit::audit_org`).
fn log_audit_user(
    state: &AppState,
    ctx: &KeyContext,
    action: &str,
    target_type: &str,
    target_id: &str,
    after: Option<serde_json::Value>,
) {
    let db = state.db.clone();
    let actor_key_id = ctx.key_id.map(|id| id.to_string());
    let actor_label = ctx
        .user
        .as_ref()
        .map(|u| u.email.clone())
        .or_else(|| ctx.key_id.map(|id| id.to_string()))
        .unwrap_or_else(|| "master-key".to_string());
    let action = action.to_string();
    let target_type = target_type.to_string();
    let target_id = target_id.to_string();
    let org_id = crate::api::audit::audit_org(state.mode, ctx.org_id.clone());
    tokio::spawn(async move {
        let entry = crate::types::AuditLogEntry {
            id: format!("audit-{}", Uuid::new_v4()),
            actor_key_id,
            actor_label,
            action,
            target_type,
            target_id,
            before_json: None,
            after_json: after.map(|v| v.to_string()),
            created_at: chrono::Utc::now(),
            org_id,
        };
        let _ = db.insert_audit_log(&entry).await;
    });
}

/// An audit entry about the account `target_id`, filed under each org it
/// belongs to, whose admins answer for it (as `audit_org` files an entry
/// about an org's object), or, when it belongs to none, as
/// [`log_audit_user`] files it (review A3).
async fn log_audit_about_account(state: &AppState, ctx: &KeyContext, action: &str, target_id: &str, after: Option<serde_json::Value>) {
    match state.db.list_orgs_for_user(target_id).await {
        Ok(orgs) if !orgs.is_empty() => {
            for (org, _) in orgs {
                crate::api::handlers::management::audit(state, ctx, Some(&org.id), action, "user", target_id, after.clone());
            }
        }
        _ => log_audit_user(state, ctx, action, "user", target_id, after),
    }
}

// ─── GET /v1/users ────────────────────────────────────────────────────────────

/// GET /v1/users — List all users (requires user:read:any).
#[utoipa::path(
    get,
    path = "/v1/users",
    responses(
        (status = 200, description = "List of users", body = Vec<PublicUser>),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Users"
)]
pub async fn list_users(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
) -> Response {
    if let Err(resp) = authorize_global_user_admin(&state, &ctx, Permission::UserReadAny) {
        return resp;
    }
    match state.db.list_users().await {
        Ok(users) => {
            let public: Vec<PublicUser> = users.into_iter().map(PublicUser::from).collect();
            (StatusCode::OK, Json(public)).into_response()
        }
        Err(e) => internal("db_error", &e),
    }
}

// ─── POST /v1/users ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InviteRequest {
    pub email: String,
    pub role: Role,
}

/// A new invitation. Its `token` and `invite_url` are shown only in this
/// answer: litegen sends no email, so give one of them to the invitee.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InviteResponse {
    pub id: String,
    pub email: String,
    pub role: String,
    /// The organization the invitee joins on accepting.
    pub org_id: String,
    pub expires_at: String,
    /// Accepts the invitation once, until `expires_at`
    /// (`POST /v1/auth/invitations/{token}/accept`): whoever holds it joins
    /// the org as `role`, so give it only to the invitee.
    pub token: String,
    /// The dashboard's page that accepts it: `/invite/{token}` on the origin
    /// of `server.public_base_url`; null when it isn't set: build it from
    /// `token` on the dashboard's origin.
    #[schema(required = true)]
    pub invite_url: Option<String>,
}

/// The dashboard's page `path` (`/invite/{token}`, `/reset-password/{token}`)
/// on the origin of `server.public_base_url`, as the MCP consent page is.
/// `None` while that isn't set: litegen never guesses its public host.
fn dashboard_url(state: &AppState, path: &str) -> Option<String> {
    let base = state.router.config.server.public_base_url.as_deref().map(str::trim).filter(|s| !s.is_empty())?;
    let origin = reqwest::Url::parse(base).ok()?.origin();
    origin.is_tuple().then(|| format!("{}{path}", origin.ascii_serialization()))
}

/// The dashboard's page that accepts the invitation `token`.
fn invite_url(state: &AppState, token: &str) -> Option<String> {
    dashboard_url(state, &format!("/invite/{token}"))
}

/// The answer to a new invitation (`POST /v1/users`,
/// `POST /v1/orgs/{id}/members`): [`InviteResponse`], the only one that
/// shows its token. The old dev flag (`LITEGEN__DEV__EXPOSE_INVITE_TOKENS`)
/// still adds `_dev_token`, the same value, for clients that read it.
pub(crate) fn invitation_created(state: &AppState, inv: &Invitation) -> Response {
    let made = InviteResponse {
        id: inv.id.clone(),
        email: inv.email.clone(),
        role: inv.role.as_str().to_string(),
        org_id: inv.org_id.clone(),
        expires_at: inv.expires_at.to_rfc3339(),
        token: inv.token.clone(),
        invite_url: invite_url(state, &inv.token),
    };
    let mut body = match serde_json::to_value(made) {
        Ok(body) => body,
        Err(e) => return internal("serialize_error", &e),
    };
    if state.dev.expose_invite_tokens {
        body["_dev_token"] = json!(inv.token);
    }
    (StatusCode::OK, Json(body)).into_response()
}

/// POST /v1/users — Invite a user (requires invitation:send).
#[utoipa::path(
    post,
    path = "/v1/users",
    request_body = InviteRequest,
    responses(
        (status = 200, description = "Invitation created", body = InviteResponse),
        (status = 400, description = "Bad request (`cannot_invite_owner`, `invalid_email`)", body = crate::types::ErrorResponse),
        (status = 401, description = "Not authenticated, or `sign_in_ended`: your sign-in ended while this request ran (a reset, a deactivation or another password change committed), and no invitation was stored.", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden: the invited role must be below your own in the organization (an Admin invites Members and Viewers, an Owner Admins too).", body = crate::types::ErrorResponse),
    ),
    tag = "Users"
)]
pub async fn invite_user(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Json(body): Json<InviteRequest>,
) -> Response {
    // The invitation is into the org the caller acts in, so the invitee joins
    // it on accept. With none, a single-tenant caller acts for the deployment,
    // whose org is the default one; in hosted mode the default org is the
    // operator's, never a fallback, and acting in no org there is nowhere to
    // invite to (FU-NOORG).
    let org_id = match (ctx.org_id.clone(), state.mode) {
        (Some(org_id), _) => org_id,
        (None, Mode::SingleTenant) => DEFAULT_ORG_ID.to_string(),
        (None, Mode::Hosted) => return crate::api::middleware::no_active_org_response(),
    };
    // A person by their role where they act; the master key, which holds
    // none, as the superuser (single-tenant: in the default org).
    if !(is_platform_admin(&ctx) || ctx.permissions.contains(&Permission::InvitationSend)) {
        return forbidden_perm("invitation:send");
    }
    if body.role == Role::Owner {
        return err(
            StatusCode::BAD_REQUEST,
            "cannot_invite_owner",
            "Cannot invite an Owner directly. Use transfer-owner.",
        );
    }
    // The invited role must be below the inviter's own there, as a role
    // change needs (review round 3b, S4): removing a Member and inviting them
    // back as an Admin promoted past the rule. The master key, any but Owner.
    match crate::api::handlers::orgs::outranks_member(&state, &ctx, &org_id, body.role).await {
        Ok(true) => {}
        Ok(false) => return err(StatusCode::FORBIDDEN, "forbidden_permission", "You may invite only to a role below your own"),
        Err(resp) => return resp,
    }

    let email = match crate::auth::email::account_email(&body.email) {
        Ok(email) => email,
        Err(why) => return err(StatusCode::BAD_REQUEST, "invalid_email", why),
    };
    let inv = Invitation {
        id: Uuid::new_v4().to_string(),
        email: email.clone(),
        role: body.role,
        token: generate_session_token(),
        invited_by: ctx.user.as_ref().map(|u| u.user_id.clone()),
        org_id,
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        used_at: None,
        created_at: chrono::Utc::now(),
    };

    // Stored only while the sender's sign-in is live (review round 6,
    // finding 2): a reset or a deactivation that committed while this ran
    // leaves nothing behind it.
    let Some(from) = crate::api::middleware::sign_in_of(&ctx) else {
        return crate::api::handlers::account::sign_in_ended();
    };
    match state.db.create_invitation_from(&inv, from).await {
        Ok(true) => {}
        Ok(false) => return crate::api::handlers::account::sign_in_ended(),
        Err(e) => return internal("db_error", &e),
    }

    log_audit_user(
        &state,
        &ctx,
        "invitation.create",
        "invitation",
        &inv.id,
        Some(json!({ "email": email, "role": inv.role.as_str() })),
    );

    invitation_created(&state, &inv)
}

// ─── GET /v1/auth/invitations/{token} ────────────────────────────────────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InvitationView {
    pub email: String,
    pub role: String,
    pub expires_at: String,
}

/// GET /v1/auth/invitations/{token} — Look up an invitation (no auth required).
#[utoipa::path(
    get,
    path = "/v1/auth/invitations/{token}",
    params(("token" = String, Path, description = "Invitation token")),
    responses(
        (status = 200, description = "Invitation details", body = InvitationView),
        (status = 400, description = "Expired or already used", body = crate::types::ErrorResponse),
        (status = 404, description = "Not found", body = crate::types::ErrorResponse),
    ),
    tag = "Users"
)]
pub async fn get_invitation(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Response {
    match state.db.get_invitation(&token).await {
        Ok(Some(inv)) => {
            if inv.used_at.is_some() || inv.expires_at < chrono::Utc::now() {
                return err(
                    StatusCode::BAD_REQUEST,
                    "invitation_expired",
                    "Invitation has expired or was already used",
                );
            }
            (
                StatusCode::OK,
                Json(json!({
                    "email": inv.email,
                    "role": inv.role.as_str(),
                    "expires_at": inv.expires_at.to_rfc3339(),
                })),
            )
                .into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, "invitation_not_found", "Invitation not found"),
        Err(e) => internal("db_error", &e),
    }
}

// ─── POST /v1/auth/invitations/{token}/accept ─────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AcceptInvitationRequest {
    pub password: Option<String>,
}

/// POST /v1/auth/invitations/{token}/accept — Accept an invitation and create an account.
#[utoipa::path(
    post,
    path = "/v1/auth/invitations/{token}/accept",
    params(("token" = String, Path, description = "Invitation token")),
    request_body = AcceptInvitationRequest,
    responses(
        (status = 200, description = "Account created and session set"),
        (status = 400, description = "Invalid, expired or already used; `invitation_revoked` when whoever sent it may no longer invite to its role (demoted, removed or deactivated since it was sent: ask for a new invitation); or (an invitation made before addresses were checked) `invalid_email`", body = crate::types::ErrorResponse),
        (status = 404, description = "Invitation not found", body = crate::types::ErrorResponse),
    ),
    tag = "Users"
)]
pub async fn accept_invitation(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    Json(body): Json<AcceptInvitationRequest>,
) -> Response {
    // Validate password is present (OAuth-flow accept is a future follow-up)
    let password = match body.password {
        Some(p) => p,
        None => {
            return err(
                StatusCode::BAD_REQUEST,
                "password_or_oauth_required",
                "A password is required to accept this invitation",
            );
        }
    };

    // Fetch and validate invitation
    let inv = match state.db.get_invitation(&token).await {
        Ok(Some(i)) => i,
        Ok(None) => return err(StatusCode::NOT_FOUND, "invitation_not_found", "Invitation not found"),
        Err(e) => return internal("db_error", &e),
    };

    if inv.used_at.is_some() {
        return err(
            StatusCode::BAD_REQUEST,
            "invitation_already_used",
            "This invitation has already been used",
        );
    }
    if inv.expires_at < chrono::Utc::now() {
        return err(
            StatusCode::BAD_REQUEST,
            "invitation_expired",
            "This invitation has expired",
        );
    }
    // Whoever sent it may still send it (review round 7, finding 6).
    match crate::api::handlers::orgs::sender_may_still_invite(state.db.as_ref(), &inv).await {
        Ok(true) => {}
        Ok(false) => return err(StatusCode::BAD_REQUEST, "invitation_revoked", crate::api::handlers::orgs::INVITATION_SENDER_REVOKED),
        Err(e) => return internal("db_error", &e),
    }

    // The account to join with: the existing one (its password proven), which
    // simply gains the membership, or a new one, made below in the same
    // transaction as the acceptance. Only a new one pays for hashing.
    let (user, is_new, begun) = match state.db.get_user_by_email(&inv.email).await {
        Ok(Some(existing)) => {
            // Its sign-in epoch, from the row whose hash is checked below
            // (review round 5, finding 2).
            let begun = existing.sign_in_epoch;
            // SECURITY: a valid invitation token is NOT proof of control over a
            // pre-existing account. The invited email is chosen freely by an org
            // admin, so without authenticating the caller as the existing user we
            // would let anyone holding the token mint a session AS that account
            // (account takeover / privilege escalation). Require the existing
            // account's password — the same check `login` performs.
            match existing.password_hash.as_deref() {
                Some(hash) => {
                    // Off the runtime's threads, as login checks it (sweep of 2026-11-16).
                    if !crate::auth::password::verify_password_async(password.clone(), hash.to_string()).await.unwrap_or(false) {
                        // Do NOT consume the invitation, create a session, or add
                        // membership on a failed authentication.
                        return err(
                            StatusCode::UNAUTHORIZED,
                            "invalid_credentials",
                            "Incorrect password for the existing account associated with this invitation",
                        );
                    }
                }
                None => {
                    // OAuth-only account (no password set): the password path
                    // cannot authenticate them. They must accept while signed in
                    // via their OAuth identity, not with a password.
                    return err(
                        StatusCode::UNAUTHORIZED,
                        "password_login_unavailable",
                        "This account has no password; sign in with your OAuth provider to accept this invitation",
                    );
                }
            }
            // Its sign-in epoch was read before the password check (round 4,
            // finding 2): a reset that committed meanwhile refuses the session.
            (existing, false, begun)
        }
        Ok(None) => {
            // The account is made under the invited email, which an
            // invitation from before addresses were checked may not be.
            if let Err(why) = crate::auth::email::account_email(&inv.email) {
                return err(StatusCode::BAD_REQUEST, "invalid_email", why);
            }
            // Hash password only when a new account must be created.
            let hash = match crate::auth::password::hash_password_async(password.clone()).await {
                Ok(h) => h,
                Err(crate::auth::password::PasswordError::TooShort) => {
                    return err(
                        StatusCode::BAD_REQUEST,
                        "password_too_short",
                        &crate::auth::password::too_short_message(),
                    );
                }
                Err(e) => return internal("hash_error", &e),
            };
            let user = User {
                id: Uuid::new_v4().to_string(),
                email: inv.email.clone(),
                password_hash: Some(hash),
                role: inv.role,
                oauth_github_id: None,
                oauth_google_id: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                last_login_at: None,
                is_active: true,
                sign_in_epoch: 0,
            };
            (user, true, 0)
        }
        Err(e) => return internal("db_error", &e),
    };

    // One transaction (review A6): the invitation consumed while it is
    // neither accepted nor revoked, with the new account and the membership.
    // An acceptance or revoke that came first leaves nothing written here; a
    // failure part way leaves the invitation unused, to try again.
    match state.db.accept_invitation(&token, is_new.then_some(&user), &user.id, &inv.org_id, inv.role).await {
        Ok(true) => {}
        Ok(false) => {
            return err(
                StatusCode::BAD_REQUEST,
                "invitation_already_used",
                "This invitation was used or revoked meanwhile",
            )
        }
        Err(e) => return internal("db_error", &e),
    }

    // Create session
    let (_, _, session_cookie, csrf_cookie) =
        match crate::api::middleware::create_session_cookies_at(&state.db, &user.id, None, None, begun).await {
            Ok(v) => v,
            Err(e) => return internal("session_error", &e),
        };

    let public = PublicUser::from(user);
    let mut resp = (StatusCode::OK, Json(json!({ "user": public }))).into_response();
    resp.headers_mut().append("set-cookie", session_cookie);
    resp.headers_mut().append("set-cookie", csrf_cookie);
    resp
}

// ─── PATCH /v1/users/{id} ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PatchUserRequest {
    pub role: Option<Role>,
    pub is_active: Option<bool>,
}

/// Whether `target` is the deployment's Owner, for this page's PATCH and
/// DELETE. In single-tenant mode the Owner is recorded twice: in
/// `users.role`, and in the default org's membership, where permissions come
/// from. Transfers keep the two together (`transfer_deployment_owner`). A pair
/// left apart by a transfer before that counts as Owner wherever either
/// place says so, so this page never demotes, deactivates or deletes the
/// account that holds the Owner's permissions (`split_owner_records` reports
/// such a pair at startup).
async fn is_owner(state: &AppState, target: &User) -> Result<bool, Response> {
    // Hosted: ownership lives in each org's membership, and every self-signup
    // holds the Owner account role, so no account is the deployment's Owner
    // (review round 4, finding 1): the master key may deactivate any of them.
    if state.mode != Mode::SingleTenant {
        return Ok(false);
    }
    if target.role == Role::Owner {
        return Ok(true);
    }
    match state.db.get_membership(DEFAULT_ORG_ID, &target.id).await {
        Ok(role) => Ok(role == Some(Role::Owner)),
        Err(e) => Err(internal("db_error", &e)),
    }
}

/// Single-tenant: the accounts each Owner record names, `(by membership, by
/// account role)`, when the two don't agree on one Owner. Before
/// `transfer_deployment_owner`, each transfer moved only one of them, so a
/// deployment that used one may hold its Owner apart. The membership is the
/// one with the permissions. A single transfer by the membership Owner, or the
/// master key, to the intended account (`POST /v1/orgs/{default}/transfer-owner`)
/// repairs both. Two empty lists mean accounts exist but neither record names
/// an Owner. `None` while there's no account at all (before the first
/// signup).
pub async fn split_owner_records(db: &dyn crate::db::DatabaseStore) -> Result<Option<(Vec<String>, Vec<String>)>, sqlx::Error> {
    let users = db.list_users().await?;
    if users.is_empty() {
        return Ok(None);
    }
    let mut by_membership: Vec<String> =
        db.list_org_members(DEFAULT_ORG_ID).await?.into_iter().filter(|m| m.role == Role::Owner).map(|m| m.user_id).collect();
    let mut by_account: Vec<String> = users.into_iter().filter(|u| u.role == Role::Owner).map(|u| u.id).collect();
    by_membership.sort();
    by_account.sort();
    let agree = by_membership.len() == 1 && by_membership == by_account;
    Ok((!agree).then_some((by_membership, by_account)))
}

/// PATCH /v1/users/{id} — Update user role or active status (requires user:write:any).
#[utoipa::path(
    patch,
    path = "/v1/users/{id}",
    params(("id" = String, Path, description = "User ID")),
    request_body = PatchUserRequest,
    responses(
        (status = 200, description = "Updated user", body = PublicUser),
        (status = 400, description = "Bad request", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden (a role change needs a role above the user's own, and never grants one at or above yours; deactivating or reactivating needs a role above the user's own, so never your own)", body = crate::types::ErrorResponse),
        (status = 404, description = "User not found", body = crate::types::ErrorResponse),
    ),
    tag = "Users"
)]
pub async fn patch_user(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
    Json(body): Json<PatchUserRequest>,
) -> Response {
    if let Err(resp) = authorize_global_user_admin(&state, &ctx, Permission::UserWriteAny) {
        return resp;
    }

    // Load target user
    let target = match state.db.get_user_by_id(&id).await {
        Ok(Some(u)) => u,
        Ok(None) => return err(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => return internal("db_error", &e),
    };

    // Ownership moves only through transfer-owner, as invitations and org
    // members have it: never by PATCH, whoever the target is. (A former
    // Owner whose account role a Members-page transfer left behind could
    // otherwise write Owner back into their membership: two Owners.)
    if body.role == Some(Role::Owner) {
        return err(
            StatusCode::BAD_REQUEST,
            "cannot_assign_owner",
            "Cannot assign the Owner role via PATCH. Use transfer-owner.",
        );
    }

    // Block changing Owner's role via this endpoint
    let target_is_owner = match is_owner(&state, &target).await {
        Ok(owner) => owner,
        Err(resp) => return resp,
    };
    if target_is_owner {
        if body.role.is_some() {
            return err(
                StatusCode::BAD_REQUEST,
                "cannot_change_owner_role",
                "Cannot change the Owner's role via PATCH. Use transfer-owner.",
            );
        }
        if body.is_active == Some(false) {
            return err(
                StatusCode::BAD_REQUEST,
                "cannot_deactivate_owner",
                "Cannot deactivate the deployment's Owner. Transfer ownership first (POST /v1/orgs/{default org}/transfer-owner, or the Users page).",
            );
        }
    }

    // Every check comes before the first write, so a refusal changes
    // nothing. Single-tenant: the account's role in the default org is the
    // one its permissions come from (a role set here is set there too), and
    // the one a person must outrank to change or deactivate it.
    let membership = match deployment_role(&state, &id).await {
        Ok(membership) => membership,
        Err(resp) => return resp,
    };
    // Deactivating or reactivating (round 3b, S1: a peer Admin could undo
    // the Owner's deactivation of an Admin).
    if body.is_active.is_some() {
        if let Err(resp) = may_change_active(&state, &ctx, membership).await {
            return resp;
        }
    }
    if let (Some(role), Mode::SingleTenant) = (body.role, state.mode) {
        // An account with no role in the default org takes the rule with its
        // own account role as the current one (round 3b, S4): its `users.role`
        // is what `/v1/auth/me`, `/v1/users` and hooks report.
        let current = membership.unwrap_or(target.role);
        // The org's Owner keeps theirs, as on the members page.
        if membership == Some(Role::Owner) {
            return err(
                StatusCode::BAD_REQUEST,
                "cannot_change_owner_role",
                "Cannot change the Owner's role via PATCH. Use transfer-owner.",
            );
        }
        // Review A2: a person moves only a role below their own, to one
        // below their own (the master key, any).
        match crate::api::handlers::orgs::may_change_role(&state, &ctx, DEFAULT_ORG_ID, current, role).await {
            Ok(true) => {}
            Ok(false) => {
                return err(
                    StatusCode::FORBIDDEN,
                    "forbidden_permission",
                    "A role change needs a role above the user's own, and never grants one at or above yours",
                );
            }
            Err(resp) => return resp,
        }
    }

    let mut previous_role = None;
    if let (Some(role), Some(current)) = (body.role, membership) {
        previous_role = Some(current);
        // Moved to a role that can't invite: the invitations they sent in
        // the org go first (SEC-INVITES).
        if crate::api::handlers::orgs::role_may_invite(current) && !crate::api::handlers::orgs::role_may_invite(role) {
            if let Err(resp) = crate::api::handlers::orgs::revoke_invitations(&state, &ctx, Some(DEFAULT_ORG_ID), None, Some(&id), "inviter_role_changed").await {
                return resp;
            }
        }
    }
    // Every role change is audited with the old role (review A2): the
    // default org's above, else the account's own (hosted mode).
    if body.role.is_some() && previous_role.is_none() {
        previous_role = Some(target.role);
    }

    // One transaction (review round 7, finding 3): the account's row, its
    // role in the deployment's org (single-tenant, where the permissions come
    // from), and a deactivation's revoke of the invitations the account sent,
    // in every org (SEC-INVITES, review round 6, finding 2). A failure (a
    // 500) changes none of it, so the PATCH can be sent again whole; only a
    // role change's revoke above has run, and it is audited.
    let member_org = membership.is_some().then_some(DEFAULT_ORG_ID);
    let (updated, revoked) = match state.db.update_user_with_revoked(&id, body.role, body.is_active, None, member_org).await {
        Ok(Some(done)) => done,
        Ok(None) => return err(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => return internal("db_error", &e),
    };
    audit_sent_invitations_revoked(&state, &ctx, &revoked).await;
    log_audit_user(
        &state,
        &ctx,
        "user.update",
        "user",
        &id,
        Some(match previous_role {
            Some(before) => json!({ "role": updated.role.as_str(), "previous_role": before.as_str(), "is_active": updated.is_active }),
            None => json!({ "role": updated.role.as_str(), "is_active": updated.is_active }),
        }),
    );
    (StatusCode::OK, Json(PublicUser::from(updated))).into_response()
}

// ─── DELETE /v1/users/{id} ────────────────────────────────────────────────────

/// DELETE /v1/users/{id} — Soft-delete (deactivate) a user (requires user:delete:any).
#[utoipa::path(
    delete,
    path = "/v1/users/{id}",
    params(("id" = String, Path, description = "User ID")),
    responses(
        (status = 204, description = "User deactivated: in the same step its sessions end, its apps' OAuth sign-ins and unredeemed codes are revoked, its unused reset links and the ones it issued are deleted, and the pending invitations it sent, in every organization, are revoked (each in the audit log)."),
        (status = 400, description = "Cannot delete owner", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden (deactivating needs a role above the user's own, so never your own)", body = crate::types::ErrorResponse),
        (status = 404, description = "User not found", body = crate::types::ErrorResponse),
    ),
    tag = "Users"
)]
pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = authorize_global_user_admin(&state, &ctx, Permission::UserDeleteAny) {
        return resp;
    }

    // Load target user
    let target = match state.db.get_user_by_id(&id).await {
        Ok(Some(u)) => u,
        Ok(None) => return err(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => return internal("db_error", &e),
    };

    match is_owner(&state, &target).await {
        Ok(true) => {
            return err(
                StatusCode::BAD_REQUEST,
                "cannot_delete_owner",
                "Cannot delete the deployment's Owner. Transfer ownership first (POST /v1/orgs/{default org}/transfer-owner, or the Users page).",
            );
        }
        Ok(false) => {}
        Err(resp) => return resp,
    }
    let membership = match deployment_role(&state, &id).await {
        Ok(membership) => membership,
        Err(resp) => return resp,
    };
    if let Err(resp) = may_change_active(&state, &ctx, membership).await {
        return resp;
    }
    // The invitations they sent, in every org, go with the account
    // (SEC-INVITES), in the deactivation's transaction (review round 6,
    // finding 2): a deactivated Admin's invitation would let them back in.
    match state.db.update_user_with_revoked(&id, None, Some(false), None, None).await {
        Ok(deactivated) => {
            if let Some((_, revoked)) = deactivated {
                audit_sent_invitations_revoked(&state, &ctx, &revoked).await;
            }
            log_audit_user(
                &state,
                &ctx,
                "user.delete",
                "user",
                &id,
                None,
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => internal("db_error", &e),
    }
}

// ─── POST /v1/users/transfer-owner ───────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TransferOwnerRequest {
    pub new_owner_id: String,
}

/// POST /v1/users/transfer-owner — Transfer ownership to another user (Owner only).
#[utoipa::path(
    post,
    path = "/v1/users/transfer-owner",
    request_body = TransferOwnerRequest,
    responses(
        (status = 204, description = "Ownership transferred"),
        (status = 400, description = "`transfer_owner_per_org`: in hosted mode each organization has its own Owner; transfer one with `POST /v1/orgs/{id}/transfer-owner`.", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden (not owner)", body = crate::types::ErrorResponse),
        (status = 404, description = "Target user not found", body = crate::types::ErrorResponse),
    ),
    tag = "Users"
)]
pub async fn transfer_owner(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Json(body): Json<TransferOwnerRequest>,
) -> Response {
    if let Err(resp) = authorize_global_user_admin(&state, &ctx, Permission::SystemTransferOwner) {
        return resp;
    }
    // Hosted: the deployment-level transfer would rewrite the account role of
    // every tenant's Owner (`UPDATE users SET role = 'admin' WHERE role =
    // 'owner'`), and delete their unused reset links with it (migration
    // 042). Each org has its own Owner there (review round 4, finding 1).
    if state.mode == Mode::Hosted {
        return err(
            StatusCode::BAD_REQUEST,
            "transfer_owner_per_org",
            "In hosted mode each organization has its own Owner: transfer one with POST /v1/orgs/{id}/transfer-owner",
        );
    }

    // Verify target exists and is active
    match state.db.get_user_by_id(&body.new_owner_id).await {
        Ok(Some(u)) if !u.is_active => {
            return err(
                StatusCode::BAD_REQUEST,
                "user_inactive",
                "Cannot transfer ownership to an inactive user",
            );
        }
        Ok(None) => {
            return err(StatusCode::NOT_FOUND, "user_not_found", "Target user not found");
        }
        Err(e) => return internal("db_error", &e),
        Ok(Some(_)) => {}
    }

    // Single-tenant: the Owner is the account role and the default org's
    // membership (where permissions come from); both move together, and only
    // to a member of that org.
    let moved = match state.mode {
        Mode::SingleTenant => state.db.transfer_deployment_owner(DEFAULT_ORG_ID, &body.new_owner_id).await,
        Mode::Hosted => state.db.transfer_owner(&body.new_owner_id).await,
    };
    match moved {
        Ok(()) => {
            log_audit_user(
                &state,
                &ctx,
                "system.transfer_owner",
                "user",
                &body.new_owner_id,
                None,
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(sqlx::Error::RowNotFound) => err(
            StatusCode::NOT_FOUND,
            "not_a_member",
            "The new Owner must be a member of the default organization",
        ),
        Err(e) => internal("db_error", &e),
    }
}

// ─── POST /v1/users/{id}/password-reset ──────────────────────────────────────

/// A password reset link issued for an account. Shown only in this answer:
/// litegen sends no email, so give `reset_url` or `token` to the account's
/// owner.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PasswordResetLink {
    pub user_id: String,
    pub email: String,
    /// Sets the account's password once, until `expires_at` (1 hour), with
    /// `POST /v1/auth/password-reset/confirm`. Whoever holds it can take over
    /// the account, so give it only to its owner.
    pub token: String,
    /// The dashboard's page that sets the password: `/reset-password/{token}`
    /// on the origin of `server.public_base_url`; null when it isn't set:
    /// build it from `token` on the dashboard's origin.
    #[schema(required = true)]
    pub reset_url: Option<String>,
    pub expires_at: String,
}

/// Audit the pending invitations a deactivation revoked (SEC-INVITES), each
/// under its org (review round 5, finding 3; round 6, finding 6).
async fn audit_sent_invitations_revoked(state: &AppState, ctx: &KeyContext, revoked: &[Invitation]) {
    crate::api::handlers::orgs::audit_revoked_invitations(state, crate::api::handlers::orgs::Revoker::Principal(ctx), revoked, Some("inviter_deactivated")).await;
}

/// The account's role in the deployment's org: in single-tenant mode the one
/// its permissions come from, and the one a person must outrank to change or
/// deactivate it. Hosted mode has none (only the master key reaches the
/// routes that ask).
async fn deployment_role(state: &AppState, id: &str) -> Result<Option<Role>, Response> {
    match state.mode {
        Mode::SingleTenant => state.db.get_membership(DEFAULT_ORG_ID, id).await.map_err(|e| internal("db_error", &e)),
        Mode::Hosted => Ok(None),
    }
}

/// The outrank rule (review A2) for deactivating (PATCH `is_active: false`,
/// DELETE) and reactivating (PATCH `is_active: true`, round 3b S1):
/// deactivating someone and inviting them back lower is a demotion by
/// another door, so a person acts only on an account whose role in the
/// deployment's org (`theirs`) is below their own, never their own. One with
/// no role there holds nothing. The master key passes.
async fn may_change_active(state: &AppState, ctx: &KeyContext, theirs: Option<Role>) -> Result<(), Response> {
    let Some(theirs) = theirs else { return Ok(()) };
    match crate::api::handlers::orgs::outranks_member(state, ctx, DEFAULT_ORG_ID, theirs).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(err(StatusCode::FORBIDDEN, "forbidden_permission", "Deactivating or reactivating an account needs a role above the user's own")),
        Err(resp) => Err(resp),
    }
}

/// How long an issued reset link works: as long as a self-service one.
const RESET_LINK_TTL_HOURS: i64 = 1;

/// Whether `ctx` may issue a reset link for `target`. The link takes the
/// whole account over, so: the master key, any account; anyone else, in
/// either mode, only an account they strictly outrank in every org it
/// belongs to (holding `member:write` there), each such org within an OAuth
/// sign-in's grant. So no one reaches a peer, a better, the deployment's
/// Owner, or an account that also belongs to an org they don't run (a hosted
/// signup's own org). Their own account goes through the password change
/// (review A1).
async fn may_issue_reset_link(state: &AppState, ctx: &KeyContext, target: &User) -> Result<bool, Response> {
    if is_platform_admin(ctx) {
        return Ok(true);
    }
    let Some(caller) = ctx.user.as_ref() else { return Ok(false) };
    let orgs = state.db.list_orgs_for_user(&target.id).await.map_err(|e| internal("db_error", &e))?;
    if orgs.is_empty() {
        return Ok(false);
    }
    for (org, theirs) in orgs {
        if !crate::api::middleware::within_grant(ctx, &org.id) {
            return Ok(false);
        }
        match state.db.get_membership(&org.id, &caller.user_id).await.map_err(|e| internal("db_error", &e))? {
            Some(mine) if crate::auth::permissions::role_has(mine, Permission::MemberWrite) && crate::auth::permissions::outranks(mine, theirs) => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// POST /v1/users/{id}/password-reset — Issue a password reset link for an account (the master key, or a person who outranks the account in every org it belongs to).
#[utoipa::path(
    post,
    path = "/v1/users/{id}/password-reset",
    params(("id" = String, Path, description = "User ID")),
    responses(
        (status = 200, description = "The link, shown only in this answer", body = PasswordResetLink),
        (status = 401, description = "Not authenticated, or `sign_in_ended`: your sign-in ended while this request ran (a reset, a deactivation or another password change committed), and no link was stored.", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden: only the master key, or a person who outranks the account in every organization it belongs to; never for your own account (change it with `PATCH /v1/account`, which asks for the current password); for a deactivated account, only one who may reactivate it, since confirming the link reactivates it.", body = crate::types::ErrorResponse),
        (status = 404, description = "`user_not_found`", body = crate::types::ErrorResponse),
    ),
    tag = "Users"
)]
pub async fn issue_password_reset(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
) -> Response {
    // Only the master key and signed-in people can hold the right; a key
    // never does.
    if !is_platform_admin(&ctx) && ctx.user.is_none() {
        return forbidden_perm("user:write:any");
    }
    let target = match state.db.get_user_by_id(&id).await {
        Ok(Some(u)) => u,
        Ok(None) => return err(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => return internal("db_error", &e),
    };
    if !is_platform_admin(&ctx) && ctx.user.as_ref().is_some_and(|u| u.user_id == target.id) {
        return err(
            StatusCode::FORBIDDEN,
            "forbidden_permission",
            "Not for your own account: change your password with PATCH /v1/account, which asks for the current one",
        );
    }
    match may_issue_reset_link(&state, &ctx, &target).await {
        Ok(true) => {}
        Ok(false) => {
            return err(
                StatusCode::FORBIDDEN,
                "forbidden_permission",
                "A reset link takes the account over: only the master key, or a person who outranks the account in every organization it belongs to, may issue one",
            );
        }
        Err(resp) => return resp,
    }
    // Confirming a link reactivates a deactivated account (round 3b, S1), so
    // only one who may reactivate it issues one: the master key, or in
    // single-tenant mode a person with `user:write:any` who outranks it in
    // the deployment's org. Its deactivation deleted the earlier links.
    if !target.is_active {
        let may_reactivate = match deployment_role(&state, &target.id).await {
            Ok(theirs) => may_manage_users(&state, &ctx, Permission::UserWriteAny) && may_change_active(&state, &ctx, theirs).await.is_ok(),
            Err(resp) => return resp,
        };
        if !may_reactivate {
            return err(
                StatusCode::FORBIDDEN,
                "forbidden_permission",
                "The account is deactivated: only someone who may reactivate it may issue it a reset link",
            );
        }
    }
    let now = chrono::Utc::now();
    // The issuer, re-checked when the link is confirmed (review round 4,
    // findings 4 and 5); the master key's links carry none.
    let reset = crate::types::PasswordReset {
        token: generate_session_token(),
        user_id: target.id.clone(),
        expires_at: now + chrono::Duration::hours(RESET_LINK_TTL_HOURS),
        used_at: None,
        created_at: now,
        issued_by: if is_platform_admin(&ctx) { None } else { ctx.user.as_ref().map(|u| u.user_id.clone()) },
    };
    // Stored only while the issuer's sign-in is live (review round 6,
    // finding 2): a reset or a deactivation of the issuer that committed
    // while this ran leaves no link behind it.
    let Some(from) = crate::api::middleware::sign_in_of(&ctx) else {
        return crate::api::handlers::account::sign_in_ended();
    };
    match state.db.create_password_reset_from(&reset, from).await {
        Ok(true) => {}
        Ok(false) => return crate::api::handlers::account::sign_in_ended(),
        Err(e) => return internal("db_error", &e),
    }
    log_audit_about_account(&state, &ctx, "user.password_reset_link", &target.id, Some(json!({ "email": target.email }))).await;
    let link = PasswordResetLink {
        user_id: target.id,
        email: target.email,
        reset_url: dashboard_url(&state, &format!("/reset-password/{}", reset.token)),
        token: reset.token,
        expires_at: reset.expires_at.to_rfc3339(),
    };
    (StatusCode::OK, Json(link)).into_response()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::{get, patch, post},
        Router,
    };
    use tower::ServiceExt;

    use crate::api::middleware::{auth_middleware, AppState};
    use crate::auth::tokens::{generate_csrf_token, generate_session_token};
    use crate::capabilities::CapabilityRegistry;
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::db::sqlite::SqliteDatabase;
    use crate::db::DatabaseStore;
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::materializer::{Materializer, MaterializeError, TempStorage};
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::LocalStore;
    use crate::types::{Role, Session, User};
    use bytes::Bytes;

    struct NoopStorage;
    #[async_trait::async_trait]
    impl TempStorage for NoopStorage {
        async fn put(&self, key: &str, _bytes: Bytes, _ct: &str) -> Result<String, MaterializeError> {
            Ok(format!("local://{}", key))
        }
        async fn delete(&self, _key: &str) -> Result<(), MaterializeError> {
            Ok(())
        }
    }

    async fn build_db() -> Arc<SqliteDatabase> {
        Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"))
    }

    async fn build_state(db: Arc<SqliteDatabase>) -> Arc<AppState> {
        let registry = Arc::new(ProviderRegistry::new());
        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = Arc::new(ProxyRouter::new(registry, cache, config, image_store));
        let materializer = Arc::new(Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("models"));
        Arc::new(AppState {
            router,
            db,
            master_key: Some("master".to_string()),
            registry: cap_registry,
            materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    async fn seed_user_with_session(db: &Arc<SqliteDatabase>, role: Role) -> (User, String, String) {
        let user = User {
            id: format!("u-{}", Uuid::new_v4()),
            email: format!("{}@test.com", role.as_str()),
            password_hash: None,
            role,
            oauth_github_id: None,
            oauth_google_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: true,
            sign_in_epoch: 0,
        };
        db.create_user(&user).await.expect("create user");
        // Every account joins the default org at signup or invitation; the role
        // it holds there is the one its permissions come from.
        db.add_org_member(crate::api::middleware::DEFAULT_ORG_ID, &user.id, role).await.expect("join the default org");

        let session_token = generate_session_token();
        let csrf_token = generate_csrf_token();
        let sess = Session {
            id: session_token.clone(),
            user_id: user.id.clone(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            ip: None,
            user_agent: None,
            csrf_token: csrf_token.clone(),
        };
        db.create_session(&sess).await.expect("create session");
        (user, session_token, csrf_token)
    }

    fn build_users_router(state: Arc<AppState>) -> Router {
        let auth_state = state.clone();
        // Auth-protected routes
        let authed = Router::new()
            .route("/v1/users", get(list_users).post(invite_user))
            .route("/v1/users/transfer-owner", post(transfer_owner))
            .route("/v1/users/{id}", patch(patch_user).delete(delete_user))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                let s = auth_state.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state.clone());

        // Unauthenticated routes
        let unauthed = Router::new()
            .route("/v1/auth/invitations/{token}", get(get_invitation))
            .route("/v1/auth/invitations/{token}/accept", post(accept_invitation))
            .with_state(state);

        Router::new().merge(authed).merge(unauthed)
    }

    /// SEC-DBERR: the invitation routes need no sign-in, so a database error
    /// there must not reach the caller: no sqlx text, no table names.
    #[tokio::test]
    async fn a_database_error_reading_an_invitation_is_not_shown() {
        let db = build_db().await;
        sqlx::query("ALTER TABLE invitations RENAME TO invitations_away").execute(db.pool()).await.unwrap();
        let app = build_users_router(build_state(db).await);
        let resp = app
            .oneshot(Request::builder().uri("/v1/auth/invitations/any-token").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&body).to_string();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "db_error", "{text}");
        assert!(!text.contains("no such table") && !text.contains("invitations"), "{text}");
    }

    // ── GET /v1/users ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_users_requires_user_read_any() {
        let db = build_db().await;
        let (_, viewer_sess, _) = seed_user_with_session(&db, Role::Viewer).await;
        let app = build_users_router(build_state(db).await);

        let req = Request::builder()
            .uri("/v1/users")
            .header("cookie", format!("litegen_session={}", viewer_sess))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn member_gets_403_listing_users() {
        let db = build_db().await;
        let (_, member_sess, _) = seed_user_with_session(&db, Role::Member).await;
        let app = build_users_router(build_state(db).await);

        let req = Request::builder()
            .uri("/v1/users")
            .header("cookie", format!("litegen_session={}", member_sess))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn admin_can_list_all_users() {
        let db = build_db().await;
        let (_, admin_sess, _) = seed_user_with_session(&db, Role::Admin).await;
        let app = build_users_router(build_state(db).await);

        let req = Request::builder()
            .uri("/v1/users")
            .header("cookie", format!("litegen_session={}", admin_sess))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 1);
    }

    // ── Hosted mode: global user endpoints are platform-admin (master key) only ──

    async fn build_state_hosted(db: Arc<SqliteDatabase>) -> Arc<AppState> {
        let mut state = build_state(db).await;
        Arc::get_mut(&mut state).expect("unique Arc").mode = crate::config::Mode::Hosted;
        state
    }

    #[tokio::test]
    async fn hosted_tenant_owner_cannot_list_global_users() {
        // Regression for the cross-tenant BOLA: in hosted mode every signup is a
        // global Role::Owner, so a permission-only check let any tenant dump the
        // platform-wide users table. A tenant session must now be rejected.
        let db = build_db().await;
        let (_, owner_sess, _) = seed_user_with_session(&db, Role::Owner).await;
        let app = build_users_router(build_state_hosted(db).await);

        let req = Request::builder()
            .uri("/v1/users")
            .header("cookie", format!("litegen_session={}", owner_sess))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn hosted_tenant_owner_cannot_transfer_global_owner() {
        let db = build_db().await;
        let (_, owner_sess, csrf) = seed_user_with_session(&db, Role::Owner).await;
        let (victim, _, _) = seed_user_with_session(&db, Role::Admin).await;
        let app = build_users_router(build_state_hosted(db).await);

        let body = serde_json::to_vec(&json!({ "new_owner_id": victim.id })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/users/transfer-owner")
            .header("cookie", format!("litegen_session={}", owner_sess))
            .header("x-csrf-token", csrf)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn hosted_master_key_can_list_users() {
        // The platform admin (master key) is still allowed to manage the global
        // users table in hosted mode.
        let db = build_db().await;
        let _ = seed_user_with_session(&db, Role::Owner).await;
        let app = build_users_router(build_state_hosted(db).await);

        let req = Request::builder()
            .uri("/v1/users")
            .header("authorization", "Bearer master")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// README: the master key is a superuser for every endpoint. In
    /// single-tenant mode it acts in the default org but holds no role
    /// there, and every `/v1/users` route refused it; it manages the users
    /// as the deployment's Owner does.
    #[tokio::test]
    async fn single_tenant_the_master_key_manages_the_users() {
        let db = build_db().await;
        let (owner, _, _) = seed_user_with_session(&db, Role::Owner).await;
        let (admin, _, _) = seed_user_with_session(&db, Role::Admin).await;
        let (member, _, _) = seed_user_with_session(&db, Role::Member).await;
        let app = build_users_router(build_state(db.clone()).await);
        let master = |method: &str, uri: String, body: Option<serde_json::Value>| {
            let req = Request::builder().method(method).uri(uri).header("authorization", "Bearer master");
            match body {
                Some(v) => req.header("content-type", "application/json").body(Body::from(serde_json::to_vec(&v).unwrap())).unwrap(),
                None => req.body(Body::empty()).unwrap(),
            }
        };

        let resp = app.clone().oneshot(master("GET", "/v1/users".into(), None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "list the users");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let listed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(listed.as_array().map(Vec::len), Some(3), "{listed}");

        let resp = app.clone().oneshot(master("PATCH", format!("/v1/users/{}", member.id), Some(json!({ "role": "viewer" })))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "change a role");
        assert_eq!(db.get_membership(DEFAULT_ORG_ID, &member.id).await.unwrap(), Some(Role::Viewer));

        let resp = app.clone().oneshot(master("POST", "/v1/users".into(), Some(json!({ "email": "new@example.com", "role": "member" })))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "invite");
        let invited = db.list_invitations().await.unwrap();
        assert_eq!(invited.iter().map(|i| (i.email.as_str(), i.org_id.as_str())).collect::<Vec<_>>(), [("new@example.com", DEFAULT_ORG_ID)]);

        let resp = app.clone().oneshot(master("DELETE", format!("/v1/users/{}", member.id), None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT, "deactivate");
        assert!(!db.get_user_by_id(&member.id).await.unwrap().unwrap().is_active);

        let resp = app.clone().oneshot(master("POST", "/v1/users/transfer-owner".into(), Some(json!({ "new_owner_id": admin.id })))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT, "hand the deployment over");
        assert_eq!(db.get_membership(DEFAULT_ORG_ID, &admin.id).await.unwrap(), Some(Role::Owner));
        assert_eq!(db.get_user_by_id(&owner.id).await.unwrap().unwrap().role, Role::Admin);
    }

    // ── POST /v1/users (invite) ───────────────────────────────────────────────

    #[tokio::test]
    async fn invite_creates_invitation_in_db() {
        let db = build_db().await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let app = build_users_router(build_state(db.clone()).await);

        let body = serde_json::to_vec(&json!({ "email": "new@example.com", "role": "member" })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/users")
            .header("cookie", format!("litegen_session={}", admin_sess))
            .header("x-csrf-token", admin_csrf)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify invitation was created
        let invs = db.list_invitations().await.unwrap();
        assert_eq!(invs.len(), 1);
        assert_eq!(invs[0].email, "new@example.com");
    }

    #[tokio::test]
    async fn invite_with_owner_role_returns_400() {
        let db = build_db().await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let app = build_users_router(build_state(db).await);

        let body = serde_json::to_vec(&json!({ "email": "bad@example.com", "role": "owner" })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/users")
            .header("cookie", format!("litegen_session={}", admin_sess))
            .header("x-csrf-token", admin_csrf)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// The account an invitation makes is labelled by its email in the audit
    /// log, where the master key's label is `master-key`: an invitation takes
    /// only an address.
    #[tokio::test]
    async fn an_invitation_to_what_is_not_an_address_is_refused() {
        let db = build_db().await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let app = build_users_router(build_state(db.clone()).await);
        for email in ["master-key", "owner@localhost"] {
            let body = serde_json::to_vec(&json!({ "email": email, "role": "member" })).unwrap();
            let req = Request::builder()
                .method("POST")
                .uri("/v1/users")
                .header("cookie", format!("litegen_session={}", admin_sess))
                .header("x-csrf-token", &admin_csrf)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{email}");
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(json["error"]["code"], "invalid_email", "{email}: {json}");
        }
        assert!(db.list_invitations().await.unwrap().is_empty(), "no invitation stored");
    }

    /// An invitation made before addresses were checked makes no account
    /// under what isn't one: accepting it is refused, and it stays unused.
    #[tokio::test]
    async fn an_old_invitation_to_what_is_not_an_address_makes_no_account() {
        let db = build_db().await;
        let inv = make_invitation("master-key", "old-token", Role::Admin, crate::api::middleware::DEFAULT_ORG_ID);
        db.create_invitation(&inv).await.unwrap();
        let app = build_users_router(build_state(db.clone()).await);

        let body = serde_json::to_vec(&json!({ "password": "a-long-enough-password" })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/invitations/old-token/accept")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(resp.headers().get("set-cookie").is_none(), "no session");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], "invalid_email", "{json}");
        assert!(db.get_user_by_email("master-key").await.unwrap().is_none(), "no account made");
        assert!(db.get_invitation("old-token").await.unwrap().unwrap().used_at.is_none(), "the invitation stays unused");
    }

    /// [`build_state`] for a deployment whose public URL is `public_base_url`.
    async fn build_state_behind(db: Arc<SqliteDatabase>, public_base_url: &str) -> Arc<AppState> {
        let mut config = AppConfig::default();
        config.server.public_base_url = Some(public_base_url.to_string());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let router = ProxyRouter::new(Arc::new(ProviderRegistry::new()), cache, Arc::new(config), Arc::new(LocalStore));
        let mut state = build_state(db).await;
        Arc::get_mut(&mut state).expect("unique Arc").router = Arc::new(router);
        state
    }

    /// litegen sends no email, so the inviter gets what the invitee needs in
    /// the answer, without a dev flag: the token, which accepts the
    /// invitation, and the dashboard's link for it where the deployment's
    /// public URL is set (never a guessed host).
    #[tokio::test]
    async fn an_invitation_answers_with_its_token_and_link() {
        let db = build_db().await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        for (base, link_origin) in [(Some("https://app.example.com/api"), Some("https://app.example.com")), (None, None)] {
            let state = match base {
                Some(base) => build_state_behind(db.clone(), base).await,
                None => build_state(db.clone()).await,
            };
            let app = build_users_router(state);
            let email = format!("invitee-{}@example.com", Uuid::new_v4().simple());
            let body = serde_json::to_vec(&json!({ "email": email, "role": "member" })).unwrap();
            let req = Request::builder()
                .method("POST")
                .uri("/v1/users")
                .header("cookie", format!("litegen_session={}", admin_sess))
                .header("x-csrf-token", &admin_csrf)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{base:?}");
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let token = json["token"].as_str().unwrap_or_else(|| panic!("{base:?}: a token: {json}"));
            let stored = db.get_invitation(token).await.unwrap().expect("the token is the invitation's");
            assert_eq!(stored.email, email);
            assert_eq!(json["org_id"], DEFAULT_ORG_ID, "{json}");
            let link = link_origin.map(|origin| format!("{origin}/invite/{token}"));
            assert!(json.get("invite_url").is_some(), "present, null when unknown: {json}");
            assert_eq!(json["invite_url"], json!(link), "{base:?}: {json}");
            assert!(json.get("_dev_token").is_none(), "{json}");
        }
    }

    // ── POST /v1/users/{id}/password-reset (follow-up item 1) ────────────────

    /// An account and a session for it, in no org (a hosted invitee, before
    /// any membership is added).
    async fn seed_account(db: &Arc<SqliteDatabase>, email: &str) -> (User, String, String) {
        let user = User {
            id: format!("u-{}", Uuid::new_v4()),
            email: email.to_string(),
            password_hash: None,
            role: Role::Member,
            oauth_github_id: None,
            oauth_google_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: true,
            sign_in_epoch: 0,
        };
        db.create_user(&user).await.expect("create user");
        let (session_token, csrf_token) = (generate_session_token(), generate_csrf_token());
        db.create_session(&Session {
            id: session_token.clone(),
            user_id: user.id.clone(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            ip: None,
            user_agent: None,
            csrf_token: csrf_token.clone(),
        })
        .await
        .expect("create session");
        (user, session_token, csrf_token)
    }

    /// An org named `name`: its id.
    async fn seed_org(db: &Arc<SqliteDatabase>, name: &str) -> String {
        let now = chrono::Utc::now();
        let org = crate::types::Organization {
            id: format!("org-{}", Uuid::new_v4()),
            name: name.to_string(),
            slug: format!("{}-{}", name.to_lowercase(), Uuid::new_v4().simple()),
            plan: "free".into(),
            status: "active".into(),
            created_at: now,
            updated_at: now,
        };
        db.create_organization(&org).await.expect("create org");
        org.id
    }

    /// `POST /v1/users/{target}/password-reset` with the master key, or as
    /// the session `(token, csrf)`.
    fn reset_link_req(target: &str, session: Option<(&str, &str)>) -> Request<Body> {
        let req = Request::builder().method("POST").uri(format!("/v1/users/{target}/password-reset"));
        let req = match session {
            Some((token, csrf)) => req.header("cookie", format!("litegen_session={token}")).header("x-csrf-token", csrf),
            None => req.header("authorization", "Bearer master"),
        };
        req.body(Body::empty()).unwrap()
    }

    async fn json_of(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    /// litegen sends no email, so a person locked out of their account gets
    /// back in through a link someone who may manage the account issues: the
    /// master key, in either mode. The token sets the password once, within
    /// the hour; the link is the dashboard's page where the public URL is set.
    #[tokio::test]
    async fn the_master_key_issues_a_reset_link_that_sets_the_password_once() {
        for (mode, behind) in [("single-tenant", None), ("hosted", None), ("single-tenant behind a proxy", Some("https://app.example.com/api"))] {
            let db = build_db().await;
            let (member, _, _) = seed_user_with_session(&db, Role::Member).await;
            let state = match (mode, behind) {
                (_, Some(base)) => build_state_behind(db.clone(), base).await,
                ("hosted", None) => build_state_hosted(db.clone()).await,
                _ => build_state(db.clone()).await,
            };
            let app = crate::api::handlers::create_router(state);
            let resp = app.clone().oneshot(reset_link_req(&member.id, None)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{mode}");
            let link = json_of(resp).await;
            assert_eq!((link["user_id"].as_str(), link["email"].as_str()), (Some(member.id.as_str()), Some("member@test.com")), "{mode}: {link}");
            let token = link["token"].as_str().unwrap_or_else(|| panic!("{mode}: a token: {link}")).to_string();
            let url = behind.map(|_| format!("https://app.example.com/reset-password/{token}"));
            assert!(link.get("reset_url").is_some(), "present, null when unknown: {link}");
            assert_eq!(link["reset_url"], json!(url), "{mode}: {link}");
            let expires = chrono::DateTime::parse_from_rfc3339(link["expires_at"].as_str().expect("expires_at")).unwrap();
            let left = expires.with_timezone(&chrono::Utc) - chrono::Utc::now();
            assert!(left > chrono::Duration::minutes(55) && left <= chrono::Duration::minutes(61), "{mode}: an hour: {link}");

            let confirm = || {
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/password-reset/confirm")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&json!({ "token": token, "new_password": "a-brand-new-password" })).unwrap()))
                    .unwrap()
            };
            assert_eq!(app.clone().oneshot(confirm()).await.unwrap().status(), StatusCode::NO_CONTENT, "{mode}");
            let login = Request::builder()
                .method("POST")
                .uri("/v1/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&json!({ "email": "member@test.com", "password": "a-brand-new-password" })).unwrap()))
                .unwrap();
            assert_eq!(app.clone().oneshot(login).await.unwrap().status(), StatusCode::OK, "{mode}: the new password signs in");
            assert_eq!(app.clone().oneshot(confirm()).await.unwrap().status(), StatusCode::BAD_REQUEST, "{mode}: once only");
        }
    }

    /// Single-tenant: the deployment's Owners and Admins issue links for the
    /// accounts they manage, but not for the Owner (as PATCH and DELETE spare
    /// it); a Member issues none. An unknown account is a 404. A deactivated
    /// one gets a link from whoever may reactivate it (round 3b, S1). Each
    /// link is in the audit log.
    #[tokio::test]
    async fn single_tenant_admins_issue_reset_links_except_for_the_owner() {
        let db = build_db().await;
        let (owner, _, _) = seed_user_with_session(&db, Role::Owner).await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let (member, member_sess, member_csrf) = seed_user_with_session(&db, Role::Member).await;
        let app = crate::api::handlers::create_router(build_state(db.clone()).await);
        let admin = Some((admin_sess.as_str(), admin_csrf.as_str()));

        let resp = app.clone().oneshot(reset_link_req(&member.id, admin)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "an admin, for a member");
        let resp = app.clone().oneshot(reset_link_req(&owner.id, admin)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "an admin, for the Owner");
        let resp = app.clone().oneshot(reset_link_req(&owner.id, Some((member_sess.as_str(), member_csrf.as_str())))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a member");
        let resp = app.clone().oneshot(reset_link_req(&owner.id, None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "the master key, for the Owner");

        let resp = app.clone().oneshot(reset_link_req("u-nobody", None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(json_of(resp).await["error"]["code"], "user_not_found");
        db.update_user(&member.id, None, Some(false), None).await.unwrap();
        let resp = app.clone().oneshot(reset_link_req(&member.id, admin)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "an admin, for a deactivated member it may reactivate");

        let filter = crate::types::AuditLogFilter { action: Some("user.password_reset_link".into()), ..Default::default() };
        let mut entries = Vec::new();
        for _ in 0..200 {
            entries = db.list_audit_log(&filter, 1, 10).await.unwrap().0;
            if entries.len() >= 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let mut targets: Vec<(&str, &str)> = entries.iter().map(|e| (e.target_id.as_str(), e.actor_label.as_str())).collect();
        targets.sort();
        let mut expected = vec![(member.id.as_str(), "admin@test.com"), (member.id.as_str(), "admin@test.com"), (owner.id.as_str(), "master-key")];
        expected.sort();
        assert_eq!(targets, expected, "{entries:?}");
    }

    /// In an org, an Owner or Admin issues a link only for an account it
    /// outranks in every org that account belongs to: a link takes over the
    /// whole account, so one that also belongs elsewhere (a hosted signup's
    /// own org) is out of reach, and no one reaches their peers or betters.
    #[tokio::test]
    async fn hosted_an_orgs_owner_or_admin_resets_only_accounts_it_outranks_everywhere() {
        let db = build_db().await;
        let (org_a, org_b) = (seed_org(&db, "Acme").await, seed_org(&db, "Bravo").await);
        let (owner, owner_sess, owner_csrf) = seed_account(&db, "owner@acme.test").await;
        let (admin, admin_sess, admin_csrf) = seed_account(&db, "admin@acme.test").await;
        let (other_admin, _, _) = seed_account(&db, "admin2@acme.test").await;
        let (member, member_sess, member_csrf) = seed_account(&db, "member@acme.test").await;
        let (founder, _, _) = seed_account(&db, "founder@bravo.test").await;
        for (user, role) in [(&owner, Role::Owner), (&admin, Role::Admin), (&other_admin, Role::Admin), (&member, Role::Member), (&founder, Role::Member)] {
            db.add_org_member(&org_a, &user.id, role).await.unwrap();
        }
        db.add_org_member(&org_b, &founder.id, Role::Owner).await.unwrap();
        let app = crate::api::handlers::create_router(build_state_hosted(db.clone()).await);
        let as_owner = Some((owner_sess.as_str(), owner_csrf.as_str()));
        let as_admin = Some((admin_sess.as_str(), admin_csrf.as_str()));
        for (who, session, target, expected) in [
            ("the owner, for a member", as_owner, &member, StatusCode::OK),
            ("the owner, for an admin", as_owner, &admin, StatusCode::OK),
            ("an admin, for a member", as_admin, &member, StatusCode::OK),
            ("an admin, for the owner", as_admin, &owner, StatusCode::FORBIDDEN),
            ("an admin, for another admin", as_admin, &other_admin, StatusCode::FORBIDDEN),
            ("the owner, for one who also owns another org", as_owner, &founder, StatusCode::FORBIDDEN),
            ("a member, for an admin", Some((member_sess.as_str(), member_csrf.as_str())), &admin, StatusCode::FORBIDDEN),
            ("the master key, for anyone", None, &founder, StatusCode::OK),
        ] {
            let resp = app.clone().oneshot(reset_link_req(&target.id, session)).await.unwrap();
            assert_eq!(resp.status(), expected, "{who}");
        }
    }

    /// Review A1: in single-tenant mode too, a person issues a link only for
    /// an account they strictly outrank in every org it belongs to, and never
    /// for their own (the password change asks for the current password).
    /// Managing the users was enough before: an Admin took over peers, other
    /// orgs' members and themselves.
    #[tokio::test]
    async fn single_tenant_a_reset_link_needs_a_higher_role_everywhere_and_never_for_oneself() {
        let db = build_db().await;
        let (owner, owner_sess, owner_csrf) = seed_user_with_session(&db, Role::Owner).await;
        let (admin, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let (peer, _, _) = seed_account(&db, "peer@test.com").await;
        db.add_org_member(DEFAULT_ORG_ID, &peer.id, Role::Admin).await.unwrap();
        let (member, _, _) = seed_user_with_session(&db, Role::Member).await;
        let (elsewhere, _, _) = seed_account(&db, "elsewhere@test.com").await;
        db.add_org_member(DEFAULT_ORG_ID, &elsewhere.id, Role::Member).await.unwrap();
        let side = seed_org(&db, "Side").await;
        db.add_org_member(&side, &elsewhere.id, Role::Member).await.unwrap();
        let app = crate::api::handlers::create_router(build_state(db.clone()).await);
        let as_admin = Some((admin_sess.as_str(), admin_csrf.as_str()));
        let as_owner = Some((owner_sess.as_str(), owner_csrf.as_str()));
        for (who, session, target, expected) in [
            ("an admin, for a member", as_admin, &member, StatusCode::OK),
            ("an admin, for a peer admin", as_admin, &peer, StatusCode::FORBIDDEN),
            ("an admin, for a member of an org it doesn't run", as_admin, &elsewhere, StatusCode::FORBIDDEN),
            ("an admin, for itself", as_admin, &admin, StatusCode::FORBIDDEN),
            ("the owner, for an admin", as_owner, &peer, StatusCode::OK),
            ("the owner, for itself", as_owner, &owner, StatusCode::FORBIDDEN),
        ] {
            let resp = app.clone().oneshot(reset_link_req(&target.id, session)).await.unwrap();
            assert_eq!(resp.status(), expected, "{who}");
            if who.ends_with("itself") {
                let body = json_of(resp).await;
                assert!(body["error"]["message"].as_str().unwrap_or("").contains("PATCH /v1/account"), "{who}: {body}");
            }
        }
    }

    /// Review A2: changing a user's role in single-tenant mode (it sets
    /// their default-org role) takes a role above theirs, and never grants
    /// one at or above the caller's; each change is audited with the role
    /// before. An Admin could demote a peer, reset them, and promote them
    /// back.
    #[tokio::test]
    async fn single_tenant_a_role_change_needs_a_higher_role_and_is_audited() {
        let db = build_db().await;
        let (_, owner_sess, owner_csrf) = seed_user_with_session(&db, Role::Owner).await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let (peer, _, _) = seed_account(&db, "peer@test.com").await;
        db.add_org_member(DEFAULT_ORG_ID, &peer.id, Role::Admin).await.unwrap();
        let (member, _, _) = seed_user_with_session(&db, Role::Member).await;
        let app = build_users_router(build_state(db.clone()).await);
        let patch = |id: &str, role: &str, (sess, csrf): (&str, &str)| {
            Request::builder()
                .method("PATCH")
                .uri(format!("/v1/users/{id}"))
                .header("cookie", format!("litegen_session={sess}"))
                .header("x-csrf-token", csrf)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&json!({ "role": role })).unwrap()))
                .unwrap()
        };
        let admin = (admin_sess.as_str(), admin_csrf.as_str());
        let owner = (owner_sess.as_str(), owner_csrf.as_str());
        for (who, req, expected) in [
            ("an admin demoting a peer admin", patch(&peer.id, "member", admin), StatusCode::FORBIDDEN),
            ("an admin promoting a member to admin", patch(&member.id, "admin", admin), StatusCode::FORBIDDEN),
            ("an admin making a member a viewer", patch(&member.id, "viewer", admin), StatusCode::OK),
            ("the owner demoting an admin", patch(&peer.id, "member", owner), StatusCode::OK),
        ] {
            assert_eq!(app.clone().oneshot(req).await.unwrap().status(), expected, "{who}");
        }
        assert_eq!(db.get_membership(DEFAULT_ORG_ID, &peer.id).await.unwrap(), Some(Role::Member));
        let filter = crate::types::AuditLogFilter { action: Some("user.update".into()), ..Default::default() };
        let mut entries = Vec::new();
        for _ in 0..200 {
            entries = db.list_audit_log(&filter, 1, 10).await.unwrap().0;
            if entries.len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let demoted = entries.iter().find(|e| e.target_id == peer.id).unwrap_or_else(|| panic!("{entries:?}"));
        let after: serde_json::Value = serde_json::from_str(demoted.after_json.as_deref().unwrap_or("null")).unwrap();
        assert_eq!((after["previous_role"].as_str(), after["role"].as_str()), (Some("admin"), Some("member")), "{after}");
    }

    /// The outrank rule (review A2), for deactivation: a person deactivates
    /// an account (PATCH `is_active: false`, or DELETE) only when they
    /// outrank it in the deployment's org, so never their own; the master
    /// key, any but the Owner. Deactivating and inviting back lower is a
    /// demotion by another door. A refusal changes nothing: the account's
    /// invitations stay.
    #[tokio::test]
    async fn single_tenant_deactivating_needs_a_higher_role() {
        let db = build_db().await;
        let (_, owner_sess, owner_csrf) = seed_user_with_session(&db, Role::Owner).await;
        let (admin, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let (peer, _, _) = seed_account(&db, "peer@test.com").await;
        db.add_org_member(DEFAULT_ORG_ID, &peer.id, Role::Admin).await.unwrap();
        let (member, _, _) = seed_user_with_session(&db, Role::Member).await;
        let (viewer, _, _) = seed_user_with_session(&db, Role::Viewer).await;
        let sent = Invitation { invited_by: Some(peer.id.clone()), ..make_invitation("guest@test.com", "peer-sent", Role::Member, DEFAULT_ORG_ID) };
        db.create_invitation(&sent).await.unwrap();
        let app = build_users_router(build_state(db.clone()).await);
        let call = |method: &str, id: &str, body: Option<serde_json::Value>, auth: (&str, &str)| {
            let req = Request::builder().method(method).uri(format!("/v1/users/{id}"));
            let req = match auth {
                ("master", _) => req.header("authorization", "Bearer master"),
                (sess, csrf) => req.header("cookie", format!("litegen_session={sess}")).header("x-csrf-token", csrf),
            };
            match body {
                Some(body) => req.header("content-type", "application/json").body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap(),
                None => req.body(Body::empty()).unwrap(),
            }
        };
        let off = || Some(json!({ "is_active": false }));
        let (as_admin, as_owner, as_master) = ((admin_sess.as_str(), admin_csrf.as_str()), (owner_sess.as_str(), owner_csrf.as_str()), ("master", ""));
        for (who, req) in [
            ("an admin deactivating a peer admin", call("PATCH", &peer.id, off(), as_admin)),
            ("an admin deleting a peer admin", call("DELETE", &peer.id, None, as_admin)),
            ("an admin deactivating themself", call("PATCH", &admin.id, off(), as_admin)),
            ("an admin deleting themself", call("DELETE", &admin.id, None, as_admin)),
        ] {
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{who}");
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["error"]["code"], "forbidden_permission", "{who}: {body}");
        }
        for still in [&peer.id, &admin.id] {
            assert!(db.get_user_by_id(still).await.unwrap().unwrap().is_active, "refused: {still} stays active");
        }
        assert_eq!(db.list_invitations().await.unwrap().len(), 1, "a refusal revokes none of their invitations");
        for (who, req, expected) in [
            ("an admin deactivating a member", call("PATCH", &member.id, off(), as_admin), StatusCode::OK),
            ("an admin deleting a viewer", call("DELETE", &viewer.id, None, as_admin), StatusCode::NO_CONTENT),
            ("the owner deactivating an admin", call("PATCH", &peer.id, off(), as_owner), StatusCode::OK),
            ("the master key deleting an admin", call("DELETE", &admin.id, None, as_master), StatusCode::NO_CONTENT),
        ] {
            assert_eq!(app.clone().oneshot(req).await.unwrap().status(), expected, "{who}");
        }
        for gone in [&member.id, &viewer.id, &peer.id, &admin.id] {
            assert!(!db.get_user_by_id(gone).await.unwrap().unwrap().is_active, "{gone} deactivated");
        }
    }

    /// Every permission check in PATCH /v1/users/{id} comes before its first
    /// write: a role change it refuses revokes nothing, even when the same
    /// PATCH also deactivates an account the caller may deactivate.
    #[tokio::test]
    async fn a_refused_role_change_revokes_nothing() {
        let db = build_db().await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let (member, _, _) = seed_user_with_session(&db, Role::Member).await;
        let sent = Invitation { invited_by: Some(member.id.clone()), ..make_invitation("guest@test.com", "member-sent", Role::Viewer, DEFAULT_ORG_ID) };
        db.create_invitation(&sent).await.unwrap();
        let app = build_users_router(build_state(db.clone()).await);
        let req = Request::builder()
            .method("PATCH")
            .uri(format!("/v1/users/{}", member.id))
            .header("cookie", format!("litegen_session={admin_sess}"))
            .header("x-csrf-token", &admin_csrf)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json!({ "role": "admin", "is_active": false })).unwrap()))
            .unwrap();
        assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::FORBIDDEN, "an admin can't grant admin");
        assert_eq!(db.list_invitations().await.unwrap().len(), 1, "the member's invitation stays");
        let after = db.get_user_by_id(&member.id).await.unwrap().unwrap();
        assert!(after.is_active, "still active");
        assert_eq!(db.get_membership(DEFAULT_ORG_ID, &member.id).await.unwrap(), Some(Role::Member), "still a member");
    }

    /// Round 3b, S1: reactivating takes the same rule as deactivating, or a
    /// peer Admin could undo the Owner's deactivation of an Admin. Either
    /// way, deactivating ends the account's sessions at once.
    #[tokio::test]
    async fn single_tenant_reactivating_needs_a_higher_role_and_deactivating_ends_sessions() {
        let db = build_db().await;
        let (_, owner_sess, owner_csrf) = seed_user_with_session(&db, Role::Owner).await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let (peer, peer_sess, _) = seed_account(&db, "peer@test.com").await;
        db.add_org_member(DEFAULT_ORG_ID, &peer.id, Role::Admin).await.unwrap();
        let (member, member_sess, _) = seed_user_with_session(&db, Role::Member).await;
        let app = build_users_router(build_state(db.clone()).await);
        let set_active = |id: &str, on: bool, (sess, csrf): (&str, &str)| {
            Request::builder()
                .method("PATCH")
                .uri(format!("/v1/users/{id}"))
                .header("cookie", format!("litegen_session={sess}"))
                .header("x-csrf-token", csrf)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&json!({ "is_active": on })).unwrap()))
                .unwrap()
        };
        let (as_admin, as_owner) = ((admin_sess.as_str(), admin_csrf.as_str()), (owner_sess.as_str(), owner_csrf.as_str()));
        for sess in [&peer_sess, &member_sess] {
            assert!(db.get_session(&crate::auth::tokens::token_hash(sess)).await.unwrap().is_some(), "stored by its token's hash");
        }

        assert_eq!(app.clone().oneshot(set_active(&peer.id, false, as_owner)).await.unwrap().status(), StatusCode::OK, "the owner deactivates an admin");
        assert!(db.get_session(&crate::auth::tokens::token_hash(&peer_sess)).await.unwrap().is_none(), "its session ends at once");
        let resp = app.clone().oneshot(set_active(&peer.id, true, as_admin)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a peer admin can't undo it");
        assert_eq!(json_of(resp).await["error"]["code"], "forbidden_permission");
        assert!(!db.get_user_by_id(&peer.id).await.unwrap().unwrap().is_active);
        assert_eq!(app.clone().oneshot(set_active(&peer.id, true, as_owner)).await.unwrap().status(), StatusCode::OK, "the owner may");
        assert!(db.get_user_by_id(&peer.id).await.unwrap().unwrap().is_active);

        let delete = Request::builder()
            .method("DELETE")
            .uri(format!("/v1/users/{}", member.id))
            .header("cookie", format!("litegen_session={admin_sess}"))
            .header("x-csrf-token", &admin_csrf)
            .body(Body::empty())
            .unwrap();
        assert_eq!(app.clone().oneshot(delete).await.unwrap().status(), StatusCode::NO_CONTENT, "an admin deletes a member");
        assert!(db.get_session(&crate::auth::tokens::token_hash(&member_sess)).await.unwrap().is_none(), "deleting ends its session too");
        assert_eq!(app.oneshot(set_active(&member.id, true, as_admin)).await.unwrap().status(), StatusCode::OK, "and may reactivate it");
    }

    /// Round 3b, S1: a deactivated account gets a reset link only from one
    /// who may reactivate it (confirming it reactivates the account): in
    /// single-tenant mode a person who outranks it, and in either mode the
    /// master key. An org's Owner in hosted mode can't reactivate anyone.
    #[tokio::test]
    async fn a_reset_link_for_a_deactivated_account_comes_from_one_who_may_reactivate_it() {
        let db = build_db().await;
        let (_, _, _) = seed_user_with_session(&db, Role::Owner).await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let (peer, _, _) = seed_account(&db, "peer@test.com").await;
        db.add_org_member(DEFAULT_ORG_ID, &peer.id, Role::Admin).await.unwrap();
        let (member, _, _) = seed_user_with_session(&db, Role::Member).await;
        for gone in [&peer.id, &member.id] {
            db.update_user(gone, None, Some(false), None).await.unwrap();
        }
        let app = crate::api::handlers::create_router(build_state(db.clone()).await);
        let admin = Some((admin_sess.as_str(), admin_csrf.as_str()));
        assert_eq!(app.clone().oneshot(reset_link_req(&member.id, admin)).await.unwrap().status(), StatusCode::OK, "an admin, for a deactivated member");
        let resp = app.clone().oneshot(reset_link_req(&peer.id, admin)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "an admin, for a deactivated peer");
        assert_eq!(json_of(resp).await["error"]["code"], "forbidden_permission");
        assert_eq!(app.clone().oneshot(reset_link_req(&peer.id, None)).await.unwrap().status(), StatusCode::OK, "the master key");

        let hosted_db = build_db().await;
        let org = seed_org(&hosted_db, "Acme").await;
        let (owner, owner_sess, owner_csrf) = seed_account(&hosted_db, "owner@acme.test").await;
        let (gone, _, _) = seed_account(&hosted_db, "gone@acme.test").await;
        hosted_db.add_org_member(&org, &owner.id, Role::Owner).await.unwrap();
        hosted_db.add_org_member(&org, &gone.id, Role::Member).await.unwrap();
        hosted_db.update_user(&gone.id, None, Some(false), None).await.unwrap();
        let hosted = crate::api::handlers::create_router(build_state_hosted(hosted_db.clone()).await);
        let resp = hosted.clone().oneshot(reset_link_req(&gone.id, Some((owner_sess.as_str(), owner_csrf.as_str())))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "hosted: an org's Owner, for its deactivated member");
        assert_eq!(hosted.oneshot(reset_link_req(&gone.id, None)).await.unwrap().status(), StatusCode::OK, "hosted: the master key");
    }

    /// Round 3b, S4: in single-tenant mode an account with no role in the
    /// deployment's org still takes the role-change rule, with its own
    /// account role as the current one: a person must outrank that and the
    /// new role. Before, its `users.role` could be written to Admin by an
    /// Admin, which `/v1/auth/me`, `/v1/users` and hooks report.
    #[tokio::test]
    async fn single_tenant_a_role_for_an_account_with_no_org_role_still_needs_a_higher_role() {
        let db = build_db().await;
        let (_, owner_sess, owner_csrf) = seed_user_with_session(&db, Role::Owner).await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let (loose, _, _) = seed_account(&db, "loose@test.com").await;
        let (stale_admin, _, _) = seed_account(&db, "stale@test.com").await;
        db.update_user(&stale_admin.id, Some(Role::Admin), None, None).await.unwrap();
        let app = build_users_router(build_state(db.clone()).await);
        let patch = |id: &str, role: &str, (sess, csrf): (&str, &str)| {
            Request::builder()
                .method("PATCH")
                .uri(format!("/v1/users/{id}"))
                .header("cookie", format!("litegen_session={sess}"))
                .header("x-csrf-token", csrf)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&json!({ "role": role })).unwrap()))
                .unwrap()
        };
        let (as_admin, as_owner) = ((admin_sess.as_str(), admin_csrf.as_str()), (owner_sess.as_str(), owner_csrf.as_str()));
        for (who, req, expected) in [
            ("an admin making an org-less member an admin", patch(&loose.id, "admin", as_admin), StatusCode::FORBIDDEN),
            ("an admin moving an org-less admin", patch(&stale_admin.id, "viewer", as_admin), StatusCode::FORBIDDEN),
            ("an admin making an org-less member a viewer", patch(&loose.id, "viewer", as_admin), StatusCode::OK),
            ("the owner making an org-less account an admin", patch(&loose.id, "admin", as_owner), StatusCode::OK),
        ] {
            assert_eq!(app.clone().oneshot(req).await.unwrap().status(), expected, "{who}");
        }
        assert_eq!(db.get_user_by_id(&stale_admin.id).await.unwrap().unwrap().role, Role::Admin, "refused: unchanged");
        assert_eq!(db.get_user_by_id(&loose.id).await.unwrap().unwrap().role, Role::Admin);
    }

    /// Round 3b, S4: inviting takes the role rule too, or "remove a Member,
    /// invite them back as Admin" promoted past it: an Admin invites Members
    /// and Viewers, an Owner Admins as well (`POST /v1/users`).
    #[tokio::test]
    async fn single_tenant_inviting_needs_a_role_above_the_invited_one() {
        let db = build_db().await;
        let (_, owner_sess, owner_csrf) = seed_user_with_session(&db, Role::Owner).await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let app = build_users_router(build_state(db.clone()).await);
        let invite = |email: &str, role: &str, (sess, csrf): (&str, &str)| {
            Request::builder()
                .method("POST")
                .uri("/v1/users")
                .header("cookie", format!("litegen_session={sess}"))
                .header("x-csrf-token", csrf)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&json!({ "email": email, "role": role })).unwrap()))
                .unwrap()
        };
        let (as_admin, as_owner) = ((admin_sess.as_str(), admin_csrf.as_str()), (owner_sess.as_str(), owner_csrf.as_str()));
        let resp = app.clone().oneshot(invite("new-admin@test.com", "admin", as_admin)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "an admin inviting an admin");
        assert_eq!(json_of(resp).await["error"]["code"], "forbidden_permission");
        for (who, req) in [
            ("an admin inviting a member", invite("new-member@test.com", "member", as_admin)),
            ("an admin inviting a viewer", invite("new-viewer@test.com", "viewer", as_admin)),
            ("the owner inviting an admin", invite("new-admin@test.com", "admin", as_owner)),
        ] {
            assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::OK, "{who}");
        }
        let mut invited: Vec<String> = db.list_invitations().await.unwrap().into_iter().map(|i| i.email).collect();
        invited.sort();
        assert_eq!(invited, ["new-admin@test.com", "new-member@test.com", "new-viewer@test.com"]);
    }

    /// Review round 5, finding 3: the invitations a deactivation revokes
    /// (SEC-INVITES) are in the audit log, each one, as a manual revoke is.
    #[tokio::test]
    async fn deactivating_audits_the_invitations_it_revokes() {
        let db = build_db().await;
        let (_, owner_sess, owner_csrf) = seed_user_with_session(&db, Role::Owner).await;
        let (admin, _, _) = seed_user_with_session(&db, Role::Admin).await;
        let sent = Invitation { invited_by: Some(admin.id.clone()), ..make_invitation("guest@test.com", "admin-sent", Role::Member, DEFAULT_ORG_ID) };
        db.create_invitation(&sent).await.unwrap();
        let app = build_users_router(build_state(db.clone()).await);
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/v1/users/{}", admin.id))
            .header("cookie", format!("litegen_session={owner_sess}"))
            .header("x-csrf-token", &owner_csrf)
            .body(Body::empty())
            .unwrap();
        assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::NO_CONTENT);
        assert!(db.list_invitations().await.unwrap().is_empty(), "revoked");
        // Written before the answer went out (review round 7, cleanup): one
        // read, no polling.
        let filter = crate::types::AuditLogFilter { action: Some("invitation.revoke".into()), ..Default::default() };
        let entries = db.list_audit_log(&filter, 1, 10).await.unwrap().0;
        assert_eq!(entries.iter().map(|e| e.target_id.as_str()).collect::<Vec<_>>(), [sent.id.as_str()], "{entries:?}");
        assert!(entries[0].after_json.as_deref().is_some_and(|a| a.contains(r#""reason":"inviter_deactivated""#)), "{entries:?}");
    }

    /// Review round 6, finding 6: every invitation revoke is audited, as a
    /// deactivation's is: here a role change to one that can't invite.
    #[tokio::test]
    async fn a_role_change_audits_the_invitations_it_revokes() {
        let db = build_db().await;
        let (_, owner_sess, owner_csrf) = seed_user_with_session(&db, Role::Owner).await;
        let (admin, _, _) = seed_user_with_session(&db, Role::Admin).await;
        let sent = Invitation { invited_by: Some(admin.id.clone()), ..make_invitation("guest@test.com", "admin-sent", Role::Member, DEFAULT_ORG_ID) };
        db.create_invitation(&sent).await.unwrap();
        let app = build_users_router(build_state(db.clone()).await);
        let req = Request::builder()
            .method("PATCH")
            .uri(format!("/v1/users/{}", admin.id))
            .header("cookie", format!("litegen_session={owner_sess}"))
            .header("x-csrf-token", &owner_csrf)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json!({ "role": "member" })).unwrap()))
            .unwrap();
        assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::OK);
        assert!(db.list_invitations().await.unwrap().is_empty(), "revoked");
        let filter = crate::types::AuditLogFilter { action: Some("invitation.revoke".into()), ..Default::default() };
        let entries = db.list_audit_log(&filter, 1, 10).await.unwrap().0;
        assert_eq!(entries.iter().map(|e| e.target_id.as_str()).collect::<Vec<_>>(), [sent.id.as_str()], "{entries:?}");
        assert!(entries[0].after_json.as_deref().is_some_and(|a| a.contains(r#""reason":"inviter_role_changed""#)), "{entries:?}");
        assert_eq!(entries[0].actor_label, "owner@test.com");
    }

    /// Review round 7, finding 3: a role change writes `users.role` and the
    /// default-org membership, where the permissions come from, in one
    /// transaction. When the membership write fails, neither role moves, so
    /// the Users page never shows a role the permissions don't match.
    #[tokio::test]
    async fn a_role_change_whose_membership_write_fails_moves_neither_role() {
        let db = build_db().await;
        let (admin, _, _) = seed_user_with_session(&db, Role::Admin).await;
        // The membership write fails, as a dropped connection would fail it.
        sqlx::query(&format!(
            "CREATE TRIGGER membership_write_fails BEFORE UPDATE ON organization_members \
             WHEN NEW.user_id = '{}' BEGIN SELECT RAISE(ABORT, 'membership write failed'); END",
            admin.id
        ))
        .execute(db.pool())
        .await
        .unwrap();
        let app = build_users_router(build_state(db.clone()).await);
        let req = Request::builder()
            .method("PATCH")
            .uri(format!("/v1/users/{}", admin.id))
            .header("authorization", "Bearer master")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"role":"member"}"#))
            .unwrap();
        assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(db.get_user_by_id(&admin.id).await.unwrap().unwrap().role, Role::Admin, "users.role moves only with the membership");
        assert_eq!(db.get_membership(DEFAULT_ORG_ID, &admin.id).await.unwrap(), Some(Role::Admin));
    }

    /// Review round 7, finding 6: accepting an invitation checks that whoever
    /// sent it may still send it, as confirming a reset link checks its
    /// issuer. The sender must still be active, and a member of the
    /// invitation's org whose role may invite and is above the invited one.
    /// So one that lands after its sender was demoted, removed or deactivated
    /// (in flight while their other invitations were revoked) can't be
    /// accepted. The master key's invitations name no sender.
    #[tokio::test]
    async fn an_invitation_whose_sender_may_no_longer_send_it_is_refused() {
        for change in ["none", "demoted", "outranked", "removed", "deactivated"] {
            let db = build_db().await;
            let (owner, _, _) = seed_user_with_session(&db, Role::Owner).await;
            let (admin, _, _) = seed_user_with_session(&db, Role::Admin).await;
            // The Admin sends a Member invitation; the Owner, an Admin one.
            let (sender, role) = if change == "outranked" { (&owner, Role::Admin) } else { (&admin, Role::Member) };
            match change {
                "demoted" => db.update_member_role(DEFAULT_ORG_ID, &admin.id, Role::Member).await.unwrap(),
                // Now an Admin, the former Owner can't invite an Admin.
                "outranked" => db.update_member_role(DEFAULT_ORG_ID, &owner.id, Role::Admin).await.unwrap(),
                "removed" => db.remove_org_member(DEFAULT_ORG_ID, &admin.id).await.unwrap(),
                "deactivated" => assert!(db.update_user(&admin.id, None, Some(false), None).await.unwrap().is_some()),
                _ => {}
            }
            // Stored after the change, as one in flight at the change lands.
            let token = format!("sent-{change}");
            let sent = Invitation { invited_by: Some(sender.id.clone()), ..make_invitation(&format!("guest-{change}@test.com"), &token, role, DEFAULT_ORG_ID) };
            db.create_invitation(&sent).await.unwrap();
            let app = build_users_router(build_state(db.clone()).await);
            let req = Request::builder()
                .method("POST")
                .uri(format!("/v1/auth/invitations/{token}/accept"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"password":"a-long-test-password-1"}"#))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let body: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
            if change == "none" {
                assert_eq!(status, StatusCode::OK, "{change}: the sender may still send it: {body}");
                continue;
            }
            assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invitation_revoked")), "{change}: {body}");
            assert!(db.get_user_by_email(&sent.email).await.unwrap().is_none(), "{change}: no account made");
            assert!(db.get_invitation(&token).await.unwrap().is_some_and(|i| i.used_at.is_none()), "{change}: not consumed");
        }
    }

    /// Review round 6, finding 2: an invitation (`POST /v1/users`, `POST
    /// /v1/orgs/{id}/members`) or a reset link whose request passed the
    /// session check before the sender's reset or deactivation committed
    /// isn't stored: 401 `sign_in_ended`. A layer between the session check
    /// and the handler commits the reset or the deactivation.
    #[tokio::test]
    async fn an_invitation_or_link_sent_as_the_senders_sign_ins_end_is_refused() {
        for ending in [None, Some("reset"), Some("deactivation")] {
            let db = build_db().await;
            let (admin, _, _) = seed_user_with_session(&db, Role::Admin).await;
            let (member, _, _) = seed_user_with_session(&db, Role::Member).await;
            let state = build_state(db.clone()).await;
            let (ends, admin_id, auth_state) = (db.clone(), admin.id.clone(), state.clone());
            let app = Router::new()
                .route("/v1/users", post(invite_user))
                .route("/v1/users/{id}/password-reset", post(issue_password_reset))
                .route("/v1/orgs/{id}/members", post(crate::api::handlers::orgs::invite_member))
                // After the session check, before the handler: the sender's
                // sign-ins end, as a reset or a deactivation committing while
                // the request runs would end them.
                .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                    let (db, admin_id) = (ends.clone(), admin_id.clone());
                    async move {
                        match ending {
                            Some("reset") => {
                                let (token, now) = (generate_session_token(), chrono::Utc::now());
                                let rescue = crate::types::PasswordReset { token: token.clone(), user_id: admin_id, expires_at: now + chrono::Duration::hours(1), used_at: None, created_at: now, issued_by: None };
                                db.create_password_reset(&rescue).await.unwrap();
                                let confirmed = db.confirm_password_reset(&token, "reset-hash", crate::config::Mode::SingleTenant).await.unwrap();
                                assert!(matches!(confirmed, crate::db::ResetConfirm::Done { .. }));
                            }
                            Some(_) => {
                                db.update_user(&admin_id, None, Some(false), None).await.unwrap();
                            }
                            None => {}
                        }
                        next.run(req).await
                    }
                }))
                .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                    let s = auth_state.clone();
                    async move {
                        let headers = req.headers().clone();
                        auth_middleware(headers, s, req, next).await
                    }
                }))
                .with_state(state);
            for (what, path, body) in [
                ("POST /v1/users", "/v1/users".to_string(), Some(json!({ "email": "stand-in@test.com", "role": "member" }))),
                ("POST /v1/orgs/{id}/members", format!("/v1/orgs/{DEFAULT_ORG_ID}/members"), Some(json!({ "email": "stand-in-2@test.com", "role": "member" }))),
                ("POST /v1/users/{id}/password-reset", format!("/v1/users/{}/password-reset", member.id), None),
            ] {
                // A live session for each request (the last one's ended).
                db.update_user(&admin.id, None, Some(true), None).await.unwrap();
                let (sid, csrf, now) = (generate_session_token(), generate_csrf_token(), chrono::Utc::now());
                let session = Session { id: sid.clone(), user_id: admin.id.clone(), created_at: now, expires_at: now + chrono::Duration::days(7), ip: None, user_agent: None, csrf_token: csrf.clone() };
                db.create_session(&session).await.unwrap();
                let req = Request::builder().method("POST").uri(&path).header("cookie", format!("litegen_session={sid}")).header("x-csrf-token", &csrf);
                let req = match body {
                    Some(body) => req.header("content-type", "application/json").body(Body::from(serde_json::to_vec(&body).unwrap())),
                    None => req.body(Body::empty()),
                };
                let resp = app.clone().oneshot(req.unwrap()).await.unwrap();
                match ending {
                    None => assert_eq!(resp.status(), StatusCode::OK, "{what}: from a live sign-in"),
                    Some(ending) => {
                        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{what}, as a {ending} commits");
                        let body = json_of(resp).await;
                        assert_eq!(body["error"]["code"], "sign_in_ended", "{what}, as a {ending} commits: {body}");
                    }
                }
            }
            let invitations = db.list_invitations().await.unwrap().len();
            let links: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM password_resets WHERE user_id = ?").bind(&member.id).fetch_one(db.pool()).await.unwrap();
            assert_eq!((invitations, links), if ending.is_some() { (0, 0) } else { (2, 1) }, "{ending:?}: what was stored");
        }
    }

    /// Review round 4, finding 1: in hosted mode ownership lives in each
    /// org's membership. An account's own role (Owner, for every
    /// self-signup) makes no deployment Owner, so the master key may
    /// deactivate it, by PATCH or DELETE, as the compromised-account flow
    /// needs.
    #[tokio::test]
    async fn hosted_the_master_key_deactivates_a_self_signup_account() {
        let db = build_db().await;
        let (first, _, _) = seed_account(&db, "first@acme.test").await;
        let (second, _, _) = seed_account(&db, "second@bravo.test").await;
        for signed_up in [&first, &second] {
            db.update_user(&signed_up.id, Some(Role::Owner), None, None).await.unwrap();
        }
        let app = build_users_router(build_state_hosted(db.clone()).await);
        let patch = Request::builder()
            .method("PATCH")
            .uri(format!("/v1/users/{}", first.id))
            .header("authorization", "Bearer master")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json!({ "is_active": false })).unwrap()))
            .unwrap();
        assert_eq!(app.clone().oneshot(patch).await.unwrap().status(), StatusCode::OK, "PATCH");
        let delete = Request::builder().method("DELETE").uri(format!("/v1/users/{}", second.id)).header("authorization", "Bearer master").body(Body::empty()).unwrap();
        assert_eq!(app.oneshot(delete).await.unwrap().status(), StatusCode::NO_CONTENT, "DELETE");
        for gone in [&first.id, &second.id] {
            assert!(!db.get_user_by_id(gone).await.unwrap().unwrap().is_active);
        }
    }

    /// Review round 4, finding 1: the deployment-level transfer rewrites
    /// `users.role` for every account that holds Owner, which in hosted mode
    /// is every tenant's signup. There it is refused, pointing at each org's
    /// own transfer, and changes nothing.
    #[tokio::test]
    async fn hosted_the_deployment_transfer_owner_is_refused() {
        let db = build_db().await;
        let (tenant_a, _, _) = seed_account(&db, "owner@acme.test").await;
        let (tenant_b, _, _) = seed_account(&db, "owner@bravo.test").await;
        for owner in [&tenant_a, &tenant_b] {
            db.update_user(&owner.id, Some(Role::Owner), None, None).await.unwrap();
        }
        let app = build_users_router(build_state_hosted(db.clone()).await);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/users/transfer-owner")
            .header("authorization", "Bearer master")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json!({ "new_owner_id": tenant_b.id })).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_of(resp).await["error"]["code"], "transfer_owner_per_org");
        assert_eq!(db.get_user_by_id(&tenant_a.id).await.unwrap().unwrap().role, Role::Owner, "no tenant's role moved");
    }

    /// Review A2: every role change is audited with the old role and the
    /// new, in hosted mode too, where only the master key sets an account's
    /// own role.
    #[tokio::test]
    async fn hosted_a_role_change_by_the_master_key_records_the_old_role() {
        let db = build_db().await;
        let (user, _, _) = seed_user_with_session(&db, Role::Member).await;
        let app = build_users_router(build_state_hosted(db.clone()).await);
        let req = Request::builder()
            .method("PATCH")
            .uri(format!("/v1/users/{}", user.id))
            .header("authorization", "Bearer master")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json!({ "role": "viewer" })).unwrap()))
            .unwrap();
        assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::OK);
        let filter = crate::types::AuditLogFilter { action: Some("user.update".into()), ..Default::default() };
        let mut entries = Vec::new();
        for _ in 0..200 {
            entries = db.list_audit_log(&filter, 1, 10).await.unwrap().0;
            if !entries.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let changed = entries.iter().find(|e| e.target_id == user.id).unwrap_or_else(|| panic!("{entries:?}"));
        let after: serde_json::Value = serde_json::from_str(changed.after_json.as_deref().unwrap_or("null")).unwrap();
        assert_eq!((after["previous_role"].as_str(), after["role"].as_str()), (Some("member"), Some("viewer")), "{after}");
    }

    /// Review A3: a reset link is filed under the account's org (each org it
    /// belongs to), as `audit_org` files an entry about an org's member, not
    /// under the caller's: the hosted master key acts in none.
    #[tokio::test]
    async fn a_reset_link_is_filed_under_the_accounts_org() {
        let db = build_db().await;
        let org = seed_org(&db, "Acme").await;
        let (member, _, _) = seed_account(&db, "member@acme.test").await;
        db.add_org_member(&org, &member.id, Role::Member).await.unwrap();
        let app = crate::api::handlers::create_router(build_state_hosted(db.clone()).await);
        assert_eq!(app.oneshot(reset_link_req(&member.id, None)).await.unwrap().status(), StatusCode::OK);
        let filter = crate::types::AuditLogFilter { action: Some("user.password_reset_link".into()), ..Default::default() };
        let mut entries = Vec::new();
        for _ in 0..200 {
            entries = db.list_audit_log(&filter, 1, 10).await.unwrap().0;
            if !entries.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(entries.iter().map(|e| e.org_id.as_deref()).collect::<Vec<_>>(), [Some(org.as_str())], "{entries:?}");
    }

    /// An app signed in with OAuth acts only in the org its grant names: it
    /// reaches an account only when every org that account belongs to is
    /// that one.
    #[tokio::test]
    async fn an_oauth_sign_in_resets_only_within_its_grant() {
        use crate::api::middleware::{McpPrincipal, Scope, UserContext};
        let db = build_db().await;
        let (org_a, org_c) = (seed_org(&db, "Acme").await, seed_org(&db, "Charlie").await);
        let (owner, _, _) = seed_account(&db, "owner@acme.test").await;
        let (only_here, _, _) = seed_account(&db, "here@acme.test").await;
        let (also_there, _, _) = seed_account(&db, "there@acme.test").await;
        for org in [&org_a, &org_c] {
            db.add_org_member(org, &owner.id, Role::Owner).await.unwrap();
            db.add_org_member(org, &also_there.id, Role::Member).await.unwrap();
        }
        db.add_org_member(&org_a, &only_here.id, Role::Member).await.unwrap();
        // The grant the app's sign-in stands on: a link is stored only while
        // it is live (review round 6, finding 2).
        let now = chrono::Utc::now();
        let stored = crate::mcp::oauth::store::Grant {
            id: "grant-acme".into(),
            client_id: "lgc-acme".into(),
            user_id: owner.id.clone(),
            org_id: org_a.clone(),
            app_id: None,
            surface: crate::mcp::Surface::Admin,
            scopes: vec!["admin:read".into(), "admin:write".into()],
            created_at: now,
            last_used_at: None,
            revoked_at: None,
            sign_in_epoch: 0,
        };
        assert!(db.mcp_oauth().expect("an OAuth store").claim_grant(&stored).await.unwrap().is_some());
        let grant = KeyContext {
            key_id: None,
            scopes: vec![Scope::Read, Scope::Admin],
            rpm_limit: None,
            quota_remaining: None,
            webhook_url: None,
            user: Some(UserContext { user_id: owner.id.clone(), email: owner.email.clone(), role: Role::Owner }),
            permissions: crate::auth::permissions::permissions_for(Role::Owner).to_vec(),
            session_id: None,
            org_id: Some(org_a.clone()),
            app_id: None,
            oauth_grant_id: Some("grant-acme".into()),
        };
        let app = crate::api::handlers::create_router(build_state_hosted(db.clone()).await).layer(middleware::from_fn(
            move |mut req: axum::extract::Request, next: middleware::Next| {
                let grant = grant.clone();
                async move {
                    req.extensions_mut().insert(McpPrincipal(grant));
                    next.run(req).await
                }
            },
        ));
        let call = |id: &str| Request::builder().method("POST").uri(format!("/v1/users/{id}/password-reset")).body(Body::empty()).unwrap();
        assert_eq!(app.clone().oneshot(call(&only_here.id)).await.unwrap().status(), StatusCode::OK, "in the grant's org only");
        assert_eq!(app.clone().oneshot(call(&also_there.id)).await.unwrap().status(), StatusCode::FORBIDDEN, "also in another org");
    }

    #[tokio::test]
    async fn invite_with_dev_token_exposure_includes_token() {
        let db = build_db().await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        // Build state with expose_invite_tokens: true (parsed config flag, not env var).
        let mut state = build_state(db).await;
        Arc::get_mut(&mut state)
            .expect("unique Arc")
            .dev
            .expose_invite_tokens = true;
        let app = build_users_router(state);

        let body = serde_json::to_vec(&json!({ "email": "tokentest@example.com", "role": "viewer" })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/users")
            .header("cookie", format!("litegen_session={}", admin_sess))
            .header("x-csrf-token", admin_csrf)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json["_dev_token"].is_string(), "should expose _dev_token");
    }

    // ── GET /v1/auth/invitations/{token} ─────────────────────────────────────

    #[tokio::test]
    async fn get_invitation_returns_email_and_role() {
        let db = build_db().await;
        let inv = Invitation {
            id: Uuid::new_v4().to_string(),
            email: "invited@example.com".to_string(),
            role: Role::Member,
            token: "test-token-abc".to_string(),
            invited_by: None,
            org_id: crate::api::middleware::DEFAULT_ORG_ID.to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            used_at: None,
            created_at: chrono::Utc::now(),
        };
        db.create_invitation(&inv).await.unwrap();
        let app = build_users_router(build_state(db).await);

        let req = Request::builder()
            .uri("/v1/auth/invitations/test-token-abc")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["email"], "invited@example.com");
        assert_eq!(json["role"], "member");
    }

    #[tokio::test]
    async fn get_expired_invitation_returns_400() {
        let db = build_db().await;
        let inv = Invitation {
            id: Uuid::new_v4().to_string(),
            email: "expired@example.com".to_string(),
            role: Role::Member,
            token: "expired-token".to_string(),
            invited_by: None,
            org_id: crate::api::middleware::DEFAULT_ORG_ID.to_string(),
            expires_at: chrono::Utc::now() - chrono::Duration::hours(1),
            used_at: None,
            created_at: chrono::Utc::now() - chrono::Duration::days(8),
        };
        db.create_invitation(&inv).await.unwrap();
        let app = build_users_router(build_state(db).await);

        let req = Request::builder()
            .uri("/v1/auth/invitations/expired-token")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── POST /v1/auth/invitations/{token}/accept ──────────────────────────────

    #[tokio::test]
    async fn accept_invitation_creates_user_and_session() {
        let db = build_db().await;
        let inv = Invitation {
            id: Uuid::new_v4().to_string(),
            email: "accept@example.com".to_string(),
            role: Role::Member,
            token: "accept-token".to_string(),
            invited_by: None,
            org_id: crate::api::middleware::DEFAULT_ORG_ID.to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            used_at: None,
            created_at: chrono::Utc::now(),
        };
        db.create_invitation(&inv).await.unwrap();
        let app = build_users_router(build_state(db.clone()).await);

        let body = serde_json::to_vec(&json!({ "password": "super-secret-pw-123" })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/invitations/accept-token/accept")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify user was created
        let user = db.get_user_by_email("accept@example.com").await.unwrap();
        assert!(user.is_some());
        assert_eq!(user.unwrap().role, Role::Member);

        // Verify invitation is marked used
        let inv_after = db.get_invitation("accept-token").await.unwrap().unwrap();
        assert!(inv_after.used_at.is_some());
    }

    #[tokio::test]
    async fn accept_used_invitation_returns_400() {
        let db = build_db().await;
        let inv = Invitation {
            id: Uuid::new_v4().to_string(),
            email: "used@example.com".to_string(),
            role: Role::Member,
            token: "used-token".to_string(),
            invited_by: None,
            org_id: crate::api::middleware::DEFAULT_ORG_ID.to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            used_at: Some(chrono::Utc::now() - chrono::Duration::hours(1)),
            created_at: chrono::Utc::now(),
        };
        db.create_invitation(&inv).await.unwrap();
        let app = build_users_router(build_state(db).await);

        let body = serde_json::to_vec(&json!({ "password": "super-secret-pw-123" })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/invitations/used-token/accept")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// Insert an existing user with a real argon2 password hash and return them.
    async fn seed_existing_password_user(
        db: &Arc<SqliteDatabase>,
        email: &str,
        plaintext_password: &str,
        role: Role,
    ) -> User {
        let hash = crate::auth::password::hash_password(plaintext_password).expect("hash");
        let user = User {
            id: format!("u-{}", Uuid::new_v4()),
            email: email.to_string(),
            password_hash: Some(hash),
            role,
            oauth_github_id: None,
            oauth_google_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: true,
            sign_in_epoch: 0,
        };
        db.create_user(&user).await.expect("create existing user");
        user
    }

    fn make_invitation(email: &str, token: &str, role: Role, org_id: &str) -> Invitation {
        Invitation {
            id: Uuid::new_v4().to_string(),
            email: email.to_string(),
            role,
            token: token.to_string(),
            invited_by: None,
            org_id: org_id.to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            used_at: None,
            created_at: chrono::Utc::now(),
        }
    }

    /// SECURITY (account-takeover): accepting an invitation for an email that
    /// already maps to an existing account must NOT mint a session for that
    /// account when the supplied password is wrong. A valid invitation token is
    /// not proof of control over a pre-existing account.
    #[tokio::test]
    async fn accept_invitation_existing_user_wrong_password_is_rejected() {
        let db = build_db().await;
        let org_id = crate::api::middleware::DEFAULT_ORG_ID.to_string();
        // Victim account already exists with a known password.
        let victim = seed_existing_password_user(
            &db,
            "victim@example.com",
            "victim-real-password-9000",
            Role::Owner,
        )
        .await;
        // An org admin invites the victim's email into some org as a member.
        let inv = make_invitation("victim@example.com", "takeover-token", Role::Member, &org_id);
        db.create_invitation(&inv).await.unwrap();
        let app = build_users_router(build_state(db.clone()).await);

        // Attacker accepts with a WRONG password.
        let body = serde_json::to_vec(&json!({ "password": "totally-wrong-password" })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/invitations/takeover-token/accept")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        // Must be rejected with 401 — no session for the victim.
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "wrong password must be rejected, not silently logged in as the existing user"
        );
        // No session cookie must be issued.
        assert!(
            resp.headers().get("set-cookie").is_none(),
            "no session/set-cookie may be issued on a failed existing-user accept"
        );
        // No membership may have been granted to the victim in the invited org.
        let membership = db.get_membership(&org_id, &victim.id).await.unwrap();
        assert!(
            membership.is_none(),
            "no membership may be added on a failed existing-user accept"
        );
        // Invitation must remain unused so it isn't burned by a failed attempt.
        let inv_after = db.get_invitation("takeover-token").await.unwrap().unwrap();
        assert!(
            inv_after.used_at.is_none(),
            "a rejected accept must not consume the invitation"
        );
    }

    /// Regression: an existing user accepting with the CORRECT password succeeds —
    /// a session is issued and the membership is added.
    #[tokio::test]
    async fn accept_invitation_existing_user_correct_password_succeeds() {
        let db = build_db().await;
        let org_id = crate::api::middleware::DEFAULT_ORG_ID.to_string();
        let user = seed_existing_password_user(
            &db,
            "existing@example.com",
            "existing-real-password-1",
            Role::Admin,
        )
        .await;
        let inv = make_invitation("existing@example.com", "good-token", Role::Member, &org_id);
        db.create_invitation(&inv).await.unwrap();
        let app = build_users_router(build_state(db.clone()).await);

        let body = serde_json::to_vec(&json!({ "password": "existing-real-password-1" })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/invitations/good-token/accept")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers().get("set-cookie").is_some(),
            "a correct-password accept must issue a session"
        );
        // Membership added in the invited org.
        let membership = db.get_membership(&org_id, &user.id).await.unwrap();
        assert!(membership.is_some(), "membership must be added on success");
        // Invitation consumed.
        let inv_after = db.get_invitation("good-token").await.unwrap().unwrap();
        assert!(inv_after.used_at.is_some(), "successful accept consumes the invitation");
    }

    /// Regression: an OAuth-only existing user (no password_hash) cannot be
    /// authenticated via the password path, so accept must be rejected.
    #[tokio::test]
    async fn accept_invitation_oauth_only_existing_user_is_rejected() {
        let db = build_db().await;
        let org_id = crate::api::middleware::DEFAULT_ORG_ID.to_string();
        // OAuth-only user: password_hash = None.
        let victim = User {
            id: format!("u-{}", Uuid::new_v4()),
            email: "oauth@example.com".to_string(),
            password_hash: None,
            role: Role::Owner,
            oauth_github_id: None,
            oauth_google_id: Some("google-123".to_string()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: true,
            sign_in_epoch: 0,
        };
        db.create_user(&victim).await.unwrap();
        let inv = make_invitation("oauth@example.com", "oauth-token", Role::Member, &org_id);
        db.create_invitation(&inv).await.unwrap();
        let app = build_users_router(build_state(db.clone()).await);

        let body = serde_json::to_vec(&json!({ "password": "any-password-at-all-1" })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/invitations/oauth-token/accept")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "OAuth-only users cannot be authenticated via the password path"
        );
        assert!(
            resp.headers().get("set-cookie").is_none(),
            "no session may be issued for an OAuth-only existing user"
        );
        let membership = db.get_membership(&org_id, &victim.id).await.unwrap();
        assert!(membership.is_none(), "no membership added for rejected OAuth-only accept");
    }

    /// Regression: a brand-new email (no existing account) still creates the
    /// account and issues a session.
    #[tokio::test]
    async fn accept_invitation_new_user_still_succeeds() {
        let db = build_db().await;
        let org_id = crate::api::middleware::DEFAULT_ORG_ID.to_string();
        let inv = make_invitation("brand-new@example.com", "newuser-token", Role::Member, &org_id);
        db.create_invitation(&inv).await.unwrap();
        let app = build_users_router(build_state(db.clone()).await);

        let body = serde_json::to_vec(&json!({ "password": "brand-new-password-123" })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/invitations/newuser-token/accept")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get("set-cookie").is_some(), "new user gets a session");
        let created = db.get_user_by_email("brand-new@example.com").await.unwrap();
        assert!(created.is_some(), "new account created");
        let created = created.unwrap();
        assert!(created.password_hash.is_some(), "new account has a password hash");
        let membership = db.get_membership(&org_id, &created.id).await.unwrap();
        assert!(membership.is_some(), "new user gains membership");
    }

    // ── PATCH /v1/users/{id} ──────────────────────────────────────────────────

    #[tokio::test]
    async fn patch_user_role_succeeds() {
        let db = build_db().await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let (member, _, _) = seed_user_with_session(&db, Role::Member).await;
        let app = build_users_router(build_state(db).await);

        let body = serde_json::to_vec(&json!({ "role": "viewer" })).unwrap();
        let req = Request::builder()
            .method("PATCH")
            .uri(format!("/v1/users/{}", member.id))
            .header("cookie", format!("litegen_session={}", admin_sess))
            .header("x-csrf-token", admin_csrf)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["role"], "viewer");
    }

    #[tokio::test]
    async fn patch_owner_role_returns_400() {
        let db = build_db().await;
        let (owner, _, _) = seed_user_with_session(&db, Role::Owner).await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let app = build_users_router(build_state(db).await);

        let body = serde_json::to_vec(&json!({ "role": "admin" })).unwrap();
        let req = Request::builder()
            .method("PATCH")
            .uri(format!("/v1/users/{}", owner.id))
            .header("cookie", format!("litegen_session={}", admin_sess))
            .header("x-csrf-token", admin_csrf)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── DELETE /v1/users/{id} ─────────────────────────────────────────────────

    #[tokio::test]
    async fn delete_user_soft_deactivates() {
        let db = build_db().await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let (member, _, _) = seed_user_with_session(&db, Role::Member).await;
        let app = build_users_router(build_state(db.clone()).await);

        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/v1/users/{}", member.id))
            .header("cookie", format!("litegen_session={}", admin_sess))
            .header("x-csrf-token", admin_csrf)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let updated = db.get_user_by_id(&member.id).await.unwrap().unwrap();
        assert!(!updated.is_active);
    }

    #[tokio::test]
    async fn delete_owner_returns_400() {
        let db = build_db().await;
        let (owner, _, _) = seed_user_with_session(&db, Role::Owner).await;
        let (_, admin_sess, admin_csrf) = seed_user_with_session(&db, Role::Admin).await;
        let app = build_users_router(build_state(db).await);

        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/v1/users/{}", owner.id))
            .header("cookie", format!("litegen_session={}", admin_sess))
            .header("x-csrf-token", admin_csrf)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── POST /v1/users/transfer-owner ─────────────────────────────────────────

    /// Review of security fix 3: Owner records that the old transfers left
    /// apart are found, so startup can name them, and one transfer repairs
    /// them.
    #[tokio::test]
    async fn split_owner_records_are_found() {
        let db = build_db().await;
        assert_eq!(split_owner_records(&*db).await.unwrap(), None, "no accounts yet");
        let (owner, _, _) = seed_user_with_session(&db, Role::Owner).await;
        let (admin, _, _) = seed_user_with_session(&db, Role::Admin).await;
        assert_eq!(split_owner_records(&*db).await.unwrap(), None, "one Owner, in both places");
        // What a Members-page transfer did before the fix.
        db.transfer_org_owner(DEFAULT_ORG_ID, &admin.id).await.unwrap();
        assert_eq!(
            split_owner_records(&*db).await.unwrap(),
            Some((vec![admin.id.clone()], vec![owner.id.clone()])),
            "(by membership, by account role)"
        );
        db.transfer_deployment_owner(DEFAULT_ORG_ID, &admin.id).await.unwrap();
        assert_eq!(split_owner_records(&*db).await.unwrap(), None, "one transfer repairs it");
        // Accounts, but no Owner in either record, needs the same transfer.
        db.update_member_role(DEFAULT_ORG_ID, &admin.id, Role::Admin).await.unwrap();
        db.update_user(&admin.id, Some(Role::Admin), None, None).await.unwrap();
        assert_eq!(split_owner_records(&*db).await.unwrap(), Some((vec![], vec![])), "no Owner at all");
    }

    #[tokio::test]
    async fn transfer_owner_changes_roles() {
        let db = build_db().await;
        let (owner, owner_sess, owner_csrf) = seed_user_with_session(&db, Role::Owner).await;
        let (admin, _, _) = seed_user_with_session(&db, Role::Admin).await;
        let app = build_users_router(build_state(db.clone()).await);

        let body = serde_json::to_vec(&json!({ "new_owner_id": admin.id })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/users/transfer-owner")
            .header("cookie", format!("litegen_session={}", owner_sess))
            .header("x-csrf-token", owner_csrf)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let new_owner = db.get_user_by_id(&admin.id).await.unwrap().unwrap();
        let old_owner = db.get_user_by_id(&owner.id).await.unwrap().unwrap();
        assert_eq!(new_owner.role, Role::Owner);
        assert_eq!(old_owner.role, Role::Admin);
    }

    #[tokio::test]
    async fn transfer_owner_to_nonexistent_returns_404() {
        let db = build_db().await;
        let (_, owner_sess, owner_csrf) = seed_user_with_session(&db, Role::Owner).await;
        let app = build_users_router(build_state(db).await);

        let body = serde_json::to_vec(&json!({ "new_owner_id": "nonexistent-id" })).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/users/transfer-owner")
            .header("cookie", format!("litegen_session={}", owner_sess))
            .header("x-csrf-token", owner_csrf)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
