pub mod backpressure;
pub mod rate_limit;
pub mod validator;
#[cfg(test)] mod validator_tests;
#[cfg(test)] mod e2e_tests;
#[cfg(test)] mod auth_tests;
#[cfg(test)] mod session_auth_tests;

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::oauth::OAuthConfig;
use crate::auth::permissions::{permissions_for, Permission};
use crate::auth::tokens::constant_time_eq;
use crate::config::Mode;
use crate::db::DatabaseStore;
use crate::proxy::materializer::Materializer;
use crate::proxy::router::ProxyRouter;
use crate::types::Role;
use crate::api::middleware::backpressure::InFlightLimit;
use crate::api::middleware::rate_limit::RateLimiter;

// ─── Default tenant ids (single-tenant) ───────────────────────────────────────

/// Default organization id created by migration 0008. In `single_tenant` mode
/// the master key and dev bypass are scoped to this org.
pub const DEFAULT_ORG_ID: &str = "00000000-0000-0000-0000-000000000001";
/// Default application id created by migration 0008.
pub const DEFAULT_APP_ID: &str = "00000000-0000-0000-0000-000000000002";

// ─── Scope ───────────────────────────────────────────────────────────────────

/// Scopes control which routes a key can access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Generate,
    Read,
    Admin,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "generate" => Some(Scope::Generate),
            "read" => Some(Scope::Read),
            "admin" => Some(Scope::Admin),
            _ => None,
        }
    }
}

// ─── UserContext ──────────────────────────────────────────────────────────────

/// Authenticated user, populated when a session cookie is used.
#[derive(Debug, Clone)]
pub struct UserContext {
    pub user_id: String,
    pub email: String,
    pub role: Role,
}

// ─── KeyContext ───────────────────────────────────────────────────────────────

/// Auth context injected into request extensions by `auth_middleware`.
/// Handlers and downstream middleware read this to know who is calling.
#[derive(Debug, Clone)]
pub struct KeyContext {
    /// None for master key; Some(id) for DB-backed keys.
    pub key_id: Option<Uuid>,
    pub scopes: Vec<Scope>,
    pub rpm_limit: Option<u32>,
    /// Remaining quota in USD. None = unlimited (either no quota set or master key).
    pub quota_remaining: Option<f64>,
    pub webhook_url: Option<String>,
    /// Populated when session cookie auth is used.
    pub user: Option<UserContext>,
    /// Role-derived permissions (populated for cookie-auth sessions).
    pub permissions: Vec<Permission>,
    /// Session ID for CSRF lookup (Some only when cookie auth was used).
    pub session_id: Option<String>,
    /// Active organization id for this request (tenant context).
    /// None for platform-admin (hosted master key) until a tenant is selected.
    pub org_id: Option<String>,
    /// Active application id for this request (tenant context).
    pub app_id: Option<String>,
}

// ─── AppState ────────────────────────────────────────────────────────────────

/// Shared application state passed to all handlers.
pub struct AppState {
    pub router: Arc<ProxyRouter>,
    pub db: Arc<dyn DatabaseStore>,
    pub master_key: Option<String>,
    pub registry: Arc<crate::capabilities::CapabilityRegistry>,
    pub materializer: Arc<Materializer>,
    pub rate_limiter: Arc<RateLimiter>,
    pub in_flight: Arc<InFlightLimit>,
    pub oauth: OAuthConfig,
    pub mode: crate::config::Mode,
    pub secrets_key: Option<[u8; 32]>,
    pub dev: crate::config::DevFlags,
}

// ─── Auth middleware ─────────────────────────────────────────────────────────

