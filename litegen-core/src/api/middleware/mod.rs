pub mod backpressure;
pub mod idempotency;
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
    /// Set when the caller signed in to MCP through OAuth: the grant it acts
    /// under. Such a principal acts as its `user`, confined to `org_id` and to
    /// `scopes` (see [`within_grant`] and [`check_grant_scope`]).
    pub oauth_grant_id: Option<String>,
}

/// The principal an in-process caller already resolved (MCP, for an OAuth
/// access token). Request extensions cannot come from the network, so only
/// code in this process can set one; [`auth_middleware`] then takes it as is.
#[derive(Debug, Clone)]
pub(crate) struct McpPrincipal(pub KeyContext);

/// Whether `ctx` may act on `org_id`: every principal may try (the handler's
/// own checks decide), except an OAuth one, which stays in the org its grant
/// names.
pub fn within_grant(ctx: &KeyContext, org_id: &str) -> bool {
    ctx.oauth_grant_id.is_none() || ctx.org_id.as_deref() == Some(org_id)
}

/// For an OAuth principal, the scope its grant must hold for the request:
/// `read` (or `admin`) to look, `admin` to change. Sessions hold every scope
/// and API keys are checked by their routes' own scope layers, so only OAuth
/// principals are checked here. Layered on the route groups that authorize
/// people by role (accounts, orgs, members, management).
pub async fn check_grant_scope(request: Request, next: Next) -> Response {
    let read = matches!(*request.method(), axum::http::Method::GET | axum::http::Method::HEAD);
    // Any signed-in app may ask whose account it acts for.
    let whoami = read && request.uri().path() == "/v1/auth/me";
    let refused = !whoami
        && request.extensions().get::<KeyContext>().filter(|c| c.oauth_grant_id.is_some()).is_some_and(|ctx| {
            let has = |s: Scope| ctx.scopes.contains(&s);
            !(has(Scope::Admin) || (read && has(Scope::Read)))
        });
    if refused {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": {
                    "message": "This app's sign-in does not include the scope this call needs",
                    "type": "forbidden_scope",
                    "code": "forbidden_scope"
                }
            })),
        )
            .into_response();
    }
    next.run(request).await
}

/// The platform admin (master key) is the only principal bound to no tenant:
/// no session user, no DB API key, and no session id. A tenant session always
/// carries `user`/`session_id`; a DB-backed API key always carries `key_id`.
/// In hosted mode there is no dev/no-auth bypass, so this uniquely identifies
/// the master key. (In single-tenant dev mode the no-auth context also matches,
/// which is harmless — that context already carries all permissions.)
pub(crate) fn is_platform_admin(ctx: &KeyContext) -> bool {
    ctx.user.is_none() && ctx.key_id.is_none() && ctx.session_id.is_none()
}

/// The maximal set of scopes `ctx` is permitted to grant to a newly created API
/// key. A key must never carry more privilege than the principal that mints it:
/// the `admin` scope bypasses permission gates on the Bearer auth path, so it
/// may only be granted by a principal that itself wields admin authority.
///
/// - platform master key → unrestricted.
/// - session (cookie) auth → role-derived: Admin/Owner (who hold `KeyWriteAny`)
///   may grant `admin`; a Member (only `KeyWriteOwn`) may not.
/// - Bearer/DB-key auth → a key may only confer scopes it itself holds.
pub fn grantable_scopes(ctx: &KeyContext) -> Vec<Scope> {
    if is_platform_admin(ctx) {
        return vec![Scope::Generate, Scope::Read, Scope::Admin];
    }
    if ctx.user.is_some() {
        return if ctx.permissions.contains(&Permission::KeyWriteAny) {
            vec![Scope::Generate, Scope::Read, Scope::Admin]
        } else {
            vec![Scope::Generate, Scope::Read]
        };
    }
    ctx.scopes.clone()
}

/// Refuse a scope list naming an unknown scope (400, so a typo is not a silently
/// missing scope) or a scope `ctx` may not grant (403). Runs when a key is
/// created and when its scopes are edited, so a key can never hold more
/// authority than the principal that set its scopes.
pub(crate) fn validate_grant(ctx: &KeyContext, scopes_csv: &str) -> Result<(), Response> {
    let grantable = grantable_scopes(ctx);
    for token in scopes_csv.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match Scope::parse(token) {
            Some(scope) if grantable.contains(&scope) => {}
            Some(_) => {
                return Err(grant_error(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    format!("insufficient privilege to grant scope '{token}'"),
                ))
            }
            None => {
                return Err(grant_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    format!("unknown scope '{token}'"),
                ))
            }
        }
    }
    Ok(())
}

