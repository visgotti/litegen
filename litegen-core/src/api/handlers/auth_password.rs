use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::api::middleware::{
    create_session_cookies, make_clear_session_cookies, AppState, KeyContext,
};
use crate::auth::lockout::{is_locked_out, retry_after_seconds};
use crate::auth::password::{hash_password, verify_dummy, verify_password, PasswordError};
use crate::types::{PasswordReset, Role, User};

// ─── Shared helpers ──────────────────────────────────────────────────────────

fn error_resp(code: StatusCode, error_code: &str, message: &str) -> Response {
    (
        code,
        Json(json!({
            "error": {
                "code": error_code,
                "message": message,
                "type": "auth_error"
            }
        })),
    )
        .into_response()
}

/// Public view of a user (no password_hash).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PublicUser {
    pub id: String,
    pub email: String,
    pub role: String,
    pub created_at: String,
    pub last_login_at: Option<String>,
    pub is_active: bool,
}

impl From<User> for PublicUser {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            email: u.email,
            role: u.role.as_str().to_string(),
            created_at: u.created_at.to_rfc3339(),
            last_login_at: u.last_login_at.map(|t| t.to_rfc3339()),
            is_active: u.is_active,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuthResponse {
    pub user: PublicUser,
}

// ─── POST /v1/auth/signup ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SignupRequest {
    pub email: String,
    pub password: String,
}

