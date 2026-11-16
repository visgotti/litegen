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
    create_session_cookies, make_clear_session_cookies, within_grant, AppState, KeyContext, DEFAULT_ORG_ID,
};
use crate::auth::lockout::{is_locked_out, retry_after_seconds};
use crate::auth::password::{
    hash_password_async, verify_dummy_async, verify_password_async, PasswordError,
};
use crate::config::Mode;
use crate::db::{OrgOwner, ResetConfirm};
use crate::types::{Application, Organization, PasswordReset, Role, User};
use crate::management::slug::{default_org_name_from_email, slugify, unique_org_slug};

// ─── Shared helpers ──────────────────────────────────────────────────────────

/// Build a 500 without leaking the underlying error to the (often
/// unauthenticated) caller. The detail — usually a raw sqlx/DB error naming
/// tables/columns/constraints — is logged server-side only; the client gets a
/// generic message under `error_code`.
fn sanitized_500(error_code: &str, detail: impl std::fmt::Display) -> Response {
    tracing::error!(error = %detail, code = error_code, "auth handler internal error");
    error_resp(StatusCode::INTERNAL_SERVER_ERROR, error_code, "internal server error")
}

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

/// 403 response returned by `signup`/`login` when password auth is disabled
/// (`AppState.allow_password == false`). Numeric `code` per the OAuth-only spec.
fn password_disabled_resp() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": {
                "type": "password_auth_disabled",
                "code": "password_auth_disabled",
                "message": "Password authentication is disabled; use Google or GitHub."
            }
        })),
    )
        .into_response()
}