fn grant_error(status: StatusCode, kind: &str, message: String) -> Response {
    (status, Json(serde_json::json!({ "error": { "message": message, "type": kind, "code": kind } }))).into_response()
}

#[cfg(test)]
mod scope_grant_tests {
    use super::*;
    use crate::auth::permissions::{permissions_for, Permission};
    use crate::types::Role;

    fn session_ctx(role: Role) -> KeyContext {
        KeyContext {
            key_id: None,
            scopes: vec![Scope::Generate, Scope::Read, Scope::Admin],
            rpm_limit: None,
            quota_remaining: None,
            webhook_url: None,
            user: Some(UserContext {
                user_id: "u1".into(),
                email: "u@example.com".into(),
                role,
            }),
            permissions: permissions_for(role).to_vec(),
            session_id: Some("sess-1".into()),
            org_id: Some("org-1".into()),
            app_id: Some("app-1".into()),
            oauth_grant_id: None,
        }
    }

    fn bearer_ctx(scopes: Vec<Scope>) -> KeyContext {
        KeyContext {
            key_id: Some(uuid::Uuid::new_v4()),
            scopes,
            rpm_limit: None,
            quota_remaining: None,
            webhook_url: None,
            user: None,
            permissions: vec![],
            session_id: None,
            org_id: Some("org-1".into()),
            app_id: Some("app-1".into()),
            oauth_grant_id: None,
        }
    }

    /// An org owner signed in to MCP through OAuth, with a grant holding `scopes`.
    fn oauth_ctx(scopes: Vec<Scope>) -> KeyContext {
        KeyContext { scopes, session_id: None, oauth_grant_id: Some("grant-1".into()), ..session_ctx(Role::Owner) }
    }

    #[tokio::test]
    async fn an_oauth_principal_needs_its_grants_scope_where_roles_decide() {
        use axum::body::Body;
        use axum::routing::get;
        use tower::ServiceExt;
        let status = |ctx: KeyContext, method: &str, path: &str| {
            let app = axum::Router::new()
                .route("/v1/orgs/o1", get(|| async { "ok" }).patch(|| async { "ok" }))
                .route("/v1/auth/me", get(|| async { "me" }))
                .layer(axum::middleware::from_fn(check_grant_scope))
                .layer(axum::Extension(ctx));
            let req = Request::builder().method(method).uri(path).body(Body::empty()).unwrap();
            async move { app.oneshot(req).await.unwrap().status() }
        };
        let ok = StatusCode::OK;
        let refused = StatusCode::FORBIDDEN;

        assert_eq!(status(oauth_ctx(vec![Scope::Read]), "GET", "/v1/orgs/o1").await, ok);
        assert_eq!(status(oauth_ctx(vec![Scope::Read]), "PATCH", "/v1/orgs/o1").await, refused, "a read grant changes nothing");
        assert_eq!(status(oauth_ctx(vec![Scope::Admin]), "PATCH", "/v1/orgs/o1").await, ok);
        assert_eq!(status(oauth_ctx(vec![Scope::Admin]), "GET", "/v1/orgs/o1").await, ok, "admin covers reads");
        assert_eq!(status(oauth_ctx(vec![Scope::Generate]), "GET", "/v1/orgs/o1").await, refused, "an AI grant reads no org data");
        assert_eq!(status(oauth_ctx(vec![Scope::Generate]), "GET", "/v1/auth/me").await, ok, "but may ask who it is");

        // Sessions hold every scope and keys answer to their routes' own layers.
        assert_eq!(status(session_ctx(Role::Viewer), "PATCH", "/v1/orgs/o1").await, ok);
        assert_eq!(status(bearer_ctx(vec![Scope::Read]), "PATCH", "/v1/orgs/o1").await, ok);
    }

    #[test]
    fn an_oauth_principal_stays_in_its_grants_org() {
        assert!(within_grant(&oauth_ctx(vec![Scope::Admin]), "org-1"));
        assert!(!within_grant(&oauth_ctx(vec![Scope::Admin]), "org-2"));
        assert!(within_grant(&session_ctx(Role::Owner), "org-2"), "a session's handlers check membership themselves");
        assert!(within_grant(&bearer_ctx(vec![Scope::Admin]), "org-2"), "a key's handlers check its org themselves");
    }

