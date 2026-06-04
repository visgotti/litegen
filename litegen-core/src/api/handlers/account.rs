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

use crate::api::middleware::{make_clear_session_cookies, AppState, KeyContext};
use crate::auth::permissions::Permission;
use crate::auth::password::{hash_password, verify_password, PasswordError};
use crate::types::Session;

// ─── Helpers ─────────────────────────────────────────────────────────────────

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

/// Public user view for account responses.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AccountUser {
    pub id: String,
    pub email: String,
    pub role: String,
    pub is_active: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

impl From<crate::types::User> for AccountUser {
    fn from(u: crate::types::User) -> Self {
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

/// Public session info (no csrf_token exposed).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: String,
    pub expires_at: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

impl From<Session> for SessionInfo {
    fn from(s: Session) -> Self {
        Self {
            id: s.id,
            created_at: s.created_at.to_rfc3339(),
            expires_at: s.expires_at.to_rfc3339(),
            ip: s.ip,
            user_agent: s.user_agent,
        }
    }
}

fn log_audit_account(
    db: Arc<dyn crate::db::DatabaseStore>,
    ctx: &KeyContext,
    action: &str,
    target_id: &str,
) {
    let actor_key_id = ctx.key_id.map(|id| id.to_string());
    let actor_label = ctx
        .user
        .as_ref()
        .map(|u| u.email.clone())
        .or_else(|| ctx.key_id.map(|id| id.to_string()))
        .unwrap_or_else(|| "master-key".to_string());
    let action = action.to_string();
    let target_id = target_id.to_string();
    let org_id = ctx.org_id.clone();
    tokio::spawn(async move {
        let entry = crate::types::AuditLogEntry {
            id: format!("audit-{}", Uuid::new_v4()),
            actor_key_id,
            actor_label,
            action,
            target_type: "user".to_string(),
            target_id,
            before_json: None,
            after_json: None,
            created_at: chrono::Utc::now(),
            org_id,
        };
        let _ = db.insert_audit_log(&entry).await;
    });
}

// ─── GET /v1/account ─────────────────────────────────────────────────────────

/// GET /v1/account — Get the current user's profile.
#[utoipa::path(
    get,
    path = "/v1/account",
    responses(
        (status = 200, description = "Current user profile", body = AccountUser),
        (status = 401, description = "Not authenticated", body = crate::types::ErrorResponse),
    ),
    tag = "Account"
)]
pub async fn get_account(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
) -> Response {
    if !ctx.permissions.contains(&Permission::UserReadSelf) {
        return forbidden_perm("user:read:self");
    }

    let user_id = match ctx.user.as_ref() {
        Some(u) => u.user_id.clone(),
        None => return err(StatusCode::UNAUTHORIZED, "not_authenticated", "Not authenticated"),
    };

    match state.db.get_user_by_id(&user_id).await {
        Ok(Some(user)) => (StatusCode::OK, Json(AccountUser::from(user))).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    }
}

// ─── PATCH /v1/account ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PatchAccountRequest {
    pub current_password: Option<String>,
    pub new_password: Option<String>,
}