/// Authentication middleware.
/// If a master_key is configured, all requests must provide a valid key.
/// Sets a `KeyContext` in the request extensions.
pub async fn auth_middleware(
    headers: HeaderMap,
    state: Arc<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.strip_prefix("Bearer ").unwrap_or(v));

    // Try Bearer token first
    if let Some(api_key) = auth_header.filter(|k| !k.is_empty()) {
        // Check master key first — unlimited, all scopes, no rate limit
        // Use constant-time comparison to prevent timing oracle attacks.
        if let Some(master) = &state.master_key {
            if constant_time_eq(api_key, master.as_str()) {
                // Master key tenant scope depends on mode:
                //  - single_tenant: god within the default org/app.
                //  - hosted: platform admin, not bound to any tenant (None).
                let (org_id, app_id) = match state.mode {
                    Mode::SingleTenant => (
                        Some(DEFAULT_ORG_ID.to_string()),
                        Some(DEFAULT_APP_ID.to_string()),
                    ),
                    Mode::Hosted => (None, None),
                };
                let ctx = KeyContext {
                    key_id: None,
                    scopes: vec![Scope::Generate, Scope::Read, Scope::Admin],
                    rpm_limit: None,
                    quota_remaining: None,
                    webhook_url: None,
                    user: None,
                    permissions: vec![],
                    session_id: None,
                    org_id,
                    app_id,
                };
                request.extensions_mut().insert(ctx);
                return next.run(request).await;
            }
        }

        // If no master key is configured and a Bearer token is present,
        // accept any token (dev bypass — the token is irrelevant).
        // Only enabled in single_tenant mode; hosted mode requires real auth.
        if state.master_key.is_none() && state.mode == Mode::SingleTenant {
            let ctx = KeyContext {
                key_id: None,
                scopes: vec![Scope::Generate, Scope::Read, Scope::Admin],
                rpm_limit: None,
                quota_remaining: None,
                webhook_url: None,
                user: None,
                permissions: vec![],
                session_id: None,
                org_id: Some(DEFAULT_ORG_ID.to_string()),
                app_id: Some(DEFAULT_APP_ID.to_string()),
            };
            request.extensions_mut().insert(ctx);
            return next.run(request).await;
        }

        // master_key configured → validate as DB key.
        // master_key None + hosted → fall through (no bypass) to the final 401.
        if state.master_key.is_some() {
            return handle_db_key(api_key, state, request, next).await;
        }
    }

    // Try session cookie auth
    if let Some(sid) = cookie_value(request.headers(), "litegen_session") {
        if let Ok(Some(sess)) = state.db.get_session(&sid).await {
            if sess.expires_at < chrono::Utc::now() {
                let _ = state.db.delete_session(&sid).await;
                return unauthorized_response("Session expired");
            }
            if let Ok(Some(user)) = state.db.get_user_by_id(&sess.user_id).await {
                if !user.is_active {
                    return unauthorized_response("Account inactive");
                }
                // Bump expiry if within 24h of expiring
                if sess.expires_at < chrono::Utc::now() + chrono::Duration::hours(24) {
                    let new_exp = chrono::Utc::now() + chrono::Duration::days(7);
                    let _ = state.db.bump_session_expiry(&sid, new_exp).await;
                }

                // TODO(perf): this issues up to ~2 extra DB queries per session request; cache the active tenant per session if it becomes hot.
                // ─── Resolve active org/app + per-org permissions ───────────
                // Active org: explicit header (validated membership) -> first
                // org -> default(single_tenant)/none(hosted).
                let header_org = request
                    .headers()
                    .get("x-litegen-org-id")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                let (org_id, membership_role) = if let Some(o) = header_org {
                    match state.db.get_membership(&o, &user.id).await {
                        Ok(Some(role)) => (Some(o), Some(role)),
                        Ok(None) => return forbidden_response("Not a member of the requested organization"),
                        Err(_) => return internal_error_response("organization lookup failed"),
                    }
                } else {
                    match state.db.list_orgs_for_user(&user.id).await {
                        Ok(mut v) if !v.is_empty() => {
                            let (org, role) = v.remove(0);
                            (Some(org.id), Some(role))
                        }
                        _ => {
                            if state.mode == Mode::SingleTenant {
                                (Some(DEFAULT_ORG_ID.to_string()), None)
                            } else {
                                (None, None)
                            }
                        }
                    }
                };
                // Active app: explicit header (validated to belong to org) ->
                // first app of org -> default(single_tenant)/none.
                let header_app = request
                    .headers()
                    .get("x-litegen-app-id")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                let app_id = match (&org_id, header_app) {
                    (Some(o), Some(a)) => match state.db.get_application(&a).await {
                        Ok(Some(app)) if &app.org_id == o => Some(a),
                        Ok(_) => return forbidden_response(
                            "Application does not belong to the active organization",
                        ),
                        Err(_) => return internal_error_response("application lookup failed"),
                    },
                    (Some(o), None) => {
                        if o == DEFAULT_ORG_ID {
                            Some(DEFAULT_APP_ID.to_string())
                        } else {
                            match state.db.list_apps_for_org(o).await {
                                Ok(mut v) => if v.is_empty() { None } else { Some(v.remove(0).id) },
                                Err(_) => return internal_error_response("application listing failed"),
                            }
                        }
                    }
                    _ => None,
                };
                // Permissions: membership role if resolved, else fall back to
                // the user's global role (preserves single_tenant behavior +
                // existing tests).
                let role_for_perms = membership_role.unwrap_or(user.role);
                let perms = permissions_for(role_for_perms).to_vec();

                let ctx = KeyContext {
                    key_id: None,
                    scopes: vec![Scope::Generate, Scope::Read, Scope::Admin],
                    rpm_limit: None,
                    quota_remaining: None,
                    webhook_url: None,
                    user: Some(UserContext {
                        user_id: user.id,
                        email: user.email,
                        role: user.role,
                    }),
                    permissions: perms,
                    session_id: Some(sid),
                    org_id,
                    app_id,
                };
                request.extensions_mut().insert(ctx);
                return next.run(request).await;
            }
        }
        // Session not found or user not found → 401
        return unauthorized_response("Invalid session");
    }

    // No Bearer and no session cookie.
    // In dev mode (no master key), allow through with an all-scopes context.
    // Only enabled in single_tenant mode; hosted mode requires real auth.
    if state.master_key.is_none() && state.mode == Mode::SingleTenant {
        let ctx = KeyContext {
            key_id: None,
            scopes: vec![Scope::Generate, Scope::Read, Scope::Admin],
            rpm_limit: None,
            quota_remaining: None,
            webhook_url: None,
            user: None,
            permissions: vec![],
            session_id: None,
            org_id: Some(DEFAULT_ORG_ID.to_string()),
            app_id: Some(DEFAULT_APP_ID.to_string()),
        };
        request.extensions_mut().insert(ctx);
        return next.run(request).await;
    }

    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": {
                "message": "Missing API key. Provide via Authorization: Bearer <key>",
                "type": "authentication_error",
                "code": 401
            }
        })),
    )
        .into_response()
}