    #[test]
    fn member_session_cannot_grant_admin_scope() {
        let g = grantable_scopes(&session_ctx(Role::Member));
        assert!(!g.contains(&Scope::Admin), "a Member must not be able to mint an admin-scope key");
        assert!(g.contains(&Scope::Generate) && g.contains(&Scope::Read));
    }

    #[test]
    fn admin_session_can_grant_admin_scope() {
        assert!(grantable_scopes(&session_ctx(Role::Admin)).contains(&Scope::Admin));
        assert!(grantable_scopes(&session_ctx(Role::Owner)).contains(&Scope::Admin));
    }

    #[test]
    fn bearer_key_can_only_grant_scopes_it_holds() {
        // A generate+read key cannot escalate by minting an admin key.
        let g = grantable_scopes(&bearer_ctx(vec![Scope::Generate, Scope::Read]));
        assert!(!g.contains(&Scope::Admin));
        // An admin-scope key may pass admin along.
        assert!(grantable_scopes(&bearer_ctx(vec![Scope::Admin])).contains(&Scope::Admin));
    }

    #[test]
    fn platform_master_key_can_grant_admin() {
        let mut ctx = bearer_ctx(vec![]);
        ctx.key_id = None; // master key: no user, no key_id, no session
        assert!(grantable_scopes(&ctx).contains(&Scope::Admin));
    }

    // Guard against the permission model drifting: the whole check hinges on
    // Member lacking KeyWriteAny while Admin/Owner hold it.
    #[test]
    fn member_lacks_key_write_any() {
        assert!(!permissions_for(Role::Member).contains(&Permission::KeyWriteAny));
        assert!(permissions_for(Role::Admin).contains(&Permission::KeyWriteAny));
    }
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
    /// Whether email/password signup + login is enabled. Default `true`.
    /// When `false` (OAuth-only hosted deployments), the password endpoints
    /// reject with 403 `password_auth_disabled`.
    pub allow_password: bool,
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
    // A principal MCP resolved in this process (an OAuth access token) is
    // taken as is: it carries no credentials to check again.
    if let Some(McpPrincipal(ctx)) = request.extensions_mut().remove::<McpPrincipal>() {
        return run_rate_limited(ctx, state, request, next).await;
    }

    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.strip_prefix("Bearer ").unwrap_or(v));

    // Try Bearer token first: the master key, the dev bypass, or a DB key.
    // master_key None + hosted → fall through (no bypass) to the final 401.
    if let Some(api_key) = auth_header.filter(|k| !k.is_empty()) {
        if state.master_key.is_some() || state.mode == Mode::SingleTenant {
            return match resolve_api_key(&state, api_key).await {
                Ok(ctx) => run_rate_limited(ctx, state, request, next).await,
                Err(resp) => resp,
            };
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
                    oauth_grant_id: None,
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
    if let Some(ctx) = dev_bypass_context(&state) {
        request.extensions_mut().insert(ctx);
        return next.run(request).await;
    }

    missing_credentials_response()
}

/// The 401 for a request that brought no credentials at all.
pub(crate) fn missing_credentials_response() -> Response {
    unauthorized_response("Missing API key. Provide via Authorization: Bearer <key>")
}

/// The context of every request in dev mode (single-tenant, no master key):
/// all scopes, the default org and app, no credentials needed. `None` in any
/// other configuration.
pub(crate) fn dev_bypass_context(state: &AppState) -> Option<KeyContext> {
    (state.master_key.is_none() && state.mode == Mode::SingleTenant).then(|| KeyContext {
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
        oauth_grant_id: None,
    })
}

/// The master key's context. Its tenant scope depends on the mode:
/// single-tenant → god within the default org/app; hosted → the platform
/// admin, bound to no tenant.
fn master_key_context(state: &AppState) -> KeyContext {
    let (org_id, app_id) = match state.mode {
        Mode::SingleTenant => (Some(DEFAULT_ORG_ID.to_string()), Some(DEFAULT_APP_ID.to_string())),
        Mode::Hosted => (None, None),
    };
    KeyContext {
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
        oauth_grant_id: None,
    }
}

/// Resolve a Bearer token exactly as `auth_middleware` does, minus the
/// per-minute rate limit: the master key (compared in constant time), the dev
/// bypass (single-tenant without a master key: any token), or a DB key, which
/// must be unexpired and under its quota. Anything else is a 401.
pub(crate) async fn resolve_api_key(state: &AppState, token: &str) -> Result<KeyContext, Response> {
    if let Some(master) = &state.master_key {
        if constant_time_eq(token, master.as_str()) {
            return Ok(master_key_context(state));
        }
    }
    if let Some(ctx) = dev_bypass_context(state) {
        return Ok(ctx);
    }
    if state.master_key.is_some() {
        return resolve_db_key(state, token).await;
    }
    Err(unauthorized_response("Invalid or inactive API key"))
}

fn unauthorized_response(msg: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": {
                "message": msg,
                "type": "authentication_error",
                "code": "authentication_error"
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
                "code": "forbidden"
            }
        })),
    )
        .into_response()
}