/// POST /v1/auth/signup — Create the first user (owner).
#[utoipa::path(
    post,
    path = "/v1/auth/signup",
    request_body = SignupRequest,
    responses(
        (status = 200, description = "Signed up and session created", body = AuthResponse),
        (status = 400, description = "Bad request (short password)", body = crate::types::ErrorResponse),
        (status = 403, description = "Owner email mismatch", body = crate::types::ErrorResponse),
        (status = 409, description = "Signup closed (users already exist)", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn signup(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SignupRequest>,
) -> Response {
    let email = body.email.trim().to_lowercase();

    // Only allow when users table is empty
    match state.db.count_users().await {
        Ok(count) if count > 0 => {
            return error_resp(StatusCode::CONFLICT, "signup_closed", "Signup is closed: users already exist");
        }
        Err(e) => {
            return error_resp(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string());
        }
        _ => {}
    }

    // If LITEGEN__OWNER_EMAIL is set, only that email can sign up
    if let Ok(required) = std::env::var("LITEGEN__OWNER_EMAIL") {
        if required.trim().to_lowercase() != email {
            return error_resp(
                StatusCode::FORBIDDEN,
                "owner_email_required",
                &format!("Only {} can claim the owner account", required),
            );
        }
    }

    let hash = match hash_password(&body.password) {
        Ok(h) => h,
        Err(PasswordError::TooShort) => {
            return error_resp(StatusCode::BAD_REQUEST, "password_too_short", "Password must be at least 12 characters");
        }
        Err(e) => {
            return error_resp(StatusCode::INTERNAL_SERVER_ERROR, "hash_error", &e.to_string());
        }
    };

    let now = chrono::Utc::now();
    let user = User {
        id: format!("user-{}", uuid::Uuid::new_v4()),
        email: email.clone(),
        password_hash: Some(hash),
        role: Role::Owner,
        oauth_github_id: None,
        oauth_google_id: None,
        created_at: now,
        updated_at: now,
        last_login_at: None,
        is_active: true,
    };

    if let Err(e) = state.db.create_user(&user).await {
        return error_resp(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string());
    }

    match create_session_cookies(&state.db, &user.id, None, None).await {
        Ok((_st, _ct, sc, cc)) => {
            let mut resp = (
                StatusCode::OK,
                Json(AuthResponse { user: user.into() }),
            )
                .into_response();
            resp.headers_mut().append("set-cookie", sc);
            resp.headers_mut().append("set-cookie", cc);
            resp
        }
        Err(e) => error_resp(StatusCode::INTERNAL_SERVER_ERROR, "session_error", &e.to_string()),
    }
}

// ─── POST /v1/auth/login ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// POST /v1/auth/login — Log in with email and password.
#[utoipa::path(
    post,
    path = "/v1/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Logged in, session cookie set", body = AuthResponse),
        (status = 401, description = "Invalid credentials", body = crate::types::ErrorResponse),
        (status = 429, description = "Too many failed attempts (locked out)", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> Response {
    let email = body.email.trim().to_lowercase();

    // Lockout check
    let since = chrono::Utc::now() - chrono::Duration::minutes(15);
    let fails = state
        .db
        .recent_failed_login_attempts(&email, since)
        .await
        .unwrap_or_default();

    if is_locked_out(&fails, chrono::Utc::now()) {
        let ra = retry_after_seconds(&fails, chrono::Utc::now());
        let mut resp = error_resp(
            StatusCode::TOO_MANY_REQUESTS,
            "locked_out",
            "Too many failed login attempts. Try again later.",
        );
        if let Ok(v) = axum::http::HeaderValue::from_str(&ra.to_string()) {
            resp.headers_mut().insert("retry-after", v);
        }
        return resp;
    }

    // Look up user (always run dummy verify for constant-time even when not found)
    let user_opt = state.db.get_user_by_email(&email).await.unwrap_or(None);
    let phc = user_opt.as_ref().and_then(|u| u.password_hash.clone());

    let verified = match phc {
        Some(ref h) => verify_password(&body.password, h).unwrap_or(false),
        None => {
            verify_dummy(&body.password);
            false
        }
    };

    if !verified || user_opt.is_none() {
        let _ = state.db.record_login_attempt(&email, false).await;
        return error_resp(StatusCode::UNAUTHORIZED, "invalid_credentials", "Invalid email or password");
    }

    let user = user_opt.unwrap();

    // Treat inactive accounts identically to bad credentials to prevent user enumeration.
    // Still record the attempt so lockout-based enumeration is also blocked.
    if !user.is_active {
        let _ = state.db.record_login_attempt(&email, false).await;
        return error_resp(StatusCode::UNAUTHORIZED, "invalid_credentials", "Invalid email or password");
    }

    let _ = state.db.record_login_attempt(&email, true).await;
    let _ = state.db.touch_last_login(&user.id).await;

    match create_session_cookies(&state.db, &user.id, None, None).await {
        Ok((_st, _ct, sc, cc)) => {
            let mut resp = (
                StatusCode::OK,
                Json(AuthResponse { user: user.into() }),
            )
                .into_response();
            resp.headers_mut().append("set-cookie", sc);
            resp.headers_mut().append("set-cookie", cc);
            resp
        }
        Err(e) => error_resp(StatusCode::INTERNAL_SERVER_ERROR, "session_error", &e.to_string()),
    }
}

// ─── POST /v1/auth/logout ─────────────────────────────────────────────────────

/// POST /v1/auth/logout — Destroy the current session.
#[utoipa::path(
    post,
    path = "/v1/auth/logout",
    responses(
        (status = 204, description = "Logged out, session cookie cleared"),
        (status = 401, description = "Not authenticated", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn logout(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
) -> Response {
    if let Some(sid) = ctx.session_id {
        let _ = state.db.delete_session(&sid).await;
    }
    let (sc, cc) = make_clear_session_cookies();
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut().append("set-cookie", sc);
    resp.headers_mut().append("set-cookie", cc);
    resp
}

// ─── GET /v1/auth/me ──────────────────────────────────────────────────────────

/// GET /v1/auth/me — Return the currently authenticated user.
#[utoipa::path(
    get,
    path = "/v1/auth/me",
    responses(
        (status = 200, description = "Current user info"),
        (status = 401, description = "Not authenticated", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn me(Extension(ctx): Extension<KeyContext>) -> Response {
    match ctx.user {
        Some(u) => (
            StatusCode::OK,
            Json(json!({
                "user": {
                    "id": u.user_id,
                    "email": u.email,
                    "role": u.role.as_str()
                }
            })),
        )
            .into_response(),
        None => error_resp(StatusCode::UNAUTHORIZED, "not_authenticated", "Not logged in"),
    }
}

// ─── GET /v1/auth/csrf ────────────────────────────────────────────────────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CsrfResponse {
    pub csrf_token: String,
}

/// GET /v1/auth/csrf — Return the CSRF token for the current session.
#[utoipa::path(
    get,
    path = "/v1/auth/csrf",
    responses(
        (status = 200, description = "CSRF token", body = CsrfResponse),
        (status = 401, description = "Not authenticated", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn csrf_token(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
) -> Response {
    let Some(sid) = ctx.session_id else {
        return error_resp(StatusCode::UNAUTHORIZED, "not_authenticated", "Not logged in");
    };
    match state.db.get_session(&sid).await {
        Ok(Some(sess)) => (
            StatusCode::OK,
            Json(json!({ "csrf_token": sess.csrf_token })),
        )
            .into_response(),
        _ => error_resp(StatusCode::UNAUTHORIZED, "session_not_found", "Session not found"),
    }
}

// ─── POST /v1/auth/password-reset/request ────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PasswordResetRequestBody {
    pub email: String,
}

/// POST /v1/auth/password-reset/request — Request a password reset email.
#[utoipa::path(
    post,
    path = "/v1/auth/password-reset/request",
    request_body = PasswordResetRequestBody,
    responses(
        (status = 200, description = "Request received (no enumeration; always 200)"),
    ),
    tag = "Auth"
)]
pub async fn password_reset_request(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PasswordResetRequestBody>,
) -> Response {
    let email = body.email.trim().to_lowercase();

    if let Ok(Some(user)) = state.db.get_user_by_email(&email).await {
        use crate::auth::tokens::generate_session_token;
        let token = generate_session_token();
        let reset = PasswordReset {
            token: token.clone(),
            user_id: user.id,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            used_at: None,
            created_at: chrono::Utc::now(),
        };
        let _ = state.db.create_password_reset(&reset).await;
        tracing::info!(email = %email, "password reset requested — token issued (not logged for security)");

        if std::env::var("LITEGEN__DEV__EXPOSE_RESET_TOKENS").as_deref() == Ok("true") {
            return (
                StatusCode::OK,
                Json(json!({ "ok": true, "_dev_token": token })),
            )
                .into_response();
        }
    }

    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

// ─── POST /v1/auth/password-reset/confirm ────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PasswordResetConfirmBody {
    pub token: String,
    pub new_password: String,
}

/// POST /v1/auth/password-reset/confirm — Confirm a password reset with the token.
#[utoipa::path(
    post,
    path = "/v1/auth/password-reset/confirm",
    request_body = PasswordResetConfirmBody,
    responses(
        (status = 204, description = "Password updated, all sessions revoked"),
        (status = 400, description = "Invalid or expired token", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn password_reset_confirm(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PasswordResetConfirmBody>,
) -> Response {
    let reset = match state.db.get_password_reset(&body.token).await {
        Ok(Some(r)) => r,
        _ => return error_resp(StatusCode::BAD_REQUEST, "invalid_token", "Token not found or invalid"),
    };

    if reset.used_at.is_some() || reset.expires_at < chrono::Utc::now() {
        return error_resp(StatusCode::BAD_REQUEST, "token_expired", "Token already used or expired");
    }

    let hash = match hash_password(&body.new_password) {
        Ok(h) => h,
        Err(PasswordError::TooShort) => {
            return error_resp(StatusCode::BAD_REQUEST, "password_too_short", "Password must be at least 12 characters");
        }
        Err(e) => {
            return error_resp(StatusCode::INTERNAL_SERVER_ERROR, "hash_error", &e.to_string());
        }
    };

    let _ = state.db.update_user(&reset.user_id, None, None, Some(&hash)).await;
    let _ = state.db.mark_password_reset_used(&body.token).await;
    let _ = state.db.delete_user_sessions(&reset.user_id, None).await;

    StatusCode::NO_CONTENT.into_response()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::Request,
        middleware,
        routing::{get, post},
        Router,
    };
    use tower::ServiceExt;

    use crate::api::middleware::{auth_middleware, csrf_middleware};
    use once_cell::sync::Lazy;
    use std::sync::Mutex;
    use crate::capabilities::CapabilityRegistry;

    /// Mutex to serialize tests that mutate environment variables.
    static ENV_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::db::sqlite::SqliteDatabase;
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::materializer::{Materializer, MaterializeError, TempStorage};
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::LocalStore;
    use bytes::Bytes;

    struct NoopStorage;
    #[async_trait::async_trait]
    impl TempStorage for NoopStorage {
        async fn put(&self, key: &str, _bytes: Bytes, _ct: &str) -> Result<String, MaterializeError> {
            Ok(format!("local://{}", key))
        }
        async fn delete(&self, _key: &str) -> Result<(), MaterializeError> { Ok(()) }
    }

    async fn build_state() -> Arc<AppState> {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
        build_state_with_db(db).await
    }

    async fn build_state_with_db(db: Arc<SqliteDatabase>) -> Arc<AppState> {
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
            master_key: None, // no master key → auth allows all (for unauth routes)
            registry: cap_registry,
            materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
        })
    }

    fn build_auth_router(state: Arc<AppState>) -> Router {
        let auth_state = state.clone();
        Router::new()
            .route("/v1/auth/signup", post(signup))
            .route("/v1/auth/login", post(login))
            .route("/v1/auth/logout", post(logout))
            .route("/v1/auth/me", get(me))
            .route("/v1/auth/csrf", get(csrf_token))
            .route("/v1/auth/password-reset/request", post(password_reset_request))
            .route("/v1/auth/password-reset/confirm", post(password_reset_confirm))
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

    fn json_body(v: serde_json::Value) -> Body {
        Body::from(serde_json::to_vec(&v).unwrap())
    }

    // ── Signup ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn signup_creates_owner_when_users_empty() {
        let state = build_state().await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "owner@test.com", "password": "strongpassword123" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Signup should return 200");

        // Check set-cookie headers
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect();
        assert!(cookies.iter().any(|c| c.contains("litegen_session=")), "Should set session cookie");
        assert!(cookies.iter().any(|c| c.contains("litegen_csrf=")), "Should set CSRF cookie");
    }

    #[tokio::test]
    async fn signup_fails_when_users_exist() {
        let state = build_state().await;
        // Pre-create a user
        let user = User {
            id: "existing".to_string(),
            email: "existing@test.com".to_string(),
            password_hash: None,
            role: Role::Owner,
            oauth_github_id: None,
            oauth_google_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: true,
        };
        state.db.create_user(&user).await.unwrap();

        let app = build_auth_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "new@test.com", "password": "strongpassword123" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT, "Signup should return 409 when users exist");
    }

    #[tokio::test]
    async fn signup_requires_owner_email_when_set() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("LITEGEN__OWNER_EMAIL", "allowed@test.com");
        let state = build_state().await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "notallowed@test.com", "password": "strongpassword123" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        std::env::remove_var("LITEGEN__OWNER_EMAIL");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "Signup should return 403 for non-owner email");
    }

    #[tokio::test]
    async fn signup_with_short_password_returns_400() {
        let state = build_state().await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "test@test.com", "password": "short" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── Login ─────────────────────────────────────────────────────────────────

    async fn create_user_with_password(state: &Arc<AppState>, email: &str, password: &str) {
        use crate::auth::password::hash_password;
        let hash = hash_password(password).unwrap();
        let user = User {
            id: format!("user-{}", uuid::Uuid::new_v4()),
            email: email.to_string(),
            password_hash: Some(hash),
            role: Role::Member,
            oauth_github_id: None,
            oauth_google_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: true,
        };
        state.db.create_user(&user).await.unwrap();
    }

    #[tokio::test]
    async fn login_with_correct_password_returns_session() {
        let state = build_state().await;
        create_user_with_password(&state, "user@test.com", "correctpassword123").await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "user@test.com", "password": "correctpassword123" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Login should return 200");

        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect();
        assert!(cookies.iter().any(|c| c.contains("litegen_session=")), "Should set session cookie");
    }

    #[tokio::test]
    async fn login_with_wrong_password_returns_401() {
        let state = build_state().await;
        create_user_with_password(&state, "user2@test.com", "correctpassword123").await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "user2@test.com", "password": "wrongpassword123" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_with_unknown_email_returns_401() {
        let state = build_state().await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "nobody@test.com", "password": "somepassword123" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "Unknown email should return 401");
    }

    #[tokio::test]
    async fn login_locked_out_after_5_fails() {
        let state = build_state().await;
        create_user_with_password(&state, "lockme@test.com", "correctpassword123").await;
        let app = build_auth_router(state.clone());

        // Record 5 failed attempts directly
        for _ in 0..5 {
            state.db.record_login_attempt("lockme@test.com", false).await.unwrap();
        }

        // Now attempt login
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "lockme@test.com", "password": "correctpassword123" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS, "Should be locked out after 5 fails");
        assert!(resp.headers().contains_key("retry-after"), "Should have Retry-After header");
    }

    // ── Logout ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn logout_deletes_session_and_clears_cookies() {
        let state = build_state().await;
        create_user_with_password(&state, "logout@test.com", "password12345678").await;
        let app = build_auth_router(state.clone());

        // Login first
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "logout@test.com", "password": "password12345678" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Extract session token from cookie
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect();
        let session_cookie = cookies.iter()
            .find(|c| c.starts_with("litegen_session="))
            .unwrap();
        let session_token = session_cookie
            .split('=').nth(1).unwrap()
            .split(';').next().unwrap();
        let csrf_cookie = cookies.iter()
            .find(|c| c.starts_with("litegen_csrf="))
            .unwrap();
        let csrf_token = csrf_cookie
            .split('=').nth(1).unwrap()
            .split(';').next().unwrap();

        // Verify session exists
        let sess = state.db.get_session(session_token).await.unwrap();
        assert!(sess.is_some(), "Session should exist after login");

        // Logout
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/logout")
            .header("cookie", format!("litegen_session={}", session_token))
            .header("x-csrf-token", csrf_token)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Verify session deleted
        let sess = state.db.get_session(session_token).await.unwrap();
        assert!(sess.is_none(), "Session should be deleted after logout");

        // Check clear cookies
        let clear_cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect();
        assert!(clear_cookies.iter().any(|c| c.contains("Max-Age=0")), "Should clear cookies");
    }

    // ── GET /v1/auth/me ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn me_returns_user_when_session_valid() {
        let state = build_state().await;
        create_user_with_password(&state, "me@test.com", "password12345678").await;
        let app = build_auth_router(state.clone());

        // Login
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "me@test.com", "password": "password12345678" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect();
        let session_token = cookies.iter()
            .find(|c| c.starts_with("litegen_session=")).unwrap()
            .split('=').nth(1).unwrap()
            .split(';').next().unwrap()
            .to_string();

        // GET /v1/auth/me
        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/me")
            .header("cookie", format!("litegen_session={}", session_token))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["user"]["email"], "me@test.com");
    }

    #[tokio::test]
    async fn me_returns_401_when_no_auth() {
        let state = build_state().await;
        // State has no master key set so auth passes through (no-auth mode)
        // But me() checks ctx.user which is None in no-auth mode
        let state_for_mw = state.clone();
        let app = Router::new()
            .route("/v1/auth/me", get(me))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                let s = state_for_mw.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/me")
            .body(Body::empty())
            .unwrap();

        // In no-master-key mode, auth passes through but user is None
        let resp = app.oneshot(req).await.unwrap();
        // me() returns 401 when ctx.user is None
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── GET /v1/auth/csrf ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn csrf_endpoint_returns_token() {
        let state = build_state().await;
        create_user_with_password(&state, "csrf@test.com", "password12345678").await;
        let app = build_auth_router(state.clone());

        // Login
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "csrf@test.com", "password": "password12345678" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect();
        let session_token = cookies.iter()
            .find(|c| c.starts_with("litegen_session=")).unwrap()
            .split('=').nth(1).unwrap()
            .split(';').next().unwrap()
            .to_string();

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/csrf")
            .header("cookie", format!("litegen_session={}", session_token))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["csrf_token"].as_str().is_some(), "Should return csrf_token");
        assert_eq!(body["csrf_token"].as_str().unwrap().len(), 64, "CSRF token should be 64 chars");
    }

    // ── Password reset ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn reset_request_always_returns_200() {
        let state = build_state().await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "nobody@unknown.com" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Should always return 200 (no enumeration)");
    }

    #[tokio::test]
    async fn reset_request_inserts_token_for_known_email() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("LITEGEN__DEV__EXPOSE_RESET_TOKENS", "true");
        let state = build_state().await;
        create_user_with_password(&state, "reset@test.com", "password12345678").await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "reset@test.com" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        std::env::remove_var("LITEGEN__DEV__EXPOSE_RESET_TOKENS");
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["_dev_token"].as_str().is_some(), "Should expose dev token");
    }

    #[tokio::test]
    async fn reset_confirm_with_valid_token_updates_password() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("LITEGEN__DEV__EXPOSE_RESET_TOKENS", "true");
        let state = build_state().await;
        create_user_with_password(&state, "resetconfirm@test.com", "oldpassword12345").await;
        let app = build_auth_router(state.clone());

        // Request reset
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "resetconfirm@test.com" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        std::env::remove_var("LITEGEN__DEV__EXPOSE_RESET_TOKENS");

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let token = body["_dev_token"].as_str().unwrap().to_string();

        // Confirm reset
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/confirm")
            .header("content-type", "application/json")
            .body(json_body(json!({ "token": token, "new_password": "newstrongpassword123" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Can login with new password
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "resetconfirm@test.com", "password": "newstrongpassword123" })))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Should login with new password");
    }

    #[tokio::test]
    async fn reset_confirm_token_is_single_use() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("LITEGEN__DEV__EXPOSE_RESET_TOKENS", "true");
        let state = build_state().await;
        create_user_with_password(&state, "resetonce@test.com", "oldpassword12345").await;
        let app = build_auth_router(state.clone());

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "resetonce@test.com" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        std::env::remove_var("LITEGEN__DEV__EXPOSE_RESET_TOKENS");

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["_dev_token"]
            .as_str()
            .unwrap()
            .to_string();

        // Use token once
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/confirm")
            .header("content-type", "application/json")
            .body(json_body(json!({ "token": &token, "new_password": "newstrongpassword123" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Use token again — should fail
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/confirm")
            .header("content-type", "application/json")
            .body(json_body(json!({ "token": &token, "new_password": "anotherpassword123" })))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "Reuse of token should fail");
    }

    #[tokio::test]
    async fn login_to_inactive_account_returns_401_not_403() {
        use crate::auth::password::hash_password;
        let state = build_state().await;
        // Create an inactive user with a known password
        let hash = hash_password("correctpassword123").unwrap();
        let user = User {
            id: format!("user-{}", uuid::Uuid::new_v4()),
            email: "inactive@test.com".to_string(),
            password_hash: Some(hash),
            role: Role::Member,
            oauth_github_id: None,
            oauth_google_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: false, // <-- inactive
        };
        state.db.create_user(&user).await.unwrap();

        let app = build_auth_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "inactive@test.com", "password": "correctpassword123" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "Inactive account must return 401, not 403");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "invalid_credentials",
            "Inactive account must return invalid_credentials code, not account_inactive");
    }

    #[tokio::test]
    async fn reset_confirm_revokes_all_sessions() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("LITEGEN__DEV__EXPOSE_RESET_TOKENS", "true");
        let state = build_state().await;
        create_user_with_password(&state, "revokeall@test.com", "oldpassword12345").await;
        let app = build_auth_router(state.clone());

        // Create a session for the user
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "revokeall@test.com", "password": "oldpassword12345" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect();
        let session_token = cookies.iter()
            .find(|c| c.starts_with("litegen_session=")).unwrap()
            .split('=').nth(1).unwrap()
            .split(';').next().unwrap()
            .to_string();

        // Verify session exists
        let sess = state.db.get_session(&session_token).await.unwrap();
        assert!(sess.is_some());

        // Request reset
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "revokeall@test.com" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        std::env::remove_var("LITEGEN__DEV__EXPOSE_RESET_TOKENS");

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["_dev_token"]
            .as_str()
            .unwrap()
            .to_string();

        // Confirm reset
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/confirm")
            .header("content-type", "application/json")
            .body(json_body(json!({ "token": &token, "new_password": "newstrongpassword123" })))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Session should be deleted
        let sess = state.db.get_session(&session_token).await.unwrap();
        assert!(sess.is_none(), "Session should be deleted after password reset");
    }
}