fn unauthorized_response(msg: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": {
                "message": msg,
                "type": "authentication_error",
                "code": 401
            }
        })),
    )
        .into_response()
}

fn forbidden_response(msg: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": {
                "message": msg,
                "type": "forbidden",
                "code": 403
            }
        })),
    )
        .into_response()
}

fn internal_error_response(msg: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error":{"message":msg,"type":"internal_error","code":500}})),
    ).into_response()
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie_header = headers.get("cookie")?.to_str().ok()?;
    cookie_header
        .split(';')
        .map(|s| s.trim())
        .find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            if k == name { Some(v.to_string()) } else { None }
        })
}

async fn handle_db_key(
    api_key: &str,
    state: Arc<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    // Hash and look up DB key
    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(api_key.as_bytes());
        hex::encode(hasher.finalize())
    };

    match state.db.lookup_api_key_by_hash(&hash).await {
        Ok(Some(key)) => {
            // Check expiry
            if let Some(exp) = key.expires_at {
                if exp < chrono::Utc::now() {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "error": {
                                "message": "API key expired",
                                "type": "authentication_error",
                                "code": 401
                            }
                        })),
                    ).into_response();
                }
            }

            // Pre-flight quota check: if tokens_used >= token_quota, reject with 402
            if let Some(quota) = key.token_quota {
                if key.tokens_used >= quota {
                    return (
                        StatusCode::PAYMENT_REQUIRED,
                        Json(serde_json::json!({
                            "error": {
                                "message": "Token quota exceeded for this API key",
                                "type": "quota_exceeded",
                                "code": 402
                            }
                        })),
                    ).into_response();
                }
            }

            let quota_remaining = key.token_quota.map(|quota| quota - key.tokens_used);

            let scopes: Vec<Scope> = key.scopes
                .split(',')
                .filter_map(Scope::parse)
                .collect();

            let ctx = KeyContext {
                key_id: Some(key.id),
                scopes,
                rpm_limit: key.rpm_limit,
                quota_remaining,
                webhook_url: key.webhook_url,
                user: None,
                permissions: vec![],
                session_id: None,
                org_id: key.org_id.clone(),
                app_id: key.app_id.clone(),
            };

            // RPM rate limit check
            if let (Some(key_id), Some(rpm)) = (ctx.key_id, ctx.rpm_limit) {
                match state.rate_limiter.try_take(key_id, rpm).await {
                    Ok(()) => {}
                    Err(retry_after) => {
                        let rpm_str = rpm.to_string();
                        let mut resp = (
                            StatusCode::TOO_MANY_REQUESTS,
                            Json(serde_json::json!({
                                "error": {
                                    "message": format!("Rate limit exceeded. Retry after {} seconds.", retry_after),
                                    "type": "rate_limit_exceeded",
                                    "code": 429
                                }
                            })),
                        ).into_response();
                        let h = resp.headers_mut();
                        if let Ok(v) = axum::http::HeaderValue::from_str(&retry_after.to_string()) {
                            h.insert(axum::http::header::RETRY_AFTER, v);
                        }
                        if let Ok(v) = axum::http::HeaderValue::from_str(&rpm_str) {
                            h.insert("x-ratelimit-limit", v);
                        }
                        h.insert("x-ratelimit-remaining", axum::http::HeaderValue::from_static("0"));
                        let reset_epoch = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() + retry_after;
                        if let Ok(v) = axum::http::HeaderValue::from_str(&reset_epoch.to_string()) {
                            h.insert("x-ratelimit-reset", v);
                        }
                        return resp;
                    }
                }
            }

            // Save key_id and rpm_limit before moving ctx into extensions
            let rate_limit_key_id = ctx.key_id;
            let rate_limit_rpm = ctx.rpm_limit;
            request.extensions_mut().insert(ctx);

            let mut response = next.run(request).await;

            // Attach X-RateLimit-* headers for rate-limited keys
            if let (Some(key_id), Some(rpm)) = (rate_limit_key_id, rate_limit_rpm) {
                let snapshot = state.rate_limiter.snapshot(key_id, rpm).await;
                let headers = response.headers_mut();
                if let Ok(v) = axum::http::HeaderValue::from_str(&rpm.to_string()) {
                    headers.insert("x-ratelimit-limit", v);
                }
                if let Ok(v) = axum::http::HeaderValue::from_str(&snapshot.remaining.to_string()) {
                    headers.insert("x-ratelimit-remaining", v);
                }
                let reset_epoch = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    + snapshot.reset_in.as_secs();
                if let Ok(v) = axum::http::HeaderValue::from_str(&reset_epoch.to_string()) {
                    headers.insert("x-ratelimit-reset", v);
                }
            }
            // Master key (key_id = None) or keys without rpm_limit get no headers

            response
        }
        Ok(None) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": {
                    "message": "Invalid or inactive API key",
                    "type": "authentication_error",
                    "code": 401
                }
            })),
        ).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to look up API key");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": "Internal authentication error",
                        "type": "internal_error",
                        "code": 500
                    }
                })),
            ).into_response()
        }
    }
}