/// Provision a hosted user with an Organization, an owner membership, and a
/// first ("Default") Application, and the account itself when `owner` is
/// [`OrgOwner::New`]. Shared by password signup and OAuth auto-create so both
/// go through one code path.
///
/// `org_name` defaults to the email local-part when `None`/empty. Returns a
/// JSON view of the created org (`{id,name,slug}`). Everything is made
/// together (`create_org_with_owner`), so a failure leaves no part of it.
pub async fn create_org_for_user(
    db: &Arc<dyn crate::db::DatabaseStore>,
    owner: OrgOwner<'_>,
    email: &str,
    org_name: Option<String>,
) -> Result<serde_json::Value, sqlx::Error> {
    let now = chrono::Utc::now();
    let org_name = org_name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| default_org_name_from_email(email));
    let slug = unique_org_slug(db.as_ref(), &slugify(&org_name)).await?;
    let org = Organization {
        id: uuid::Uuid::new_v4().to_string(),
        name: org_name,
        slug,
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    let app = Application {
        id: uuid::Uuid::new_v4().to_string(),
        org_id: org.id.clone(),
        name: "Default".into(),
        slug: "default".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_org_with_owner(&org, &app, owner).await?;
    Ok(json!({ "id": org.id, "name": org.name, "slug": org.slug }))
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
    /// Optional organization name (hosted mode). Defaults to the email local-part.
    #[serde(default)]
    pub org_name: Option<String>,
}

/// POST /v1/auth/signup — Create the first user (owner).
#[utoipa::path(
    post,
    path = "/v1/auth/signup",
    request_body = SignupRequest,
    responses(
        (status = 200, description = "Signed up and session created", body = AuthResponse),
        (status = 400, description = "Bad request (`password_too_short`, `invalid_email`)", body = crate::types::ErrorResponse),
        (status = 403, description = "Owner email mismatch", body = crate::types::ErrorResponse),
        (status = 409, description = "Signup closed (users already exist)", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn signup(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SignupRequest>,
) -> Response {
    if !state.allow_password {
        return password_disabled_resp();
    }

    let email = match crate::auth::email::account_email(&body.email) {
        Ok(email) => email,
        Err(why) => return error_resp(StatusCode::BAD_REQUEST, "invalid_email", why),
    };

    // single_tenant: the first user claims the (single) owner account.
    // hosted: anyone can sign up and gets their own org — skip the gate + owner email.
    if state.mode == Mode::SingleTenant {
        // Only allow when users table is empty
        match state.db.count_users().await {
            Ok(count) if count > 0 => {
                return error_resp(StatusCode::CONFLICT, "signup_closed", "Signup is closed: users already exist");
            }
            Err(e) => {
                return sanitized_500("db_error", e);
            }
            _ => {}
        }

        // If LITEGEN__OWNER_EMAIL is set, only that email can sign up: the
        // deployment's config, read once at start (empty counts as unset), as
        // the start-up check reads it (`auth::email::owner_email_problem`,
        // which refuses one that isn't an address).
        if let Some(required) = state.router.config.owner_email() {
            if required.to_lowercase() != email {
                // Generic (review A5): naming the address told a scanner whom
                // to impersonate.
                return error_resp(StatusCode::FORBIDDEN, "owner_email_required", "Only the deployment's designated owner can claim the owner account");
            }
        }
    }

    // Hosted mode: pre-flight duplicate-email check before any expensive work.
    if state.mode == Mode::Hosted {
        match state.db.get_user_by_email(&email).await {
            Ok(Some(_)) => {
                return error_resp(StatusCode::CONFLICT, "email_taken", "An account with that email already exists");
            }
            Err(e) => {
                return sanitized_500("db_error", e);
            }
            Ok(None) => {}
        }
    }

    let hash = match hash_password_async(body.password.clone()).await {
        Ok(h) => h,
        Err(PasswordError::TooShort) => {
            return error_resp(StatusCode::BAD_REQUEST, "password_too_short", &crate::auth::password::too_short_message());
        }
        Err(e) => {
            return sanitized_500("hash_error", e);
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
        sign_in_epoch: 0,
    };

    let org_view = match state.mode {
        // The deployment's first Owner: the account and its Owner membership
        // of the pre-existing default org (migration 0008), made together
        // and only while no account exists (`create_first_owner`), so two
        // first signups at once make one Owner, and a failed write leaves no
        // account behind a closed signup. The count above only saves a
        // closed signup the password hash.
        Mode::SingleTenant => {
            match state.db.create_first_owner(&user, DEFAULT_ORG_ID).await {
                Ok(true) => {}
                Ok(false) => {
                    return error_resp(StatusCode::CONFLICT, "signup_closed", "Signup is closed: users already exist");
                }
                Err(e) => return sanitized_500("db_error", e),
            }
            // Reflect the default org in the response for forward-compat.
            match state.db.get_organization(DEFAULT_ORG_ID).await {
                Ok(Some(org)) => json!({ "id": org.id, "name": org.name, "slug": org.slug }),
                _ => json!({ "id": DEFAULT_ORG_ID, "name": "Default", "slug": "default" }),
            }
        }
        // The account, its own org, the Owner membership and the first app,
        // made together: a failure part way leaves none of them.
        Mode::Hosted => match create_org_for_user(&state.db, OrgOwner::New(&user), &email, body.org_name.clone()).await {
            Ok(view) => view,
            Err(e) => {
                // Backstop: if a concurrent request raced past the pre-flight check and
                // hit the UNIQUE constraint, return 409 instead of leaking the constraint name.
                let msg = e.to_string().to_lowercase();
                if msg.contains("unique") && msg.contains("email") {
                    return error_resp(StatusCode::CONFLICT, "email_taken", "An account with that email already exists");
                }
                return sanitized_500("db_error", e);
            }
        },
    };

    match create_session_cookies(&state.db, &user.id, None, None).await {
        Ok((_st, _ct, sc, cc)) => {
            let public: PublicUser = user.into();
            let mut resp = (
                StatusCode::OK,
                Json(json!({ "user": public, "org": org_view })),
            )
                .into_response();
            resp.headers_mut().append("set-cookie", sc);
            resp.headers_mut().append("set-cookie", cc);
            resp
        }
        Err(e) => sanitized_500("session_error", e),
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
    if !state.allow_password {
        return password_disabled_resp();
    }

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
    // The account's sign-in epoch, read in the same row as the hash the
    // password is checked against (review round 5, finding 2): a reset, a
    // deactivation or a password change that commits after this read moves
    // it on, and the session this login inserts is refused.
    let begun = user_opt.as_ref().map(|u| u.sign_in_epoch);
    let phc = user_opt.as_ref().and_then(|u| u.password_hash.clone());

    let verified = match phc {
        Some(ref h) => verify_password_async(body.password.clone(), h.clone()).await.unwrap_or(false),
        None => {
            verify_dummy_async(body.password.clone()).await;
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

    match crate::api::middleware::create_session_cookies_at(&state.db, &user.id, None, None, begun.unwrap_or(-1)).await {
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
        Err(e) => sanitized_500("session_error", e),
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

// ─── GET /v1/auth/config ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuthConfigResponse {
    /// Whether email/password signup + login is enabled.
    pub password_enabled: bool,
    /// OAuth providers with both CLIENT_ID and CLIENT_SECRET configured
    /// (e.g. `["github", "google"]`).
    pub providers_enabled: Vec<String>,
    /// Whether self-service signup is open: always in hosted mode; in
    /// single-tenant mode while no account exists and password signup is on
    /// (the first signup makes the Owner).
    pub signup_open: bool,
    /// The shortest password signup, invitations, password changes and
    /// resets accept.
    pub password_min_length: u32,
    /// Where MCP clients connect, when this deployment serves MCP.
    pub mcp: Option<McpEndpoints>,
}

/// The MCP endpoints a deployment serves, for the dashboard's Connect page.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct McpEndpoints {
    /// The AI endpoint: `{public_base_url}/mcp`.
    pub ai_url: String,
    /// The management endpoint, `{public_base_url}/admin/mcp`, when it is served.
    pub admin_url: Option<String>,
    /// Whether clients can sign in with OAuth (ChatGPT and claude.ai need it).
    pub oauth: bool,
}

/// GET /v1/auth/config — Public auth-method discovery for the dashboard.
#[utoipa::path(
    get,
    path = "/v1/auth/config",
    responses(
        (status = 200, description = "Enabled auth methods", body = AuthConfigResponse),
    ),
    tag = "Auth"
)]
pub async fn auth_config(State(state): State<Arc<AppState>>) -> Response {
    let providers_enabled: Vec<String> = state
        .oauth
        .enabled_providers()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let mcp = &state.router.config.mcp;
    // Single-tenant: the first signup, by password (OAuth makes no first
    // account), is open until an account exists. A lookup that fails says
    // closed.
    let signup_open = match state.mode {
        Mode::Hosted => true,
        Mode::SingleTenant => state.allow_password && matches!(state.db.count_users().await, Ok(0)),
    };
    let body = AuthConfigResponse {
        password_enabled: state.allow_password,
        providers_enabled,
        signup_open,
        password_min_length: crate::auth::password::MIN_PASSWORD_LEN as u32,
        mcp: mcp.enabled.then(|| {
            use crate::mcp::{oauth, Surface};
            McpEndpoints {
                ai_url: oauth::discovery::resource_url(&state, Surface::Ai),
                admin_url: mcp.admin_enabled.then(|| oauth::discovery::resource_url(&state, Surface::Admin)),
                oauth: oauth::available(&state),
            }
        }),
    };
    (StatusCode::OK, Json(body)).into_response()
}

// ─── GET /v1/auth/me ──────────────────────────────────────────────────────────

/// `GET /v1/auth/me`: the signed-in person, the organizations they belong to
/// with their role and permissions in each, and where this request acts.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MeResponse {
    pub user: MeUser,
    /// The organizations the person belongs to. A sign-in through OAuth lists
    /// only the one its grant is for.
    pub orgs: Vec<MeOrg>,
    /// The organization this request acts in (`X-Litegen-Org-Id`, else the
    /// person's first; an OAuth sign-in's own). Null when there is none.
    #[schema(required = true)]
    pub active_org: Option<String>,
    /// The app this request acts in (`X-Litegen-App-Id`, else the
    /// organization's first; an OAuth sign-in's own). Null when there is none.
    #[schema(required = true)]
    pub active_app: Option<String>,
}

/// The signed-in account.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MeUser {
    pub id: String,
    pub email: String,
    /// The account's own role. What the person may do in an organization is
    /// their role there: `orgs[].role` and `orgs[].permissions`.
    pub role: String,
}

/// One organization the person belongs to.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MeOrg {
    pub id: String,
    pub name: String,
    /// The person's role in this organization: `owner`, `admin`, `member` or
    /// `viewer`.
    pub role: String,
    /// The permissions this role holds acting in this organization, as the
    /// server checks them on every request (e.g. `org:write`,
    /// `generation:create`), for a client to offer only what will work. The
    /// deployment's own permissions (`system:config`, `cache:clear`,
    /// `user:read:any`, `user:write:any`, `user:delete:any`,
    /// `system:transfer_owner`) are listed only for the default org of a
    /// single-tenant deployment, and in hosted mode never. A sign-in through
    /// OAuth is also held to its scopes.
    pub permissions: Vec<String>,
}

/// GET /v1/auth/me — Return the currently authenticated user.
#[utoipa::path(
    get,
    path = "/v1/auth/me",
    responses(
        (status = 200, description = "The signed-in person, each organization they belong to with their role and permissions there, and the organization and app this request acts in.", body = MeResponse),
        (status = 401, description = "Not authenticated", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn me(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
) -> Response {
    let Some(u) = ctx.user.clone() else {
        return error_resp(StatusCode::UNAUTHORIZED, "not_authenticated", "Not logged in");
    };

    // An app signed in through OAuth sees only the org its grant is for.
    // Each org's permissions are what the person holds acting there, by the
    // rule the checks themselves use (`permissions_held_in`): the role
    // there, the deployment's own permissions only from the default org.
    let orgs: Vec<MeOrg> = state
        .db
        .list_orgs_for_user(&u.user_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|(org, _)| within_grant(&ctx, &org.id))
        .map(|(org, role)| MeOrg {
            permissions: crate::api::middleware::permissions_held_in(state.mode, &ctx, &org.id, role)
                .iter()
                .map(|p| p.as_str().to_string())
                .collect(),
            id: org.id,
            name: org.name,
            role: role.as_str().to_string(),
        })
        .collect();

    let body = MeResponse {
        user: MeUser { id: u.user_id, email: u.email, role: u.role.as_str().to_string() },
        orgs,
        active_org: ctx.org_id.clone(),
        active_app: ctx.app_id.clone(),
    };
    (StatusCode::OK, Json(body)).into_response()
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
    if !state.allow_password {
        return password_disabled_resp();
    }

    // litegen sends no email, so a token made here would reach no one: the
    // request looks nothing up and stores nothing, and answers the same, as
    // fast, for any email. Stored, it would void the link an admin issued
    // (a new link voids the older ones), for anyone who knows the address.
    // A reset link comes from an admin or the master key
    // (`POST /v1/users/{id}/password-reset`).
    if !state.dev.expose_reset_tokens {
        return (StatusCode::OK, Json(json!({ "ok": true }))).into_response();
    }

    // Development only: `dev.expose_reset_tokens` (YAML, or
    // LITEGEN__DEV__EXPOSE_RESET_TOKENS) makes the token and answers with it.
    // Never for a deactivated account: confirming would reactivate it.
    let email = body.email.trim().to_lowercase();
    if let Ok(Some(user)) = state.db.get_user_by_email(&email).await.map(|u| u.filter(|u| u.is_active)) {
        use crate::auth::tokens::generate_session_token;
        let token = generate_session_token();
        let reset = PasswordReset {
            token: token.clone(),
            user_id: user.id,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            used_at: None,
            created_at: chrono::Utc::now(),
            issued_by: None,
        };
        let _ = state.db.create_password_reset(&reset).await;
        return (
            StatusCode::OK,
            Json(json!({ "ok": true, "_dev_token": token })),
        )
            .into_response();
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
        (status = 204, description = "Password updated, and a deactivated account reactivated. In the same step the account's sessions end and its apps' OAuth sign-ins and unredeemed codes are revoked; its API keys keep working."),
        (status = 400, description = "Invalid, expired or already used token (`invalid_token`, `token_expired`), a link whose issuer may no longer issue it (`invalid_token`: no longer active, no longer above the account in every organization, or no longer allowed to reactivate it), or a short password (`password_too_short`).", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn password_reset_confirm(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PasswordResetConfirmBody>,
) -> Response {
    if !state.allow_password {
        return password_disabled_resp();
    }

    let reset = match state.db.get_password_reset(&body.token).await {
        Ok(Some(r)) => r,
        _ => return error_resp(StatusCode::BAD_REQUEST, "invalid_token", "Token not found or invalid"),
    };

    if reset.used_at.is_some() || reset.expires_at < chrono::Utc::now() {
        return error_resp(StatusCode::BAD_REQUEST, "token_expired", "Token already used or expired");
    }

    let hash = match hash_password_async(body.new_password.clone()).await {
        Ok(h) => h,
        Err(PasswordError::TooShort) => {
            return error_resp(StatusCode::BAD_REQUEST, "password_too_short", &crate::auth::password::too_short_message());
        }
        Err(e) => {
            return sanitized_500("hash_error", e);
        }
    };

    // One transaction (review A4, round 3b S1 and S2): the token consumed
    // (the single-use CAS), the password written, a deactivated account
    // reactivated, and every sign-in ended: sessions, OAuth grants with
    // their tokens, unredeemed codes. A failure part way leaves all of it as
    // it was, so the link can be used again, and no 204 leaves a sign-in.
    let (user_id, reactivated, revoked_invitations) = match state.db.confirm_password_reset(&body.token, &hash, state.mode).await {
        Ok(ResetConfirm::Done { user_id, reactivated, revoked_invitations }) => (user_id, reactivated, revoked_invitations),
        Ok(ResetConfirm::Unusable) => return error_resp(StatusCode::BAD_REQUEST, "token_expired", "Token already used or expired"),
        Ok(ResetConfirm::IssuerRevoked) => return error_resp(StatusCode::BAD_REQUEST, "invalid_token", "The link's issuer may no longer issue it"),
        Err(e) => return sanitized_500("db_error", e),
    };
    // API keys keep working; the Keys page (or DELETE /v1/keys/{id}) revokes
    // them.
    audit_reset_confirmed(&state, &user_id, reactivated, &revoked_invitations).await;

    StatusCode::NO_CONTENT.into_response()
}

/// The audit entry for a confirmed reset (review A3): the account acted,
/// through its link; filed under each org it belongs to, or, in none, where
/// `audit_org` files an entry with no org (single-tenant: the default org).
/// `reactivated` says whether it brought a deactivated account back. Each
/// invitation the confirm revoked (`revoked`) gets an `invitation.revoke`
/// entry under its org, by the same actor (review round 5, finding 3).
async fn audit_reset_confirmed(state: &AppState, user_id: &str, reactivated: bool, revoked: &[crate::types::Invitation]) {
    let email = match state.db.get_user_by_id(user_id).await {
        Ok(Some(u)) => u.email,
        _ => user_id.to_string(),
    };
    let orgs: Vec<Option<String>> = match state.db.list_orgs_for_user(user_id).await {
        Ok(orgs) if !orgs.is_empty() => orgs.into_iter().map(|(org, _)| Some(org.id)).collect(),
        _ => vec![crate::api::audit::audit_org(state.mode, None)],
    };
    for org_id in orgs {
        let entry = crate::types::AuditLogEntry {
            id: format!("audit-{}", uuid::Uuid::new_v4()),
            actor_key_id: None,
            actor_label: email.clone(),
            action: "user.password_reset".into(),
            target_type: "user".into(),
            target_id: user_id.to_string(),
            before_json: None,
            after_json: Some(serde_json::json!({ "reactivated": reactivated }).to_string()),
            created_at: chrono::Utc::now(),
            org_id,
        };
        if let Err(e) = state.db.insert_audit_log(&entry).await {
            tracing::warn!(error = %e, "could not audit a confirmed password reset");
        }
    }
    use crate::api::handlers::orgs::{audit_revoked_invitations, Revoker};
    audit_revoked_invitations(state, Revoker::ResetLink { account_email: &email }, revoked, Some("inviter_reset")).await;
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
    use crate::capabilities::CapabilityRegistry;
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

    async fn build_hosted_state() -> Arc<AppState> {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
        let mut state = build_state_with_db(db).await;
        // build_state_with_db returns Arc; mutate before any clones exist.
        Arc::get_mut(&mut state).expect("unique Arc").mode = crate::config::Mode::Hosted;
        state
    }

    /// [`build_state`] with `dev.expose_reset_tokens` on (the config field, as
    /// `LITEGEN__DEV__EXPOSE_RESET_TOKENS` sets it): the self-service reset
    /// request answers with its token as `_dev_token`.
    async fn build_state_exposing_reset_tokens() -> Arc<AppState> {
        let mut state = build_state().await;
        Arc::get_mut(&mut state).expect("unique Arc").dev.expose_reset_tokens = true;
        state
    }

    /// [`build_state`] for a deployment whose config sets `owner_email`
    /// (`LITEGEN__OWNER_EMAIL`), on its own state: no process-wide variable.
    async fn build_state_owned_by(owner_email: &str) -> Arc<AppState> {
        let config = AppConfig { owner_email: Some(owner_email.to_string()), ..AppConfig::default() };
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let router = ProxyRouter::new(Arc::new(ProviderRegistry::new()), cache, Arc::new(config), Arc::new(LocalStore));
        let mut state = build_state().await;
        Arc::get_mut(&mut state).expect("unique Arc").router = Arc::new(router);
        state
    }

    /// Hosted mode with password auth disabled (OAuth-only deployment).
    async fn build_hosted_state_no_password() -> Arc<AppState> {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
        let mut state = build_state_with_db(db).await;
        let s = Arc::get_mut(&mut state).expect("unique Arc");
        s.mode = crate::config::Mode::Hosted;
        s.allow_password = false;
        state
    }

    async fn build_state_with_db(db: Arc<dyn crate::db::DatabaseStore>) -> Arc<AppState> {
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
            allow_password: true,
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

    /// Router that mounts signup/login as unauthenticated routes (matching the
    /// production wiring in handlers/mod.rs, where these bypass the auth +
    /// CSRF middleware). Required for hosted mode, which has no dev bypass.
    fn build_unauth_signup_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/v1/auth/signup", post(signup))
            .route("/v1/auth/login", post(login))
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
            sign_in_epoch: 0,
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

    /// Item 2 (2026-11-15): signup counted the accounts and then made one,
    /// in separate steps, so first signups at once each became the
    /// deployment's Owner. One wins; the others hear that signup is closed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn first_signups_at_once_make_one_owner() {
        let state = build_state().await;
        let app = build_unauth_signup_router(state.clone());
        let signups: Vec<_> = (0..4)
            .map(|i| {
                let app = app.clone();
                tokio::spawn(async move {
                    let req = Request::builder()
                        .method("POST")
                        .uri("/v1/auth/signup")
                        .header("content-type", "application/json")
                        .body(json_body(json!({ "email": format!("first{i}@test.com"), "password": "strongpassword123" })))
                        .unwrap();
                    app.oneshot(req).await.unwrap().status().as_u16()
                })
            })
            .collect();
        let mut statuses = Vec::new();
        for s in signups {
            statuses.push(s.await.unwrap());
        }
        statuses.sort();
        assert_eq!(statuses, [200, 409, 409, 409]);
        assert_eq!(state.db.count_users().await.unwrap(), 1, "one account");
        let owners = state.db.list_org_members(DEFAULT_ORG_ID).await.unwrap().into_iter().filter(|m| m.role == Role::Owner).count();
        assert_eq!(owners, 1, "one Owner");
    }

    /// A first signup whose membership can't be written leaves no account:
    /// before, the account stayed, holding nothing, and signup was closed.
    #[tokio::test]
    async fn a_first_signup_that_fails_leaves_signup_open() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
        let state = build_state_with_db(db.clone()).await;
        let app = build_unauth_signup_router(state.clone());
        let signup = || {
            Request::builder()
                .method("POST")
                .uri("/v1/auth/signup")
                .header("content-type", "application/json")
                .body(json_body(json!({ "email": "first@test.com", "password": "strongpassword123" })))
                .unwrap()
        };
        sqlx::query("ALTER TABLE organization_members RENAME TO organization_members_away").execute(db.pool()).await.unwrap();
        let resp = app.clone().oneshot(signup()).await.unwrap();
        sqlx::query("ALTER TABLE organization_members_away RENAME TO organization_members").execute(db.pool()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(state.db.count_users().await.unwrap(), 0, "no account left behind");
        let resp = app.oneshot(signup()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "signup is still open");
    }

    #[tokio::test]
    async fn signup_requires_owner_email_when_set() {
        let state = build_state_owned_by("allowed@test.com").await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "notallowed@test.com", "password": "strongpassword123" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "Signup should return 403 for non-owner email");
    }

    /// Follow-up item 3: an empty LITEGEN__OWNER_EMAIL (a compose file's
    /// `${VAR:-}`) counts as unset, as the start-up check reads it; before,
    /// it matched no address and closed the first signup to everyone.
    #[tokio::test]
    async fn an_empty_owner_email_limits_nothing() {
        let app = build_unauth_signup_router(build_state_owned_by(" ").await);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "first@test.com", "password": "strongpassword123" })))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Review A5: the owner-email gate refuses everyone else without naming
    /// the address it waits for: a scanner learned whom to impersonate.
    #[tokio::test]
    async fn the_owner_email_gate_names_no_address() {
        let app = build_unauth_signup_router(build_state_owned_by("boss@corp.test").await);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "someone@else.test", "password": "strongpassword123" })))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let text = String::from_utf8_lossy(&axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap()).to_string();
        assert!(!text.contains("boss@corp.test"), "{text}");
    }

    /// A self-service reset token for `email`, through the dev flag.
    async fn dev_reset_token(app: &Router, email: &str) -> String {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": email })))
            .unwrap();
        let bytes = axum::body::to_bytes(app.clone().oneshot(req).await.unwrap().into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["_dev_token"].as_str().expect("a dev token").to_string()
    }

    fn confirm_req(token: &str, new_password: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/confirm")
            .header("content-type", "application/json")
            .body(json_body(json!({ "token": token, "new_password": new_password })))
            .unwrap()
    }

    /// Round 3b, S1: deactivating deletes the account's unused links, so a
    /// link from before can't bring it back; one issued after (by someone who
    /// may reactivate it) reactivates it when confirmed, and the audit entry
    /// says so.
    #[tokio::test]
    async fn a_link_issued_after_deactivation_reactivates_the_account() {
        let state = build_state_exposing_reset_tokens().await;
        create_user_with_password(&state, "gone@test.com", "oldpassword12345").await;
        let app = build_auth_router(state.clone());
        let before = dev_reset_token(&app, "gone@test.com").await;
        let user = state.db.get_user_by_email("gone@test.com").await.unwrap().unwrap();
        state.db.update_user(&user.id, None, Some(false), None).await.unwrap();
        let resp = app.clone().oneshot(confirm_req(&before, "newstrongpassword123")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "a link from before the deactivation is gone");
        assert!(!state.db.get_user_by_id(&user.id).await.unwrap().unwrap().is_active);

        // Issued after it, as POST /v1/users/{id}/password-reset stores one.
        let now = chrono::Utc::now();
        let after = PasswordReset { token: "issued-after".into(), user_id: user.id.clone(), expires_at: now + chrono::Duration::hours(1), used_at: None, created_at: now, issued_by: None };
        state.db.create_password_reset(&after).await.unwrap();
        assert_eq!(app.oneshot(confirm_req("issued-after", "newstrongpassword123")).await.unwrap().status(), StatusCode::NO_CONTENT);
        let back = state.db.get_user_by_id(&user.id).await.unwrap().unwrap();
        assert!(back.is_active, "reactivated");
        assert_ne!(back.password_hash, user.password_hash, "with the new password");
        let filter = crate::types::AuditLogFilter { action: Some("user.password_reset".into()), ..Default::default() };
        let entries = state.db.list_audit_log(&filter, 1, 10).await.unwrap().0;
        let after_json: serde_json::Value = serde_json::from_str(entries.first().and_then(|e| e.after_json.as_deref()).unwrap_or("null")).unwrap();
        assert_eq!(after_json["reactivated"], true, "{entries:?}");
    }

    /// Review round 5, finding 3: a reset (the compromised-account recovery)
    /// revokes the pending invitations the account sent, a stand-in a thief
    /// invited included, in the confirm's transaction, and audits each.
    #[tokio::test]
    async fn a_reset_revokes_the_invitations_the_account_sent_and_audits_them() {
        let state = build_state().await;
        create_user_with_password(&state, "stolen@test.com", "oldpassword12345").await;
        let user = state.db.get_user_by_email("stolen@test.com").await.unwrap().unwrap();
        let now = chrono::Utc::now();
        let stand_in = crate::types::Invitation {
            id: "inv-stand-in".into(),
            email: "stand-in@test.com".into(),
            role: Role::Admin,
            token: "stand-in-token".into(),
            invited_by: Some(user.id.clone()),
            org_id: DEFAULT_ORG_ID.into(),
            expires_at: now + chrono::Duration::days(7),
            used_at: None,
            created_at: now,
        };
        state.db.create_invitation(&stand_in).await.unwrap();
        let link = PasswordReset { token: "recovery".into(), user_id: user.id.clone(), expires_at: now + chrono::Duration::hours(1), used_at: None, created_at: now, issued_by: None };
        state.db.create_password_reset(&link).await.unwrap();
        let app = build_auth_router(state.clone());
        assert_eq!(app.oneshot(confirm_req("recovery", "newstrongpassword123")).await.unwrap().status(), StatusCode::NO_CONTENT);
        assert!(state.db.get_invitation("stand-in-token").await.unwrap().is_none(), "the stand-in's invitation is revoked");
        let filter = crate::types::AuditLogFilter { action: Some("invitation.revoke".into()), ..Default::default() };
        let mut entries = Vec::new();
        for _ in 0..200 {
            entries = state.db.list_audit_log(&filter, 1, 10).await.unwrap().0;
            if !entries.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(entries.iter().map(|e| e.target_id.as_str()).collect::<Vec<_>>(), ["inv-stand-in"], "{entries:?}");
    }

    /// Round 3b, S1: the dev flag's self-service request makes no token for
    /// a deactivated account, so it can't reactivate one.
    #[tokio::test]
    async fn the_dev_reset_request_makes_no_token_for_a_deactivated_account() {
        let state = build_state_exposing_reset_tokens().await;
        create_user_with_password(&state, "off@test.com", "oldpassword12345").await;
        let user = state.db.get_user_by_email("off@test.com").await.unwrap().unwrap();
        state.db.update_user(&user.id, None, Some(false), None).await.unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "off@test.com" })))
            .unwrap();
        let resp = build_auth_router(state.clone()).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert!(body.get("_dev_token").is_none(), "{body}");
    }

    /// Review A4/A3, round 3b S2: a reset ends the account's sessions, its
    /// apps' OAuth sign-ins (MCP) and its codes not yet redeemed, keeps its
    /// API keys (they're revoked on the Keys page), and is in the audit log.
    #[tokio::test]
    async fn a_reset_ends_sessions_and_oauth_sign_ins_keeps_api_keys_and_is_audited() {
        use crate::mcp::oauth::store::Grant;
        let state = build_state_exposing_reset_tokens().await;
        create_user_with_password(&state, "reset@test.com", "oldpassword12345").await;
        let user = state.db.get_user_by_email("reset@test.com").await.unwrap().unwrap();
        let oauth = state.db.mcp_oauth().expect("an OAuth store");
        let now = chrono::Utc::now();
        oauth
            .insert_grant(&Grant {
                id: format!("grant-{}", uuid::Uuid::new_v4().simple()),
                client_id: "lgc-test".into(),
                user_id: user.id.clone(),
                org_id: DEFAULT_ORG_ID.into(),
                app_id: None,
                surface: crate::mcp::Surface::Admin,
                scopes: vec!["admin:read".into()],
                created_at: now,
                last_used_at: None,
                revoked_at: None,
                sign_in_epoch: 0,
            })
            .await
            .unwrap();
        let key = state.db.create_api_key("kept", "hash-kept", "lg-kept", None, None, "generate,read", None).await.unwrap();
        state.db.set_api_key_owner(&key.id, &user.id).await.unwrap();
        // A code approved moments ago, not yet redeemed (round 3b, S2): it
        // would mint a fresh grant after the reset.
        let request = crate::mcp::oauth::store::AuthRequest {
            id: "req-pending".into(),
            client_id: "lgc-test".into(),
            redirect_uri: "http://127.0.0.1/callback".into(),
            state: None,
            code_challenge: "challenge".into(),
            scopes: vec!["admin:read".into()],
            surface: crate::mcp::Surface::Admin,
            created_at: now,
            expires_at: now + chrono::Duration::minutes(5),
            user_id: None,
            org_id: None,
            app_id: None,
            granted_scopes: None,
            code_hash: None,
            decided_at: None,
            consumed_at: None,
            sign_in_epoch: None,
        };
        oauth.insert_request(&request).await.unwrap();
        let approve = crate::mcp::oauth::store::Decision::Approve {
            user_id: user.id.clone(),
            org_id: DEFAULT_ORG_ID.into(),
            app_id: None,
            scopes: vec!["admin:read".into()],
            code_hash: "code-pending".into(),
            code_expires_at: now + chrono::Duration::seconds(60),
            at: now,
            sign_in_epoch: 0,
        };
        assert!(oauth.decide_request("req-pending", approve).await.unwrap());

        let app = build_auth_router(state.clone());
        let token = dev_reset_token(&app, "reset@test.com").await;
        assert_eq!(app.oneshot(confirm_req(&token, "newstrongpassword123")).await.unwrap().status(), StatusCode::NO_CONTENT);

        assert!(oauth.list_grants_for_user(&user.id).await.unwrap().is_empty(), "its OAuth sign-ins end");
        assert!(
            matches!(oauth.consume_code("code-pending", chrono::Utc::now()).await.unwrap(), crate::mcp::oauth::store::ConsumeOutcome::Missing),
            "its unredeemed code can't be redeemed"
        );
        assert!(state.db.get_api_key(&key.id).await.unwrap().is_some_and(|k| k.is_active), "its API keys keep working");
        let filter = crate::types::AuditLogFilter { action: Some("user.password_reset".into()), ..Default::default() };
        let mut entries = Vec::new();
        for _ in 0..200 {
            entries = state.db.list_audit_log(&filter, 1, 10).await.unwrap().0;
            if !entries.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(entries.iter().map(|e| e.target_id.as_str()).collect::<Vec<_>>(), [user.id.as_str()], "{entries:?}");
    }

    /// Review A4 follow-on: a new link voids the account's older ones, so the
    /// unauthenticated self-service request, which reaches no one (litegen
    /// sends no email), must store none: anyone who knows the email could
    /// otherwise void the link an admin just issued, as often as they liked.
    #[tokio::test]
    async fn a_self_service_request_leaves_an_issued_link_working() {
        let state = build_state().await;
        create_user_with_password(&state, "keeps@test.com", "oldpassword12345").await;
        let user = state.db.get_user_by_email("keeps@test.com").await.unwrap().unwrap();
        // The link an admin issued, stored as POST /v1/users/{id}/password-reset stores it.
        let now = chrono::Utc::now();
        let issued = PasswordReset { token: "issued-token".into(), user_id: user.id.clone(), expires_at: now + chrono::Duration::hours(1), used_at: None, created_at: now, issued_by: None };
        state.db.create_password_reset(&issued).await.unwrap();
        let app = build_auth_router(state.clone());
        let asked = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "keeps@test.com" })))
            .unwrap();
        assert_eq!(app.clone().oneshot(asked).await.unwrap().status(), StatusCode::OK);
        assert_eq!(app.oneshot(confirm_req("issued-token", "newstrongpassword123")).await.unwrap().status(), StatusCode::NO_CONTENT, "the issued link still works");
    }

    /// The owner email is each deployment's own setting, read once at start
    /// into its config: two app states in one process decide apart. Read
    /// from the process environment on every request, as it was, one test's
    /// variable leaked into another's signup (a 403 in a full run).
    #[tokio::test]
    async fn each_deployment_keeps_its_own_owner_email() {
        let signup = |email: &str| {
            Request::builder()
                .method("POST")
                .uri("/v1/auth/signup")
                .header("content-type", "application/json")
                .body(json_body(json!({ "email": email, "password": "strongpassword123" })))
                .unwrap()
        };
        let a = build_unauth_signup_router(build_state_owned_by("a@test.com").await);
        let b = build_unauth_signup_router(build_state_owned_by("b@test.com").await);
        assert_eq!(b.clone().oneshot(signup("a@test.com")).await.unwrap().status(), StatusCode::FORBIDDEN, "b's owner is b");
        assert_eq!(a.oneshot(signup("a@test.com")).await.unwrap().status(), StatusCode::OK, "a's owner is a");
        assert_eq!(b.oneshot(signup("b@test.com")).await.unwrap().status(), StatusCode::OK, "b's owner is b");
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

    /// An account's email is its actor label in the audit log, where the
    /// master key's is `master-key`: signup takes only an address, in both
    /// modes, and a refused one makes no account (the first signup stays
    /// open).
    #[tokio::test]
    async fn signup_refuses_an_email_that_is_not_an_address() {
        let signup = |email: &str| {
            Request::builder()
                .method("POST")
                .uri("/v1/auth/signup")
                .header("content-type", "application/json")
                .body(json_body(json!({ "email": email, "password": "strongpassword123" })))
                .unwrap()
        };
        for state in [build_state().await, build_hosted_state().await] {
            let mode = state.mode;
            let app = build_unauth_signup_router(state.clone());
            for email in ["master-key", "two@@example.com", "owner\u{200b}@example.com"] {
                let resp = app.clone().oneshot(signup(email)).await.unwrap();
                assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{mode:?} {email:?}");
                let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
                let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(body["error"]["code"], "invalid_email", "{mode:?} {email:?}: {body}");
            }
            assert_eq!(state.db.count_users().await.unwrap(), 0, "{mode:?}: no account made");
            let resp = app.oneshot(signup(" Owner@Example.com ")).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{mode:?}: an address signs up");
            assert!(state.db.get_user_by_email("owner@example.com").await.unwrap().is_some(), "{mode:?}: stored lower-cased");
        }
    }

    #[tokio::test]
    async fn hosted_signup_creates_org_and_app() {
        let state = build_hosted_state().await;
        let app = build_unauth_signup_router(state.clone());

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({
                "email": "founder@acme.com",
                "password": "strongpassword123",
                "org_name": "Acme"
            })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Hosted signup should return 200");

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["org"]["name"], "Acme");
        assert_eq!(body["org"]["slug"], "acme");
        let org_id = body["org"]["id"].as_str().expect("org id").to_string();
        let user_id = body["user"]["id"].as_str().expect("user id").to_string();

        // Org exists and is named "Acme".
        let org = state.db.get_organization(&org_id).await.unwrap().expect("org persisted");
        assert_eq!(org.name, "Acme");

        // Signer is the org owner.
        let role = state.db.get_membership(&org_id, &user_id).await.unwrap();
        assert_eq!(role, Some(Role::Owner), "Signer should be the org owner");

        // Exactly one application created for the org.
        let apps = state.db.list_apps_for_org(&org_id).await.unwrap();
        assert_eq!(apps.len(), 1, "Should create exactly one default application");
        assert_eq!(apps[0].slug, "default");
    }

    /// Follow-up item 4: a hosted signup is the account, its org, the Owner
    /// membership and the first app, made together. One whose app can't be
    /// written leaves no account and no org; before, the account stayed and
    /// its email was taken.
    #[tokio::test]
    async fn a_hosted_signup_that_fails_leaves_nothing_behind() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
        let mut state = build_state_with_db(db.clone()).await;
        Arc::get_mut(&mut state).expect("unique Arc").mode = crate::config::Mode::Hosted;
        let app = build_unauth_signup_router(state.clone());
        let signup = || {
            Request::builder()
                .method("POST")
                .uri("/v1/auth/signup")
                .header("content-type", "application/json")
                .body(json_body(json!({ "email": "founder@acme.com", "password": "strongpassword123", "org_name": "Acme" })))
                .unwrap()
        };
        sqlx::query("ALTER TABLE applications RENAME TO applications_away").execute(db.pool()).await.unwrap();
        let resp = app.clone().oneshot(signup()).await.unwrap();
        sqlx::query("ALTER TABLE applications_away RENAME TO applications").execute(db.pool()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(state.db.get_user_by_email("founder@acme.com").await.unwrap().is_none(), "no account left behind");
        assert!(state.db.get_org_by_slug("acme").await.unwrap().is_none(), "no org left behind");

        let resp = app.oneshot(signup()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "the email is free to sign up again");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["org"]["slug"], "acme", "and the slug: {body}");
    }

    #[tokio::test]
    async fn hosted_signup_is_not_gated_by_existing_users() {
        let state = build_hosted_state().await;
        // Pre-create a user so count_users() > 0; hosted must still allow signup.
        create_user_with_password(&state, "first@acme.com", "password12345678").await;
        let app = build_unauth_signup_router(state.clone());

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({
                "email": "second@globex.com",
                "password": "strongpassword123"
            })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Hosted signup must not be gated by existing users");

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // org_name omitted → derived from email local-part "second".
        assert_eq!(body["org"]["name"], "second");
    }

    #[tokio::test]
    async fn hosted_signup_duplicate_email_returns_409() {
        let state = build_hosted_state().await;
        let app = build_unauth_signup_router(state.clone());

        // First signup — must succeed.
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({
                "email": "dup@example.com",
                "password": "strongpassword123"
            })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "First signup should return 200");

        // Second signup with the same email — must return 409.
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({
                "email": "dup@example.com",
                "password": "strongpassword123"
            })))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT, "Duplicate email should return 409");

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let body_str = body.to_string().to_lowercase();
        assert!(
            !body_str.contains("constraint"),
            "Response must not leak constraint name, got: {body_str}"
        );
        assert_eq!(body["error"]["code"], "email_taken", "Error code must be email_taken");
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
            sign_in_epoch: 0,
        };
        state.db.create_user(&user).await.unwrap();
    }

    use crate::db::DatabaseStore as _;

    /// A store over SQLite that lets a reset commit right after
    /// `get_user_by_email` reads the account, as one landing between a
    /// login's read of the password hash and anything it reads after would.
    struct ResetAfterTheRead {
        inner: Arc<SqliteDatabase>,
        token: String,
    }

    #[async_trait::async_trait]
    impl crate::db::DatabaseStore for ResetAfterTheRead {
        async fn insert_generation(
            &self, id: &str, key_id: Option<&uuid::Uuid>, model: &str, provider: &str, media_type: &str,
            provider_job_id: Option<&str>, cost_usd: f64, org_id: Option<&str>, app_id: Option<&str>,
        ) -> Result<(), sqlx::Error> {
            self.inner.insert_generation(id, key_id, model, provider, media_type, provider_job_id, cost_usd, org_id, app_id).await
        }
        async fn update_generation_status(
            &self, id: &str, status: &str, progress: i32, result_url: Option<&str>, error: Option<&str>,
            completed_at: Option<chrono::DateTime<chrono::Utc>>,
        ) -> Result<(), sqlx::Error> {
            self.inner.update_generation_status(id, status, progress, result_url, error, completed_at).await
        }
        async fn update_generation_metadata(&self, id: &str, metadata: &serde_json::Value) -> Result<(), sqlx::Error> {
            self.inner.update_generation_metadata(id, metadata).await
        }
        async fn list_generations(&self, key_id: Option<&uuid::Uuid>, page: u32, per_page: u32) -> Result<Vec<crate::types::Generation>, sqlx::Error> {
            self.inner.list_generations(key_id, page, per_page).await
        }
        async fn log_request(
            &self, id: &str, model: &str, provider: &str, status: &str, media_type: &str, cost_usd: f64, latency_ms: i64,
            error: Option<&str>, metadata: Option<&serde_json::Value>, org_id: Option<&str>, app_id: Option<&str>,
        ) -> Result<(), sqlx::Error> {
            self.inner.log_request(id, model, provider, status, media_type, cost_usd, latency_ms, error, metadata, org_id, app_id).await
        }
        async fn update_request_log_status(&self, id: &str, status: &str, error: Option<&str>) -> Result<(), sqlx::Error> {
            self.inner.update_request_log_status(id, status, error).await
        }
        async fn get_request_logs(&self, page: u32, per_page: u32) -> Result<(Vec<crate::types::RequestLog>, u64), sqlx::Error> {
            self.inner.get_request_logs(page, per_page).await
        }
        async fn get_request_logs_filtered(&self, filters: &crate::types::LogFilters, page: u32, per_page: u32) -> Result<(Vec<crate::types::RequestLog>, u64), sqlx::Error> {
            self.inner.get_request_logs_filtered(filters, page, per_page).await
        }
        async fn create_api_key(
            &self, name: &str, key_hash: &str, key_prefix: &str, token_quota: Option<f64>, rpm_limit: Option<u32>, scopes: &str, webhook_url: Option<&str>,
        ) -> Result<crate::types::ApiKey, sqlx::Error> {
            self.inner.create_api_key(name, key_hash, key_prefix, token_quota, rpm_limit, scopes, webhook_url).await
        }
        async fn update_api_key(&self, id: &uuid::Uuid, req: &crate::types::UpdateApiKeyRequest) -> Result<Option<crate::types::ApiKey>, sqlx::Error> {
            self.inner.update_api_key(id, req).await
        }
        async fn atomic_charge_tokens(&self, id: &uuid::Uuid, cost_usd: f64) -> Result<f64, sqlx::Error> {
            self.inner.atomic_charge_tokens(id, cost_usd).await
        }
        async fn get_user_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error> {
            let user = self.inner.get_user_by_email(email).await?;
            self.inner.confirm_password_reset(&self.token, "reset-hash", crate::config::Mode::SingleTenant).await?;
            Ok(user)
        }
        async fn sign_in_epoch(&self, user_id: &str) -> Result<Option<i64>, sqlx::Error> {
            self.inner.sign_in_epoch(user_id).await
        }
        async fn recent_failed_login_attempts(&self, email: &str, since: chrono::DateTime<chrono::Utc>) -> Result<Vec<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
            self.inner.recent_failed_login_attempts(email, since).await
        }
        async fn record_login_attempt(&self, email: &str, success: bool) -> Result<(), sqlx::Error> {
            self.inner.record_login_attempt(email, success).await
        }
        async fn touch_last_login(&self, id: &str) -> Result<(), sqlx::Error> {
            self.inner.touch_last_login(id).await
        }
        async fn create_session(&self, s: &crate::types::Session) -> Result<(), sqlx::Error> {
            self.inner.create_session(s).await
        }
        async fn create_session_at_epoch(&self, s: &crate::types::Session, epoch: i64) -> Result<(), sqlx::Error> {
            self.inner.create_session_at_epoch(s, epoch).await
        }
        async fn get_generation(&self, id: &str) -> Result<Option<crate::types::Generation>, sqlx::Error> {
            self.inner.get_generation(id).await
        }
        async fn list_active_generations(&self, limit: u32) -> Result<Vec<crate::types::Generation>, sqlx::Error> {
            self.inner.list_active_generations(limit).await
        }
        async fn count_generations(&self, key_id: Option<&uuid::Uuid>) -> Result<i64, sqlx::Error> {
            self.inner.count_generations(key_id).await
        }
        async fn cancel_generation(&self, id: &str) -> Result<Option<crate::types::Generation>, sqlx::Error> {
            self.inner.cancel_generation(id).await
        }
        async fn get_api_key(&self, id: &uuid::Uuid) -> Result<Option<crate::types::ApiKey>, sqlx::Error> {
            self.inner.get_api_key(id).await
        }
        async fn lookup_api_key_by_hash(&self, key_hash: &str) -> Result<Option<crate::types::ApiKey>, sqlx::Error> {
            self.inner.lookup_api_key_by_hash(key_hash).await
        }
        async fn validate_api_key(&self, key_hash: &str) -> Result<Option<crate::types::ApiKey>, sqlx::Error> {
            self.inner.validate_api_key(key_hash).await
        }
        async fn list_api_keys(&self) -> Result<Vec<crate::types::ApiKey>, sqlx::Error> {
            self.inner.list_api_keys().await
        }
        async fn revoke_api_key(&self, id: &uuid::Uuid) -> Result<bool, sqlx::Error> {
            self.inner.revoke_api_key(id).await
        }
        async fn get_stats(&self) -> Result<crate::types::ProxyStats, sqlx::Error> {
            self.inner.get_stats().await
        }
    }

    /// Review round 5, finding 2: login takes the account's sign-in epoch
    /// from the row it checks the password hash against. A reset that
    /// commits right after that read (between the two queries login used to
    /// make) leaves the session stamped with the old epoch, so it is refused.
    #[tokio::test]
    async fn a_login_whose_account_is_reset_after_its_read_gets_no_working_session() {
        use crate::auth::password::hash_password;
        let inner = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let user = User {
            id: format!("user-{}", uuid::Uuid::new_v4()),
            email: "racer@test.com".into(),
            password_hash: Some(hash_password("oldpassword12345").unwrap()),
            role: Role::Member,
            oauth_github_id: None,
            oauth_google_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: true,
            sign_in_epoch: 0,
        };
        inner.create_user(&user).await.unwrap();
        let now = chrono::Utc::now();
        inner
            .create_password_reset(&PasswordReset { token: "rescue".into(), user_id: user.id.clone(), expires_at: now + chrono::Duration::hours(1), used_at: None, created_at: now, issued_by: None })
            .await
            .unwrap();
        let state = build_state_with_db(Arc::new(ResetAfterTheRead { inner: inner.clone(), token: "rescue".into() })).await;
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "racer@test.com", "password": "oldpassword12345" })))
            .unwrap();
        let resp = build_auth_router(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "the password checked against the hash read before the reset");
        let session = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|c| c.strip_prefix("litegen_session=").map(|rest| rest.split(';').next().unwrap_or("").to_string()))
            .expect("a session cookie");
        assert!(inner.get_session_with_user(&session).await.unwrap().is_none(), "the session is stamped with the epoch before the reset");
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

        // Verify session exists (stored by its token's hash)
        let stored = crate::auth::tokens::token_hash(session_token);
        let sess = state.db.get_session(&stored).await.unwrap();
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
        let sess = state.db.get_session(&stored).await.unwrap();
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
        let state = build_state_exposing_reset_tokens().await;
        create_user_with_password(&state, "reset@test.com", "password12345678").await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "reset@test.com" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["_dev_token"].as_str().is_some(), "Should expose dev token");
    }

    #[tokio::test]
    async fn reset_confirm_with_valid_token_updates_password() {
        let state = build_state_exposing_reset_tokens().await;
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
        let state = build_state_exposing_reset_tokens().await;
        create_user_with_password(&state, "resetonce@test.com", "oldpassword12345").await;
        let app = build_auth_router(state.clone());

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "resetonce@test.com" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();

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
            is_active: false, // <-- inactive,
            sign_in_epoch: 0,
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
        let state = build_state_exposing_reset_tokens().await;
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

        // Verify session exists (stored by its token's hash)
        let stored = crate::auth::tokens::token_hash(&session_token);
        let sess = state.db.get_session(&stored).await.unwrap();
        assert!(sess.is_some());

        // Request reset
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "revokeall@test.com" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();

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
        let sess = state.db.get_session(&stored).await.unwrap();
        assert!(sess.is_none(), "Session should be deleted after password reset");
    }

    // ── Password auth disabled (OAuth-only) ─────────────────────────────────────

    fn build_config_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/v1/auth/config", get(auth_config))
            .route("/v1/auth/signup", post(signup))
            .route("/v1/auth/login", post(login))
            .with_state(state)
    }

    #[tokio::test]
    async fn password_disabled_blocks_signup_and_login() {
        let state = build_hosted_state_no_password().await;
        let app = build_config_router(state);

        // Signup → 403 password_auth_disabled
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "no@pw.com", "password": "strongpassword123" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "signup must be 403 when password disabled");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["type"], "password_auth_disabled");
        assert_eq!(body["error"]["code"], "password_auth_disabled");

        // Login → 403 password_auth_disabled
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/login")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "no@pw.com", "password": "strongpassword123" })))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "login must be 403 when password disabled");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["type"], "password_auth_disabled");
    }

    #[tokio::test]
    async fn password_reset_disabled_when_password_off() {
        let state = build_hosted_state_no_password().await;
        let app = Router::new()
            .route("/v1/auth/password-reset/request", post(password_reset_request))
            .route("/v1/auth/password-reset/confirm", post(password_reset_confirm))
            .with_state(state);

        // request → 403 password_auth_disabled
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/request")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "no@pw.com" })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "reset request must be 403 when password disabled");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["type"], "password_auth_disabled");
        assert_eq!(body["error"]["code"], "password_auth_disabled");

        // confirm → 403 password_auth_disabled
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/password-reset/confirm")
            .header("content-type", "application/json")
            .body(json_body(json!({ "token": "whatever", "new_password": "strongpassword123" })))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "reset confirm must be 403 when password disabled");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["type"], "password_auth_disabled");
    }

    #[tokio::test]
    async fn password_enabled_by_default_allows_signup() {
        // Default state has allow_password = true → signup must still work.
        let state = build_state().await;
        let app = build_config_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/signup")
            .header("content-type", "application/json")
            .body(json_body(json!({ "email": "owner@default.com", "password": "strongpassword123" })))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "default (password-enabled) signup must work");
    }

    // ── GET /v1/auth/config ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn auth_config_reports_methods() {
        use crate::auth::oauth::{OAuthConfig, ProviderConfig};

        // Hosted, OAuth-only, github configured.
        let mut state = build_hosted_state_no_password().await;
        Arc::get_mut(&mut state).expect("unique Arc").oauth = OAuthConfig {
            github: Some(ProviderConfig {
                client_id: "gh-id".to_string(),
                client_secret: "gh-secret".to_string(),
            }),
            ..Default::default()
        };
        let app = build_config_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/config")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["password_enabled"], false, "password disabled in OAuth-only mode");
        assert_eq!(body["signup_open"], true, "signup open in hosted mode");
        let providers = body["providers_enabled"].as_array().expect("providers array");
        assert!(providers.iter().any(|p| p == "github"), "github should be enabled");
        assert!(!providers.iter().any(|p| p == "google"), "google should not be enabled");
    }

    #[tokio::test]
    async fn auth_config_defaults_password_enabled() {
        // Default single-tenant state, no account yet: password enabled, no
        // providers, the first signup open (follow-up item 2; it said closed).
        let state = build_state().await;
        let app = build_config_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/config")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["password_enabled"], true);
        assert_eq!(body["signup_open"], true, "the first signup is open");
    }

    /// The shortest password the server accepts is served, so the dashboard
    /// stops copying the number (it held 12 in three pages).
    #[tokio::test]
    async fn auth_config_serves_the_shortest_password_accepted() {
        let req = Request::builder().method("GET").uri("/v1/auth/config").body(Body::empty()).unwrap();
        let resp = build_config_router(build_state().await).oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["password_min_length"], json!(crate::auth::password::MIN_PASSWORD_LEN), "{body}");
    }

    /// Follow-up item 2: single-tenant signup is open while no account
    /// exists (the first signup makes the Owner) and password signup is on,
    /// and closed after; `signup_open` said closed throughout.
    #[tokio::test]
    async fn single_tenant_signup_is_open_until_the_first_account() {
        let signup_open = |state: Arc<AppState>| async move {
            let req = Request::builder().method("GET").uri("/v1/auth/config").body(Body::empty()).unwrap();
            let resp = build_config_router(state).oneshot(req).await.unwrap();
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["signup_open"].clone()
        };
        let state = build_state().await;
        assert_eq!(signup_open(state.clone()).await, true, "no account yet");
        create_user_with_password(&state, "first@test.com", "password12345678").await;
        assert_eq!(signup_open(state).await, false, "an account exists");

        let mut no_password = build_state().await;
        Arc::get_mut(&mut no_password).expect("unique Arc").allow_password = false;
        assert_eq!(signup_open(no_password).await, false, "password signup is off: the first account can't be made by signing up");
    }
}