/// PATCH /v1/account — Update the current user's account (e.g. change password).
#[utoipa::path(
    patch,
    path = "/v1/account",
    request_body = PatchAccountRequest,
    responses(
        (status = 200, description = "Updated profile", body = AccountUser),
        (status = 400, description = "Bad request (no changes or missing fields)", body = crate::types::ErrorResponse),
        (status = 401, description = "Not authenticated or wrong password", body = crate::types::ErrorResponse),
    ),
    tag = "Account"
)]
pub async fn patch_account(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Json(body): Json<PatchAccountRequest>,
) -> Response {
    if !ctx.permissions.contains(&Permission::UserReadSelf) {
        return forbidden_perm("user:read:self");
    }

    let user_id = match ctx.user.as_ref() {
        Some(u) => u.user_id.clone(),
        None => return err(StatusCode::UNAUTHORIZED, "not_authenticated", "Not authenticated"),
    };

    // Only password change is supported for now
    if body.new_password.is_none() {
        return err(StatusCode::BAD_REQUEST, "no_changes", "No changes requested");
    }

    // Require current_password when changing password
    let current_password = match body.current_password {
        Some(p) => p,
        None => {
            return err(
                StatusCode::BAD_REQUEST,
                "current_password_required",
                "current_password is required to change your password",
            );
        }
    };

    let user = match state.db.get_user_by_id(&user_id).await {
        Ok(Some(u)) => u,
        Ok(None) => return err(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    };

    // Verify current password
    let phc = match user.password_hash.as_deref() {
        Some(h) => h.to_string(),
        None => {
            return err(
                StatusCode::BAD_REQUEST,
                "no_password_set",
                "No password set on this account; use OAuth to sign in",
            );
        }
    };

    match verify_password(&current_password, &phc) {
        Ok(true) => {}
        Ok(false) => {
            return err(StatusCode::UNAUTHORIZED, "wrong_password", "Current password is incorrect");
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "verify_error", &e.to_string()),
    }

    let new_hash = match hash_password(body.new_password.as_deref().unwrap()) {
        Ok(h) => h,
        Err(PasswordError::TooShort) => {
            return err(
                StatusCode::BAD_REQUEST,
                "password_too_short",
                "Password must be at least 12 characters",
            );
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "hash_error", &e.to_string()),
    };

    match state.db.update_user(&user_id, None, None, Some(&new_hash)).await {
        Ok(Some(updated)) => {
            // Revoke all other sessions so a stolen session can't outlive a password change.
            // Pass the current session id as `except_id` so the caller stays logged in.
            let except_id = ctx.session_id.as_deref();
            let _ = state.db.delete_user_sessions(&user_id, except_id).await;
            tracing::info!(user_id = %user_id, "password changed — other sessions revoked");
            log_audit_account(state.db.clone(), &ctx, "account.change_password", &user_id);
            (StatusCode::OK, Json(AccountUser::from(updated))).into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, "user_not_found", "User not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    }
}

// ─── GET /v1/account/sessions ─────────────────────────────────────────────────

/// GET /v1/account/sessions — List the current user's active sessions.
#[utoipa::path(
    get,
    path = "/v1/account/sessions",
    responses(
        (status = 200, description = "List of active sessions", body = Vec<SessionInfo>),
        (status = 401, description = "Not authenticated", body = crate::types::ErrorResponse),
    ),
    tag = "Account"
)]
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
) -> Response {
    if !ctx.permissions.contains(&Permission::SessionRevokeOwn) {
        return forbidden_perm("session:revoke:own");
    }

    let user_id = match ctx.user.as_ref() {
        Some(u) => u.user_id.clone(),
        None => return err(StatusCode::UNAUTHORIZED, "not_authenticated", "Not authenticated"),
    };

    match state.db.list_user_sessions(&user_id).await {
        Ok(sessions) => {
            let infos: Vec<SessionInfo> = sessions.into_iter().map(SessionInfo::from).collect();
            (StatusCode::OK, Json(infos)).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    }
}

// ─── DELETE /v1/account/sessions/{id} ────────────────────────────────────────

/// DELETE /v1/account/sessions/{id} — Revoke one of the current user's sessions.
#[utoipa::path(
    delete,
    path = "/v1/account/sessions/{id}",
    params(("id" = String, Path, description = "Session ID to revoke")),
    responses(
        (status = 204, description = "Session revoked"),
        (status = 403, description = "Cannot revoke another user's session", body = crate::types::ErrorResponse),
        (status = 404, description = "Session not found", body = crate::types::ErrorResponse),
    ),
    tag = "Account"
)]
pub async fn revoke_session(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
) -> Response {
    if !ctx.permissions.contains(&Permission::SessionRevokeOwn) {
        return forbidden_perm("session:revoke:own");
    }

    let user_id = match ctx.user.as_ref() {
        Some(u) => u.user_id.clone(),
        None => return err(StatusCode::UNAUTHORIZED, "not_authenticated", "Not authenticated"),
    };

    let current_session_id = ctx.session_id.clone();

    // Verify the session belongs to this user
    let session = match state.db.get_session(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return err(StatusCode::NOT_FOUND, "session_not_found", "Session not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()),
    };

    if session.user_id != user_id {
        return err(
            StatusCode::FORBIDDEN,
            "forbidden",
            "You can only revoke your own sessions",
        );
    }

    if let Err(e) = state.db.delete_session(&id).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string());
    }

    // If revoking the current session, clear cookies
    if current_session_id.as_deref() == Some(&id) {
        let (sc, cc) = make_clear_session_cookies();
        return (
            StatusCode::NO_CONTENT,
            [
                ("set-cookie", sc.to_str().unwrap_or("").to_string()),
                ("set-cookie", cc.to_str().unwrap_or("").to_string()),
            ],
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::{delete, get},
        Router,
    };
    use tower::ServiceExt;

    use crate::api::middleware::{auth_middleware, AppState};
    use crate::auth::password::hash_password;
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

    async fn seed_user_with_session(
        db: &Arc<SqliteDatabase>,
        role: Role,
        email: &str,
        with_password: Option<&str>,
    ) -> (User, String, String) {
        let password_hash = with_password.map(|pw| hash_password(pw).expect("hash"));
        let user = User {
            id: format!("u-{}", Uuid::new_v4()),
            email: email.to_string(),
            password_hash,
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

    fn build_account_router(state: Arc<AppState>) -> Router {
        let auth_state = state.clone();
        Router::new()
            .route("/v1/account", get(get_account).patch(patch_account))
            .route("/v1/account/sessions", get(list_sessions))
            .route("/v1/account/sessions/{id}", delete(revoke_session))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                let s = auth_state.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state)
    }

    // ── GET /v1/account ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_account_returns_current_user() {
        let db = build_db().await;
        let (user, session_tok, _) = seed_user_with_session(&db, Role::Member, "me@test.com", None).await;
        let app = build_account_router(build_state(db).await);

        let req = Request::builder()
            .uri("/v1/account")
            .header("cookie", format!("litegen_session={}", session_tok))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["email"], user.email);
        assert_eq!(json["id"], user.id);
    }

    // ── PATCH /v1/account ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn patch_account_changes_password_when_current_matches() {
        let db = build_db().await;
        let (user, session_tok, csrf_tok) =
            seed_user_with_session(&db, Role::Member, "pw@test.com", Some("correct-horse-battery-1")).await;
        let app = build_account_router(build_state(db.clone()).await);

        let body = serde_json::to_vec(&serde_json::json!({
            "current_password": "correct-horse-battery-1",
            "new_password": "new-super-secure-pw-99"
        }))
        .unwrap();
        let req = Request::builder()
            .method("PATCH")
            .uri("/v1/account")
            .header("cookie", format!("litegen_session={}", session_tok))
            .header("x-csrf-token", csrf_tok)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify hash changed
        let updated = db.get_user_by_id(&user.id).await.unwrap().unwrap();
        assert!(updated.password_hash.is_some());
        let new_hash = updated.password_hash.unwrap();
        assert!(crate::auth::password::verify_password("new-super-secure-pw-99", &new_hash).unwrap());
    }

    #[tokio::test]
    async fn patch_account_with_wrong_current_password_returns_401() {
        let db = build_db().await;
        let (_, session_tok, csrf_tok) =
            seed_user_with_session(&db, Role::Member, "wrong@test.com", Some("correct-horse-battery-1")).await;
        let app = build_account_router(build_state(db).await);

        let body = serde_json::to_vec(&serde_json::json!({
            "current_password": "wrong-password-here-1",
            "new_password": "new-super-secure-pw-99"
        }))
        .unwrap();
        let req = Request::builder()
            .method("PATCH")
            .uri("/v1/account")
            .header("cookie", format!("litegen_session={}", session_tok))
            .header("x-csrf-token", csrf_tok)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── GET /v1/account/sessions ──────────────────────────────────────────────

    #[tokio::test]
    async fn list_account_sessions_returns_own_only() {
        let db = build_db().await;
        let (user, sess1, _) = seed_user_with_session(&db, Role::Member, "sess@test.com", None).await;
        // Create a second session for the same user
        let sess2 = generate_session_token();
        let s2 = Session {
            id: sess2.clone(),
            user_id: user.id.clone(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            ip: None,
            user_agent: None,
            csrf_token: generate_csrf_token(),
        };
        db.create_session(&s2).await.unwrap();

        // Create another user + session (should not appear)
        seed_user_with_session(&db, Role::Viewer, "other@test.com", None).await;

        let app = build_account_router(build_state(db).await);

        let req = Request::builder()
            .uri("/v1/account/sessions")
            .header("cookie", format!("litegen_session={}", sess1))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json.is_array());
        // Only this user's sessions (2 sessions)
        assert_eq!(json.as_array().unwrap().len(), 2);
    }

    // ── DELETE /v1/account/sessions/{id} ──────────────────────────────────────

    #[tokio::test]
    async fn delete_account_session_succeeds_for_own() {
        let db = build_db().await;
        let (user, sess1, csrf1) = seed_user_with_session(&db, Role::Member, "del@test.com", None).await;
        // Create a second session to delete
        let sess2 = generate_session_token();
        let s2 = Session {
            id: sess2.clone(),
            user_id: user.id.clone(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            ip: None,
            user_agent: None,
            csrf_token: generate_csrf_token(),
        };
        db.create_session(&s2).await.unwrap();

        let app = build_account_router(build_state(db.clone()).await);

        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/v1/account/sessions/{}", sess2))
            .header("cookie", format!("litegen_session={}", sess1))
            .header("x-csrf-token", csrf1)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Session deleted
        let s = db.get_session(&sess2).await.unwrap();
        assert!(s.is_none());
    }

    #[tokio::test]
    async fn delete_account_session_for_other_user_returns_403() {
        let db = build_db().await;
        let (_, sess1, csrf1) = seed_user_with_session(&db, Role::Member, "user1@test.com", None).await;
        let (_, sess2, _) = seed_user_with_session(&db, Role::Member, "user2@test.com", None).await;

        let app = build_account_router(build_state(db).await);

        // user1 tries to delete user2's session
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/v1/account/sessions/{}", sess2))
            .header("cookie", format!("litegen_session={}", sess1))
            .header("x-csrf-token", csrf1)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // ── P1: Password change revokes other sessions ─────────────────────────────

    /// Build a router that includes both account and auth (me) routes,
    /// so we can verify the second session is invalidated end-to-end.
    fn build_combined_router(state: Arc<AppState>) -> Router {
        use crate::api::handlers::auth_password::me;
        use axum::routing::get as axum_get;
        use crate::api::middleware::csrf_middleware;
        let auth_state = state.clone();
        Router::new()
            .route("/v1/account", get(get_account).patch(patch_account))
            .route("/v1/account/sessions", get(list_sessions))
            .route("/v1/account/sessions/{id}", delete(revoke_session))
            .route("/v1/auth/me", axum_get(me))
            .layer(middleware::from_fn_with_state(state.clone(), csrf_middleware))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                let s = auth_state.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state)
    }

    #[tokio::test]
    async fn changing_password_revokes_other_sessions_but_keeps_current() {
        let db = build_db().await;
        // Create user with password
        let (user, sess_a, csrf_a) =
            seed_user_with_session(&db, Role::Member, "pwchange@test.com", Some("old-strong-password-12")).await;

        // Create a second session (sess_b) for the same user
        let sess_b = generate_session_token();
        let csrf_b = generate_csrf_token();
        let sess_b_record = Session {
            id: sess_b.clone(),
            user_id: user.id.clone(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            ip: None,
            user_agent: None,
            csrf_token: csrf_b.clone(),
        };
        db.create_session(&sess_b_record).await.expect("create sess_b");

        let app = build_combined_router(build_state(db.clone()).await);

        // Change password using sess_a
        let body = serde_json::to_vec(&serde_json::json!({
            "current_password": "old-strong-password-12",
            "new_password": "new-strong-password-12"
        }))
        .unwrap();
        let req = Request::builder()
            .method("PATCH")
            .uri("/v1/account")
            .header("cookie", format!("litegen_session={}", sess_a))
            .header("x-csrf-token", &csrf_a)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Password change should return 200");

        // sess_a (current session) must still be alive
        let me_req_a = Request::builder()
            .method("GET")
            .uri("/v1/auth/me")
            .header("cookie", format!("litegen_session={}", sess_a))
            .body(Body::empty())
            .unwrap();
        let resp_a = app.clone().oneshot(me_req_a).await.unwrap();
        assert_eq!(resp_a.status(), StatusCode::OK, "Current session (A) must remain valid after password change");

        // sess_b (other session) must be revoked
        let me_req_b = Request::builder()
            .method("GET")
            .uri("/v1/auth/me")
            .header("cookie", format!("litegen_session={}", sess_b))
            .body(Body::empty())
            .unwrap();
        let resp_b = app.oneshot(me_req_b).await.unwrap();
        assert_eq!(resp_b.status(), StatusCode::UNAUTHORIZED, "Other session (B) must be revoked after password change");
    }
}