// ─── Scope middleware ─────────────────────────────────────────────────────────

/// Axum middleware function that checks the `KeyContext` for the required scope.
/// Returns 403 if the scope is missing, 401 if there's no context.
pub async fn check_scope(required: Scope, request: Request, next: Next) -> Response {
    let ctx = request.extensions().get::<KeyContext>().cloned();
    match ctx {
        Some(ctx) if ctx.scopes.contains(&required) => next.run(request).await,
        Some(_) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": {
                    "message": "Insufficient scope for this endpoint",
                    "type": "forbidden_scope",
                    "code": 403
                }
            })),
        ).into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": {
                    "message": "Not authenticated",
                    "type": "authentication_error",
                    "code": 401
                }
            })),
        ).into_response(),
    }
}

// ─── CSRF middleware ─────────────────────────────────────────────────────────

/// CSRF enforcement middleware for session-cookie-authenticated requests.
/// For mutating verbs (POST/PUT/PATCH/DELETE), if the request used cookie auth
/// (session_id is Some), the `X-CSRF-Token` header must match the stored token.
/// Bearer-token requests bypass CSRF entirely.
pub async fn csrf_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let method = req.method().clone();
    // Only check on mutating methods
    if !matches!(
        method,
        axum::http::Method::POST
            | axum::http::Method::PUT
            | axum::http::Method::PATCH
            | axum::http::Method::DELETE
    ) {
        return next.run(req).await;
    }

    let ctx = req.extensions().get::<KeyContext>().cloned();
    let Some(ctx) = ctx else {
        return next.run(req).await;
    };

    // Bearer-authenticated requests skip CSRF (no session_id)
    let Some(ref sid) = ctx.session_id else {
        return next.run(req).await;
    };

    let header_token = req
        .headers()
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let Some(header_token) = header_token else {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": {
                    "message": "CSRF token missing",
                    "type": "csrf_error",
                    "code": 403
                }
            })),
        )
            .into_response();
    };

    let sess = state.db.get_session(sid).await.ok().flatten();
    let Some(sess) = sess else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": {
                    "message": "Session not found",
                    "type": "authentication_error",
                    "code": 401
                }
            })),
        )
            .into_response();
    };

    if !constant_time_eq(&sess.csrf_token, &header_token) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": {
                    "message": "Invalid CSRF token",
                    "type": "csrf_error",
                    "code": 403
                }
            })),
        )
            .into_response();
    }

    next.run(req).await
}

