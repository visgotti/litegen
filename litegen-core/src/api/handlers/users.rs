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

use crate::api::middleware::{AppState, KeyContext};
use crate::auth::permissions::Permission;
use crate::auth::tokens::generate_session_token;
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

fn forbidden_perm(perm: &str) -> Response {
    err(
        StatusCode::FORBIDDEN,
        "forbidden_permission",
        &format!("Permission '{}' required", perm),
    )
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

/// Spawn a fire-and-forget audit log insert.
fn log_audit_user(
    db: Arc<dyn crate::db::DatabaseStore>,
    ctx: &KeyContext,
    action: &str,
    target_type: &str,
    target_id: &str,
    after: Option<serde_json::Value>,
) {
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
    let org_id = ctx.org_id.clone();
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
    if !ctx.permissions.contains(&Permission::UserReadAny) {
        return forbidden_perm("user:read:any");
    }
    match state.db.list_users().await {
        Ok(users) => {
            let public: Vec<PublicUser> = users.into_iter().map(PublicUser::from).collect();
            (StatusCode::OK, Json(public)).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    }
}

// ─── POST /v1/users ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InviteRequest {
    pub email: String,
    pub role: Role,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InviteResponse {
    pub id: String,
    pub email: String,
    pub role: String,
    pub expires_at: String,
}

/// POST /v1/users — Invite a user (requires invitation:send).
#[utoipa::path(
    post,
    path = "/v1/users",
    request_body = InviteRequest,
    responses(
        (status = 200, description = "Invitation created", body = InviteResponse),
        (status = 400, description = "Bad request (e.g. owner role not allowed)", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
    ),
    tag = "Users"
)]
pub async fn invite_user(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Json(body): Json<InviteRequest>,
) -> Response {
    if !ctx.permissions.contains(&Permission::InvitationSend) {
        return forbidden_perm("invitation:send");
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
    // Scope the invitation to the caller's active org so the invitee joins it
    // on accept. Falls back to the single-tenant default org.
    let org_id = ctx
        .org_id
        .clone()
        .unwrap_or_else(|| crate::api::middleware::DEFAULT_ORG_ID.to_string());
    let inv = Invitation {
        id: Uuid::new_v4().to_string(),
        email: email.clone(),
        role: body.role,
        token: token.clone(),
        invited_by: ctx.user.as_ref().map(|u| u.user_id.clone()),
        org_id,
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        used_at: None,
        created_at: chrono::Utc::now(),
    };

    if let Err(e) = state.db.create_invitation(&inv).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string());
    }

    log_audit_user(
        state.db.clone(),
        &ctx,
        "invitation.create",
        "invitation",
        &inv.id,
        Some(json!({ "email": email, "role": inv.role.as_str() })),
    );

    if state.dev.expose_invite_tokens {
        return (
            StatusCode::OK,
            Json(json!({
                "id": inv.id,
                "email": email,
                "role": inv.role.as_str(),
                "expires_at": inv.expires_at.to_rfc3339(),
                "_dev_token": token,
            })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "id": inv.id,
            "email": email,
            "role": inv.role.as_str(),
            "expires_at": inv.expires_at.to_rfc3339(),
        })),
    )
        .into_response()
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
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
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
        (status = 400, description = "Invalid, expired, or already used", body = crate::types::ErrorResponse),
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
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
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

    // Find or create the invited user. If they already have an account they
    // simply gain membership in the invited org. Password hashing is deferred
    // to the new-user branch so an existing invitee doesn't pay the hash cost.
    let user = match state.db.get_user_by_email(&inv.email).await {
        Ok(Some(existing)) => {
            // SECURITY: a valid invitation token is NOT proof of control over a
            // pre-existing account. The invited email is chosen freely by an org
            // admin, so without authenticating the caller as the existing user we
            // would let anyone holding the token mint a session AS that account
            // (account takeover / privilege escalation). Require the existing
            // account's password — the same check `login` performs.
            match existing.password_hash.as_deref() {
                Some(hash) => {
                    if !crate::auth::password::verify_password(&password, hash).unwrap_or(false) {
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
            existing
        }
        Ok(None) => {
            // Hash password only when a new account must be created.
            let hash = match crate::auth::password::hash_password(&password) {
                Ok(h) => h,
                Err(crate::auth::password::PasswordError::TooShort) => {
                    return err(
                        StatusCode::BAD_REQUEST,
                        "password_too_short",
                        "Password must be at least 12 characters",
                    );
                }
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "hash_error", &e.to_string()),
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
            };
            if let Err(e) = state.db.create_user(&user).await {
                return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string());
            }
            user
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    };

    // Membership is added before the atomic invite-consume below. If we then lose
    // the single-use race (Ok(false)), this membership row persists — that's
    // intentional and benign: both racers held a valid token, so the membership is
    // correct data; only the loser's response omits a session. We deliberately do
    // NOT consume the invite before add_org_member, so a transient add_org_member
    // failure (500) leaves the invite unconsumed and retryable rather than burned.
    // Join the invited org with the invited role. Ignore a duplicate-membership
    // error so re-accepting is idempotent.
    if state.db.get_membership(&inv.org_id, &user.id).await.ok().flatten().is_none() {
        if let Err(e) = state.db.add_org_member(&inv.org_id, &user.id, inv.role).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string());
        }
    }

    // Atomically consume the invitation. If a concurrent request already used it,
    // abort before minting a session (defense-in-depth on top of the used_at check).
    match state.db.mark_invitation_used(&token).await {
        Ok(true) => {}
        Ok(false) => {
            return err(
                StatusCode::BAD_REQUEST,
                "invitation_already_used",
                "This invitation has already been used",
            )
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    }

    // Create session
    let (_, _, session_cookie, csrf_cookie) =
        match crate::api::middleware::create_session_cookies(&state.db, &user.id, None, None).await {
            Ok(v) => v,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "session_error", &e.to_string()),
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

/// PATCH /v1/users/{id} — Update user role or active status (requires user:write:any).
#[utoipa::path(
    patch,
    path = "/v1/users/{id}",
    params(("id" = String, Path, description = "User ID")),
    request_body = PatchUserRequest,
    responses(
        (status = 200, description = "Updated user", body = PublicUser),
        (status = 400, description = "Bad request", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
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
    if !ctx.permissions.contains(&Permission::UserWriteAny) {
        return forbidden_perm("user:write:any");
    }

    // Load target user
    let target = match state.db.get_user_by_id(&id).await {
        Ok(Some(u)) => u,
        Ok(None) => return err(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    };

    // Block changing Owner's role via this endpoint
    if target.role == Role::Owner {
        if body.role.is_some() && body.role != Some(Role::Owner) {
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
                "Cannot deactivate the Owner account. Transfer ownership first.",
            );
        }
    }

    match state.db.update_user(&id, body.role, body.is_active, None).await {
        Ok(Some(updated)) => {
            log_audit_user(
                state.db.clone(),
                &ctx,
                "user.update",
                "user",
                &id,
                Some(json!({ "role": updated.role.as_str(), "is_active": updated.is_active })),
            );
            (StatusCode::OK, Json(PublicUser::from(updated))).into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    }
}

// ─── DELETE /v1/users/{id} ────────────────────────────────────────────────────

/// DELETE /v1/users/{id} — Soft-delete (deactivate) a user (requires user:delete:any).
#[utoipa::path(
    delete,
    path = "/v1/users/{id}",
    params(("id" = String, Path, description = "User ID")),
    responses(
        (status = 204, description = "User deactivated"),
        (status = 400, description = "Cannot delete owner", body = crate::types::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::types::ErrorResponse),
        (status = 404, description = "User not found", body = crate::types::ErrorResponse),
    ),
    tag = "Users"
)]
pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
) -> Response {
    if !ctx.permissions.contains(&Permission::UserDeleteAny) {
        return forbidden_perm("user:delete:any");
    }

    // Load target user
    let target = match state.db.get_user_by_id(&id).await {
        Ok(Some(u)) => u,
        Ok(None) => return err(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    };

    if target.role == Role::Owner {
        return err(
            StatusCode::BAD_REQUEST,
            "cannot_delete_owner",
            "Cannot delete the Owner account. Transfer ownership first.",
        );
    }

    match state.db.update_user(&id, None, Some(false), None).await {
        Ok(_) => {
            log_audit_user(
                state.db.clone(),
                &ctx,
                "user.delete",
                "user",
                &id,
                None,
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
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
    if !ctx.permissions.contains(&Permission::SystemTransferOwner) {
        return forbidden_perm("system:transfer_owner");
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
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
        Ok(Some(_)) => {}
    }

    match state.db.transfer_owner(&body.new_owner_id).await {
        Ok(()) => {
            log_audit_user(
                state.db.clone(),
                &ctx,
                "system.transfer_owner",
                "user",
                &body.new_owner_id,
                None,
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    }
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
        };
        db.create_user(&user).await.expect("create user");

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