fn internal_error_response(msg: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error":{"message":msg,"type":"internal_error","code":"internal_error"}})),
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

/// Look a DB key up by its hash and build its context: refused when it is
/// unknown or inactive (401), expired (401) or over its quota (402).
async fn resolve_db_key(state: &AppState, api_key: &str) -> Result<KeyContext, Response> {
    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(api_key.as_bytes());
        hex::encode(hasher.finalize())
    };

    let key = match state.db.lookup_api_key_by_hash(&hash).await {
        Ok(Some(key)) => key,
        Ok(None) => return Err(unauthorized_response("Invalid or inactive API key")),
        Err(e) => {
            tracing::error!(error = %e, "Failed to look up API key");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": "Internal authentication error",
                        "type": "internal_error",
                        "code": "internal_error"
                    }
                })),
            ).into_response());
        }
    };

    // Check expiry
    if let Some(exp) = key.expires_at {
        if exp < chrono::Utc::now() {
            return Err(unauthorized_response("API key expired"));
        }
    }

    // Pre-flight quota check: if tokens_used >= token_quota, reject with 402
    if let Some(quota) = key.token_quota {
        if key.tokens_used >= quota {
            return Err((
                StatusCode::PAYMENT_REQUIRED,
                Json(serde_json::json!({
                    "error": {
                        "message": "Token quota exceeded for this API key",
                        "type": "quota_exceeded",
                        "code": "quota_exceeded"
                    }
                })),
            ).into_response());
        }
    }

    let quota_remaining = key.token_quota.map(|quota| quota - key.tokens_used);

    let scopes: Vec<Scope> = key.scopes
        .split(',')
        .filter_map(Scope::parse)
        .collect();

    Ok(KeyContext {
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
        oauth_grant_id: None,
    })
}

/// Run the request as `ctx`, applying a DB key's per-minute rate limit (429
/// with `Retry-After` once exhausted) and reporting it in `X-RateLimit-*`. The
/// master key, the dev bypass and keys without an `rpm_limit` are unlimited.
async fn run_rate_limited(ctx: KeyContext, state: Arc<AppState>, mut request: Request, next: Next) -> Response {
    let limit = ctx.key_id.zip(ctx.rpm_limit);

    // RPM rate limit check
    if let Some((key_id, rpm)) = limit {
        if let Err(retry_after) = state.rate_limiter.try_take(key_id, rpm).await {
            let rpm_str = rpm.to_string();
            let mut resp = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Rate limit exceeded. Retry after {} seconds.", retry_after),
                        "type": "rate_limit_exceeded",
                        "code": "rate_limit_exceeded"
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

    request.extensions_mut().insert(ctx);
    let mut response = next.run(request).await;

    // Attach X-RateLimit-* headers for rate-limited keys
    if let Some((key_id, rpm)) = limit {
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

// ─── Scope middleware ─────────────────────────────────────────────────────────

/// Axum middleware function that checks the `KeyContext` for the required scope.
/// Returns 403 if the scope is missing, 401 if there's no context.
pub async fn check_scope(required: Scope, request: Request, next: Next) -> Response {
    check_any_scope(std::slice::from_ref(&required), request, next).await
}

/// [`check_scope`] for routes several scopes may call: passes when the context
/// holds any of `required`.
pub async fn check_any_scope(required: &[Scope], request: Request, next: Next) -> Response {
    let ctx = request.extensions().get::<KeyContext>().cloned();
    match ctx {
        Some(ctx) if required.iter().any(|s| ctx.scopes.contains(s)) => next.run(request).await,
        Some(_) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": {
                    "message": "Insufficient scope for this endpoint",
                    "type": "forbidden_scope",
                    "code": "forbidden_scope"
                }
            })),
        ).into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": {
                    "message": "Not authenticated",
                    "type": "authentication_error",
                    "code": "authentication_error"
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
                    "code": "csrf_error"
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
                    "code": "authentication_error"
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
                    "code": "csrf_error"
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
                    "code": "forbidden_permission"
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
                    "code": "authentication_error"
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