// ─── Permission middleware ────────────────────────────────────────────────────

/// Check that the `KeyContext` contains the given permission.
/// Returns 403 if the permission is missing, passes through otherwise.
pub async fn check_permission(required: Permission, request: Request, next: Next) -> Response {
    let ctx = request.extensions().get::<KeyContext>().cloned();
    match ctx {
        Some(ctx) if ctx.permissions.contains(&required) => next.run(request).await,
        Some(_) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Permission '{}' required", required.as_str()),
                    "type": "forbidden_permission",
                    "code": 403
                }
            })),
        )
            .into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": {
                    "message": "Not authenticated",
                    "type": "authentication_error",
                    "code": 401
                }
            })),
        )
            .into_response(),
    }
}

// ─── Session helper ──────────────────────────────────────────────────────────

/// Cookie security: set `litegen_session` and `litegen_csrf` cookies.
/// Returns `(session_cookie_value, csrf_cookie_value)` as header values.
pub fn make_session_cookies(
    session_token: &str,
    csrf_token: &str,
) -> (axum::http::HeaderValue, axum::http::HeaderValue) {
    let secure = std::env::var("LITEGEN__COOKIE_INSECURE_DEV").as_deref() != Ok("true");
    let secure_str = if secure { "; Secure" } else { "" };
    let session_val = format!(
        "litegen_session={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800{}",
        session_token, secure_str
    );
    let csrf_val = format!(
        "litegen_csrf={}; Path=/; SameSite=Lax; Max-Age=604800{}",
        csrf_token, secure_str
    );
    (
        axum::http::HeaderValue::from_str(&session_val)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("")),
        axum::http::HeaderValue::from_str(&csrf_val)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("")),
    )
}

/// Cookie values that clear both session cookies (Max-Age=0).
pub fn make_clear_session_cookies() -> (axum::http::HeaderValue, axum::http::HeaderValue) {
    let session_val = "litegen_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0";
    let csrf_val = "litegen_csrf=; Path=/; SameSite=Lax; Max-Age=0";
    (
        axum::http::HeaderValue::from_static(session_val),
        axum::http::HeaderValue::from_static(csrf_val),
    )
}

/// Create a session in the database and return the Set-Cookie header values.
pub async fn create_session_cookies(
    db: &Arc<dyn crate::db::DatabaseStore>,
    user_id: &str,
    ip: Option<String>,
    user_agent: Option<String>,
) -> Result<(String, String, axum::http::HeaderValue, axum::http::HeaderValue), sqlx::Error> {
    use crate::auth::tokens::{generate_session_token, generate_csrf_token};
    use crate::types::Session;

    let session_token = generate_session_token();
    let csrf_token = generate_csrf_token();
    let sess = Session {
        id: session_token.clone(),
        user_id: user_id.to_string(),
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        ip,
        user_agent,
        csrf_token: csrf_token.clone(),
    };
    db.create_session(&sess).await?;
    let (sc, cc) = make_session_cookies(&session_token, &csrf_token);
    Ok((session_token, csrf_token, sc, cc))
}
