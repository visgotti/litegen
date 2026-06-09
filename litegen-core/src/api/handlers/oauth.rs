//! OAuth 2.0 handlers for GitHub and Google.
//!
//! All HTTP calls to provider APIs are done with `reqwest` — no oauth2/openidconnect crate
//! is needed for the simple authorization-code flow.  The endpoint base URLs are taken from
//! `AppState.oauth.{github,google}_{authorize,api,token,userinfo}_base` so tests can inject
//! a wiremock server without touching real provider APIs.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::api::handlers::auth_password::create_org_for_user;
use crate::api::middleware::{create_session_cookies, cookie_value, AppState};
use crate::auth::tokens::{constant_time_eq, generate_session_token};
use crate::config::Mode;
use crate::types::{Role, User};

// ─── Shared helpers ───────────────────────────────────────────────────────────

fn error_resp(code: StatusCode, error_code: &str, message: &str) -> Response {
    (
        code,
        Json(json!({
            "error": {
                "code": error_code,
                "message": message,
                "type": "oauth_error"
            }
        })),
    )
        .into_response()
}

/// Error response that also clears the OAuth state cookie to prevent reuse.
/// Use this for ALL error returns inside `github_callback` and `google_callback`.
fn error_resp_clear_state(code: StatusCode, error_code: &str, message: &str) -> Response {
    let mut resp = error_resp(code, error_code, message);
    resp.headers_mut().append("set-cookie", clear_oauth_state_cookie());
    resp
}

/// Build a `Set-Cookie` header value that clears the `litegen_oauth_state` cookie.
fn clear_oauth_state_cookie() -> axum::http::HeaderValue {
    axum::http::HeaderValue::from_static(
        "litegen_oauth_state=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
    )
}

/// Build a `Set-Cookie` header value that sets the `litegen_oauth_state` cookie.
fn make_oauth_state_cookie(token: &str) -> axum::http::HeaderValue {
    let secure = std::env::var("LITEGEN__COOKIE_INSECURE_DEV").as_deref() != Ok("true");
    let secure_str = if secure { "; Secure" } else { "" };
    let val = format!(
        "litegen_oauth_state={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600{}",
        token, secure_str
    );
    axum::http::HeaderValue::from_str(&val)
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static(""))
}

/// Build a `Set-Cookie` header value storing the post-login `next` target.
fn make_oauth_next_cookie(next: &str) -> axum::http::HeaderValue {
    let secure = std::env::var("LITEGEN__COOKIE_INSECURE_DEV").as_deref() != Ok("true");
    let secure_str = if secure { "; Secure" } else { "" };
    let val = format!(
        "litegen_oauth_next={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600{}",
        urlencoding::encode(next),
        secure_str
    );
    axum::http::HeaderValue::from_str(&val)
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static(""))
}

/// Build a `Set-Cookie` header value that clears the `litegen_oauth_next` cookie.
fn clear_oauth_next_cookie() -> axum::http::HeaderValue {
    axum::http::HeaderValue::from_static(
        "litegen_oauth_next=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
    )
}

/// Build a `Set-Cookie` header value storing the pending invitation token.
/// The callback reads this cookie raw (no URL-decode), so we write it raw too;
/// invite tokens are URL-safe hex (`generate_session_token`), so no encoding is needed.
fn make_oauth_invite_cookie(token: &str) -> axum::http::HeaderValue {
    let secure = std::env::var("LITEGEN__COOKIE_INSECURE_DEV").as_deref() != Ok("true");
    let secure_str = if secure { "; Secure" } else { "" };
    let val = format!(
        "litegen_oauth_invite={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600{}",
        token, secure_str
    );
    axum::http::HeaderValue::from_str(&val)
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static(""))
}

/// Build a `Set-Cookie` header value that clears the `litegen_oauth_invite` cookie.
fn clear_oauth_invite_cookie() -> axum::http::HeaderValue {
    axum::http::HeaderValue::from_static(
        "litegen_oauth_invite=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
    )
}

/// Build a `Set-Cookie` header value recording which provider (`github`/`google`)
/// initiated the flow, so the unified `/auth/redirect` callback can dispatch.
fn make_oauth_provider_cookie(provider: &str) -> axum::http::HeaderValue {
    let secure = std::env::var("LITEGEN__COOKIE_INSECURE_DEV").as_deref() != Ok("true");
    let secure_str = if secure { "; Secure" } else { "" };
    let val = format!(
        "litegen_oauth_provider={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600{}",
        provider, secure_str
    );
    axum::http::HeaderValue::from_str(&val)
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static(""))
}

/// Build a `Set-Cookie` header value that clears the `litegen_oauth_provider` cookie.
fn clear_oauth_provider_cookie() -> axum::http::HeaderValue {
    axum::http::HeaderValue::from_static(
        "litegen_oauth_provider=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
    )
}

/// Resolve the post-login redirect target. Reads the `litegen_oauth_next`
/// cookie set at `start`; only accepts same-origin relative paths (must start
/// with `/`, not `//`, and contain no backslash — browsers normalize `\` to
/// `/`, so `/\evil.com` would otherwise escape the origin), defaulting to `/`.
fn resolve_next_target(headers: &HeaderMap) -> String {
    let raw = cookie_value(headers, "litegen_oauth_next")
        .and_then(|v| urlencoding::decode(&v).ok().map(|c| c.into_owned()));
    match raw {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.contains('\\') => n,
        _ => "/".to_string(),
    }
}

/// Resolve an OAuth identity to a `User`, auto-creating in hosted mode.
///
/// Order:
///  1. existing user by `(provider, oauth_id)` → use as-is.
///  2. existing user by verified `email` → LINK the oauth id, use it.
///  3. no user:
///       - hosted: AUTO-CREATE user (no password; role Owner) + org + app.
///       - single_tenant: invite-only → return `None` (caller emits 403).
///
/// `email` MUST already be lowercased by the caller.
/// Returns `Ok(Some(user))` on success, `Ok(None)` for the single-tenant
/// invite-only rejection, and `Err` on a DB failure.
async fn resolve_or_create_user(
    state: &Arc<AppState>,
    provider: &str,
    oauth_id: &str,
    email: &str,
) -> Result<Option<User>, sqlx::Error> {
    if let Some(u) = state.db.get_user_by_oauth(provider, oauth_id).await? {
        return Ok(Some(u));
    }
    if let Some(u) = state.db.get_user_by_email(email).await? {
        // Same email created via another provider (or password) → link this id.
        state.db.link_oauth(&u.id, provider, oauth_id).await?;
        return Ok(Some(u));
    }
    if state.mode != Mode::Hosted {
        // single_tenant is invite-only: an admin must create the account first.
        return Ok(None);
    }

    // Hosted auto-create: new owner user with the matching oauth id.
    let now = chrono::Utc::now();
    let user = User {
        id: format!("user-{}", uuid::Uuid::new_v4()),
        email: email.to_string(),
        password_hash: None,
        role: Role::Owner,
        oauth_github_id: if provider == "github" { Some(oauth_id.to_string()) } else { None },
        oauth_google_id: if provider == "google" { Some(oauth_id.to_string()) } else { None },
        created_at: now,
        updated_at: now,
        last_login_at: None,
        is_active: true,
    };
    state.db.create_user(&user).await?;
    // Provision org + owner membership + first app (shared with password signup).
    create_org_for_user(&state.db, &user.id, email, None).await?;
    Ok(Some(user))
}

/// Create a session for `user_id`, set the session + csrf cookies, clear the
/// OAuth state + next cookies, and redirect (302) to the resolved `next` target.
async fn finish_oauth_login(state: &Arc<AppState>, user_id: &str, headers: &HeaderMap) -> Response {
    let next = resolve_next_target(headers);
    match create_session_cookies(&state.db, user_id, None, None).await {
        Ok((_st, _ct, sc, cc)) => {
            let mut resp = StatusCode::FOUND.into_response();
            resp.headers_mut().insert(
                "location",
                axum::http::HeaderValue::from_str(&next)
                    .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/")),
            );
            resp.headers_mut().append("set-cookie", sc);
            resp.headers_mut().append("set-cookie", cc);
            resp.headers_mut().append("set-cookie", clear_oauth_state_cookie());
            resp.headers_mut().append("set-cookie", clear_oauth_next_cookie());
            resp.headers_mut().append("set-cookie", clear_oauth_provider_cookie());
            resp.headers_mut().append("set-cookie", clear_oauth_invite_cookie());
            resp
        }
        Err(e) => error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "session_error", &e.to_string()),
    }
}

/// 302 back to the AcceptInvite SPA page with an error code, clearing all OAuth
/// round-trip cookies (incl. the invite cookie).
fn invite_error_redirect(token: &str, code: &str) -> Response {
    let location = format!("/invite/{}?invite_error={}", urlencoding::encode(token), code);
    let mut resp = StatusCode::FOUND.into_response();
    resp.headers_mut().insert(
        "location",
        axum::http::HeaderValue::from_str(&location)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/")),
    );
    resp.headers_mut().append("set-cookie", clear_oauth_state_cookie());
    resp.headers_mut().append("set-cookie", clear_oauth_provider_cookie());
    resp.headers_mut().append("set-cookie", clear_oauth_next_cookie());
    resp.headers_mut().append("set-cookie", clear_oauth_invite_cookie());
    resp
}

/// Apply an invitation during an OAuth callback. On success the returned user is
/// created/linked, added to the invited org, and the invite is consumed — caller
/// then mints a session. On any invite failure returns a 302 redirect (Err).
async fn apply_invitation_oauth(
    state: &Arc<AppState>,
    provider: &str,
    oauth_id: &str,
    email: &str,
    invite_token: &str,
) -> Result<String, Response> {
    let inv = match state.db.get_invitation(invite_token).await {
        Ok(Some(i)) => i,
        Ok(None) => return Err(invite_error_redirect(invite_token, "invitation_invalid")),
        Err(e) => return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string())),
    };
    if inv.used_at.is_some() || inv.expires_at < chrono::Utc::now() {
        return Err(invite_error_redirect(invite_token, "invitation_invalid"));
    }
    // Strict verified-email match (both already lowercased by the caller / storage).
    if inv.email.to_lowercase() != email {
        return Err(invite_error_redirect(invite_token, "email_mismatch"));
    }

    // Resolve the user without auto-creating a fresh org.
    let user = match state.db.get_user_by_oauth(provider, oauth_id).await {
        Ok(Some(u)) => u,
        Ok(None) => match state.db.get_user_by_email(email).await {
            Ok(Some(u)) => {
                let _ = state.db.link_oauth(&u.id, provider, oauth_id).await;
                u
            }
            Ok(None) => {
                let now = chrono::Utc::now();
                let u = User {
                    id: format!("user-{}", uuid::Uuid::new_v4()),
                    email: email.to_string(),
                    password_hash: None,
                    role: inv.role,
                    oauth_github_id: if provider == "github" { Some(oauth_id.to_string()) } else { None },
                    oauth_google_id: if provider == "google" { Some(oauth_id.to_string()) } else { None },
                    created_at: now,
                    updated_at: now,
                    last_login_at: None,
                    is_active: true,
                };
                if let Err(e) = state.db.create_user(&u).await {
                    return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()));
                }
                u
            }
            Err(e) => return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string())),
        },
        Err(e) => return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string())),
    };

    if !user.is_active {
        return Err(invite_error_redirect(invite_token, "account_inactive"));
    }

    // Add membership (idempotent).
    if state.db.get_membership(&inv.org_id, &user.id).await.ok().flatten().is_none() {
        if let Err(e) = state.db.add_org_member(&inv.org_id, &user.id, inv.role).await {
            return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string()));
        }
    }

    // Atomically consume — if we lost a race, treat as already used.
    match state.db.mark_invitation_used(invite_token).await {
        Ok(true) => {}
        Ok(false) => return Err(invite_error_redirect(invite_token, "invitation_invalid")),
        Err(e) => return Err(error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "db_error", &e.to_string())),
    }

    Ok(user.id)
}

// ─── Query params ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct StartParams {
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub invite: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    pub code: String,
    pub state: String,
}

// ─── GitHub ───────────────────────────────────────────────────────────────────

/// GET /v1/auth/oauth/github/start — Redirect to GitHub OAuth authorize page.
#[utoipa::path(
    get,
    path = "/v1/auth/oauth/github/start",
    responses(
        (status = 302, description = "Redirect to GitHub authorize page"),
        (status = 404, description = "GitHub OAuth not configured", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn github_start(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StartParams>,
) -> Response {
    let Some(cfg) = &state.oauth.github else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": {"code": "oauth_not_configured", "message": "GitHub OAuth is not configured"}}))).into_response();
    };

    let oauth_state = generate_session_token();
    let callback_base = state.oauth.callback_base.as_deref().unwrap_or("");
    // Unified callback: both providers redirect to `{base}/auth/redirect`; the
    // `litegen_oauth_provider` cookie tells the callback which provider it is.
    let redirect_uri = format!("{}/auth/redirect", callback_base);

    let authorize_base = state
        .oauth
        .github_authorize_base
        .as_deref()
        .unwrap_or("https://github.com");
    let url = format!(
        "{}/login/oauth/authorize?client_id={}&redirect_uri={}&scope=user:email&state={}",
        authorize_base,
        urlencoding::encode(&cfg.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&oauth_state),
    );

    let mut resp = StatusCode::FOUND.into_response();
    resp.headers_mut().insert(
        "location",
        axum::http::HeaderValue::from_str(&url)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/")),
    );
    resp.headers_mut()
        .append("set-cookie", make_oauth_state_cookie(&oauth_state));
    resp.headers_mut()
        .append("set-cookie", make_oauth_provider_cookie("github"));
    if let Some(next) = params
        .next
        .as_deref()
        .filter(|n| n.starts_with('/') && !n.starts_with("//") && !n.contains('\\'))
    {
        resp.headers_mut()
            .append("set-cookie", make_oauth_next_cookie(next));
    }
    if let Some(invite) = params.invite.as_deref().filter(|t| !t.is_empty()) {
        resp.headers_mut()
            .append("set-cookie", make_oauth_invite_cookie(invite));
    }
    resp
}

/// GET /v1/auth/oauth/github/callback — Handle GitHub OAuth callback.
#[utoipa::path(
    get,
    path = "/v1/auth/oauth/github/callback",
    params(
        ("code" = String, Query, description = "Authorization code from GitHub"),
        ("state" = String, Query, description = "State for CSRF verification"),
    ),
    responses(
        (status = 302, description = "Redirect to app after successful auth"),
        (status = 400, description = "State mismatch or no verified email", body = crate::types::ErrorResponse),
        (status = 403, description = "Account not found or inactive", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn github_callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Response {
    handle_github_callback(&state, &headers, params).await
}

/// Core GitHub OAuth callback logic, shared by the legacy
/// `/v1/auth/oauth/github/callback` route and the unified `/auth/redirect`
/// dispatcher.
pub async fn handle_github_callback(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    params: CallbackParams,
) -> Response {
    // 1. Verify state cookie
    let Some(cookie_state) = cookie_value(headers, "litegen_oauth_state") else {
        return error_resp_clear_state(StatusCode::BAD_REQUEST, "state_missing", "OAuth state cookie missing");
    };
    if !constant_time_eq(&cookie_state, &params.state) {
        return error_resp_clear_state(StatusCode::BAD_REQUEST, "state_mismatch", "OAuth state mismatch");
    }

    let Some(cfg) = &state.oauth.github else {
        return error_resp_clear_state(StatusCode::NOT_FOUND, "oauth_not_configured", "GitHub OAuth not configured");
    };

    // 2. Exchange code for access token
    let token_base = state
        .oauth
        .github_token_base
        .as_deref()
        .unwrap_or("https://github.com");
    let token_url = format!("{}/login/oauth/access_token", token_base);
    let client = reqwest::Client::new();

    #[derive(Deserialize)]
    struct GithubTokenResponse {
        access_token: Option<String>,
        #[allow(dead_code)]
        token_type: Option<String>,
        #[allow(dead_code)]
        scope: Option<String>,
        error: Option<String>,
    }

    let token_resp = client
        .post(&token_url)
        .header("accept", "application/json")
        .json(&json!({
            "client_id": cfg.client_id,
            "client_secret": cfg.client_secret,
            "code": params.code,
        }))
        .send()
        .await;

    let token_resp = match token_resp {
        Ok(r) => r,
        Err(_) => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "Failed to reach GitHub token endpoint"),
    };

    let token_data: GithubTokenResponse = match token_resp.json().await {
        Ok(d) => d,
        Err(_) => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "Failed to parse GitHub token response"),
    };

    if let Some(err) = token_data.error {
        return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", &format!("GitHub token error: {}", err));
    }

    let access_token = match token_data.access_token {
        Some(t) if !t.is_empty() => t,
        _ => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "No access_token in GitHub response"),
    };

    // 3. Fetch GitHub user profile
    let api_base = state
        .oauth
        .github_api_base
        .as_deref()
        .unwrap_or("https://api.github.com");

    #[derive(Deserialize)]
    struct GithubUser {
        id: serde_json::Value, // number in JSON
        #[allow(dead_code)]
        login: String,
    }

    let user_resp = client
        .get(format!("{}/user", api_base))
        .header("authorization", format!("Bearer {}", access_token))
        .header("user-agent", "litegen")
        .header("accept", "application/json")
        .send()
        .await;

    let gh_user: GithubUser = match user_resp {
        Ok(r) => match r.json().await {
            Ok(u) => u,
            Err(_) => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "Failed to parse GitHub user"),
        },
        Err(_) => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "Failed to fetch GitHub user"),
    };

    // GitHub returns id as a JSON number — convert to string for storage
    let gh_id = match &gh_user.id {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        _ => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "Unexpected GitHub user id format"),
    };

    // 4. Fetch emails
    #[derive(Deserialize)]
    struct GithubEmail {
        email: String,
        primary: bool,
        verified: bool,
    }

    let emails_resp = client
        .get(format!("{}/user/emails", api_base))
        .header("authorization", format!("Bearer {}", access_token))
        .header("user-agent", "litegen")
        .header("accept", "application/json")
        .send()
        .await;

    let emails: Vec<GithubEmail> = match emails_resp {
        Ok(r) => match r.json().await {
            Ok(e) => e,
            Err(_) => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "Failed to parse GitHub emails"),
        },
        Err(_) => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "Failed to fetch GitHub emails"),
    };

    let primary_email = emails
        .into_iter()
        .find(|e| e.primary && e.verified)
        .map(|e| e.email.to_lowercase());

    let Some(email) = primary_email else {
        return error_resp_clear_state(StatusCode::BAD_REQUEST, "no_verified_email", "No verified primary email on GitHub");
    };

    // Invitation-aware path: an invite cookie means "join the inviter's org",
    // not the normal resolve/auto-create flow.
    if let Some(invite_token) = cookie_value(headers, "litegen_oauth_invite") {
        return match apply_invitation_oauth(state, "github", &gh_id, &email, &invite_token).await {
            Ok(user_id) => finish_oauth_login(state, &user_id, headers).await,
            Err(resp) => resp,
        };
    }

    // 5. Resolve the user: existing (by oauth id / linked email) or, in hosted
    //    mode, auto-create account + org + first app. single_tenant stays
    //    invite-only (403 account_not_invited).
    let user = match resolve_or_create_user(state, "github", &gh_id, &email).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return error_resp_clear_state(
                StatusCode::FORBIDDEN,
                "account_not_invited",
                "No account exists for this email. Ask an admin to invite you.",
            );
        }
        Err(e) => return error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "session_error", &e.to_string()),
    };

    if !user.is_active {
        return error_resp_clear_state(StatusCode::FORBIDDEN, "account_inactive", "Account is inactive");
    }

    let _ = state.db.touch_last_login(&user.id).await;

    // 6. Create session
    finish_oauth_login(state, &user.id, headers).await
}

// ─── Google ───────────────────────────────────────────────────────────────────

/// GET /v1/auth/oauth/google/start — Redirect to Google OAuth authorize page.
#[utoipa::path(
    get,
    path = "/v1/auth/oauth/google/start",
    responses(
        (status = 302, description = "Redirect to Google authorize page"),
        (status = 404, description = "Google OAuth not configured", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn google_start(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StartParams>,
) -> Response {
    let Some(cfg) = &state.oauth.google else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": {"code": "oauth_not_configured", "message": "Google OAuth is not configured"}}))).into_response();
    };

    let oauth_state = generate_session_token();
    let callback_base = state.oauth.callback_base.as_deref().unwrap_or("");
    // Unified callback: both providers redirect to `{base}/auth/redirect`; the
    // `litegen_oauth_provider` cookie tells the callback which provider it is.
    let redirect_uri = format!("{}/auth/redirect", callback_base);

    let authorize_base = state
        .oauth
        .google_authorize_base
        .as_deref()
        .unwrap_or("https://accounts.google.com");
    let url = format!(
        "{}/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
        authorize_base,
        urlencoding::encode(&cfg.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("openid email profile"),
        urlencoding::encode(&oauth_state),
    );

    let mut resp = StatusCode::FOUND.into_response();
    resp.headers_mut().insert(
        "location",
        axum::http::HeaderValue::from_str(&url)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("/")),
    );
    resp.headers_mut()
        .append("set-cookie", make_oauth_state_cookie(&oauth_state));
    resp.headers_mut()
        .append("set-cookie", make_oauth_provider_cookie("google"));
    if let Some(next) = params
        .next
        .as_deref()
        .filter(|n| n.starts_with('/') && !n.starts_with("//") && !n.contains('\\'))
    {
        resp.headers_mut()
            .append("set-cookie", make_oauth_next_cookie(next));
    }
    if let Some(invite) = params.invite.as_deref().filter(|t| !t.is_empty()) {
        resp.headers_mut()
            .append("set-cookie", make_oauth_invite_cookie(invite));
    }
    resp
}

/// GET /v1/auth/oauth/google/callback — Handle Google OAuth callback.
#[utoipa::path(
    get,
    path = "/v1/auth/oauth/google/callback",
    params(
        ("code" = String, Query, description = "Authorization code from Google"),
        ("state" = String, Query, description = "State for CSRF verification"),
    ),
    responses(
        (status = 302, description = "Redirect to app after successful auth"),
        (status = 400, description = "State mismatch or unverified email", body = crate::types::ErrorResponse),
        (status = 403, description = "Account not found or inactive", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn google_callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Response {
    handle_google_callback(&state, &headers, params).await
}

/// Core Google OAuth callback logic, shared by the legacy
/// `/v1/auth/oauth/google/callback` route and the unified `/auth/redirect`
/// dispatcher.
pub async fn handle_google_callback(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    params: CallbackParams,
) -> Response {
    // 1. Verify state cookie
    let Some(cookie_state) = cookie_value(headers, "litegen_oauth_state") else {
        return error_resp_clear_state(StatusCode::BAD_REQUEST, "state_missing", "OAuth state cookie missing");
    };
    if !constant_time_eq(&cookie_state, &params.state) {
        return error_resp_clear_state(StatusCode::BAD_REQUEST, "state_mismatch", "OAuth state mismatch");
    }

    let Some(cfg) = &state.oauth.google else {
        return error_resp_clear_state(StatusCode::NOT_FOUND, "oauth_not_configured", "Google OAuth not configured");
    };

    // 2. Exchange code for access token (form-encoded POST)
    let token_base = state
        .oauth
        .google_token_base
        .as_deref()
        .unwrap_or("https://oauth2.googleapis.com");
    let token_url = format!("{}/token", token_base);
    let callback_base = state.oauth.callback_base.as_deref().unwrap_or("");
    // Must match the `redirect_uri` sent at `google_start` (unified callback).
    let redirect_uri = format!("{}/auth/redirect", callback_base);
    let client = reqwest::Client::new();

    #[derive(Deserialize)]
    struct GoogleTokenResponse {
        access_token: Option<String>,
        #[allow(dead_code)]
        id_token: Option<String>,
        #[allow(dead_code)]
        expires_in: Option<i64>,
        #[allow(dead_code)]
        token_type: Option<String>,
        error: Option<String>,
    }

    let token_resp = client
        .post(&token_url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&code={}&client_id={}&client_secret={}&redirect_uri={}",
            urlencoding::encode(&params.code),
            urlencoding::encode(&cfg.client_id),
            urlencoding::encode(&cfg.client_secret),
            urlencoding::encode(&redirect_uri),
        ))
        .send()
        .await;

    let token_resp = match token_resp {
        Ok(r) => r,
        Err(_) => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "Failed to reach Google token endpoint"),
    };

    let token_data: GoogleTokenResponse = match token_resp.json().await {
        Ok(d) => d,
        Err(_) => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "Failed to parse Google token response"),
    };

    if let Some(err) = token_data.error {
        return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", &format!("Google token error: {}", err));
    }

    let access_token = match token_data.access_token {
        Some(t) if !t.is_empty() => t,
        _ => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "No access_token in Google response"),
    };

    // 3. Fetch userinfo
    let userinfo_base = state
        .oauth
        .google_userinfo_base
        .as_deref()
        .unwrap_or("https://openidconnect.googleapis.com");
    let userinfo_url = format!("{}/v1/userinfo", userinfo_base);

    #[derive(Deserialize)]
    struct GoogleUserinfo {
        sub: String,
        email: String,
        email_verified: Option<serde_json::Value>,
        #[allow(dead_code)]
        name: Option<String>,
    }

    let userinfo_resp = client
        .get(&userinfo_url)
        .header("authorization", format!("Bearer {}", access_token))
        .header("accept", "application/json")
        .send()
        .await;

    let userinfo: GoogleUserinfo = match userinfo_resp {
        Ok(r) => match r.json().await {
            Ok(u) => u,
            Err(_) => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "Failed to parse Google userinfo"),
        },
        Err(_) => return error_resp_clear_state(StatusCode::BAD_GATEWAY, "oauth_upstream_error", "Failed to fetch Google userinfo"),
    };

    // email_verified can be bool or "true"/"false" string depending on implementation
    let email_verified = match &userinfo.email_verified {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s == "true",
        _ => false,
    };

    if !email_verified {
        return error_resp_clear_state(StatusCode::BAD_REQUEST, "no_verified_email", "Google email is not verified");
    }

    let google_id = userinfo.sub.clone();
    let email = userinfo.email.to_lowercase();

    // Invitation-aware path: an invite cookie means "join the inviter's org",
    // not the normal resolve/auto-create flow.
    if let Some(invite_token) = cookie_value(headers, "litegen_oauth_invite") {
        return match apply_invitation_oauth(state, "google", &google_id, &email, &invite_token).await {
            Ok(user_id) => finish_oauth_login(state, &user_id, headers).await,
            Err(resp) => resp,
        };
    }

    // 4. Resolve the user: existing (by oauth id / linked email) or, in hosted
    //    mode, auto-create account + org + first app. single_tenant stays
    //    invite-only (403 account_not_invited).
    let user = match resolve_or_create_user(state, "google", &google_id, &email).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return error_resp_clear_state(
                StatusCode::FORBIDDEN,
                "account_not_invited",
                "No account exists for this email. Ask an admin to invite you.",
            );
        }
        Err(e) => return error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "session_error", &e.to_string()),
    };

    if !user.is_active {
        return error_resp_clear_state(StatusCode::FORBIDDEN, "account_inactive", "Account is inactive");
    }

    let _ = state.db.touch_last_login(&user.id).await;

    // 5. Create session
    finish_oauth_login(state, &user.id, headers).await
}

// ─── Unified callback ───────────────────────────────────────────────────────────

/// GET /auth/redirect — Unified OAuth callback for BOTH providers.
///
/// The deployed Google + GitHub OAuth apps share a single redirect URI
/// (`https://app.litegen.ai/api/auth/redirect`, served to the backend as
/// `/auth/redirect` once nginx strips `/api`). We disambiguate the provider via
/// the short-lived `litegen_oauth_provider` cookie set at `*_start`, then
/// dispatch to the matching core callback fn.
#[utoipa::path(
    get,
    path = "/auth/redirect",
    params(
        ("code" = String, Query, description = "Authorization code from the provider"),
        ("state" = String, Query, description = "State for CSRF verification"),
    ),
    responses(
        (status = 302, description = "Redirect to app after successful auth"),
        (status = 400, description = "Missing/invalid provider context or state", body = crate::types::ErrorResponse),
        (status = 403, description = "Account not found or inactive", body = crate::types::ErrorResponse),
    ),
    tag = "Auth"
)]
pub async fn oauth_redirect(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Response {
    match cookie_value(&headers, "litegen_oauth_provider").as_deref() {
        Some("github") => handle_github_callback(&state, &headers, params).await,
        Some("google") => handle_google_callback(&state, &headers, params).await,
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "type": "invalid_oauth_state",
                    "code": 400,
                    "message": "Missing or invalid OAuth provider context"
                }
            })),
        )
            .into_response(),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::Request,
        routing::get,
        Router,
    };
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::auth::oauth::{OAuthConfig, ProviderConfig};
    use crate::capabilities::CapabilityRegistry;
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::db::sqlite::SqliteDatabase;
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::materializer::{Materializer, MaterializeError, TempStorage};
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::LocalStore;
    use crate::types::{Invitation, Role, User};
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

    pub async fn build_state_with_oauth(oauth: OAuthConfig) -> Arc<AppState> {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
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
            master_key: None,
            registry: cap_registry,
            materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
            oauth,
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    /// Same as `build_state_with_oauth` but in hosted mode (enables OAuth
    /// auto-create of account + org on first login).
    pub async fn build_hosted_state_with_oauth(oauth: OAuthConfig) -> Arc<AppState> {
        let mut state = build_state_with_oauth(oauth).await;
        Arc::get_mut(&mut state).expect("unique Arc").mode = crate::config::Mode::Hosted;
        state
    }

    fn build_github_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/v1/auth/oauth/github/start", get(github_start))
            .route("/v1/auth/oauth/github/callback", get(github_callback))
            .with_state(state)
    }

    pub async fn create_user(state: &Arc<AppState>, email: &str, github_id: Option<&str>, google_id: Option<&str>) {
        let user = User {
            id: format!("user-{}", uuid::Uuid::new_v4()),
            email: email.to_string(),
            password_hash: None,
            role: Role::Member,
            oauth_github_id: github_id.map(|s| s.to_string()),
            oauth_google_id: google_id.map(|s| s.to_string()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: true,
        };
        state.db.create_user(&user).await.unwrap();
    }

    // ─── GitHub tests ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn github_callback_state_mismatch_returns_400() {
        let oauth = OAuthConfig {
            github: Some(ProviderConfig {
                client_id: "gh-id".to_string(),
                client_secret: "gh-secret".to_string(),
            }),
            ..Default::default()
        };
        let state = build_state_with_oauth(oauth).await;
        let app = build_github_router(state);

        // Cookie state = "correct_state", query state = "wrong_state"
        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/github/callback?code=someCode&state=wrong_state")
            .header("cookie", "litegen_oauth_state=correct_state")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "state_mismatch");
    }

    #[tokio::test]
    async fn github_callback_unknown_email_returns_403_account_not_invited() {
        let server = MockServer::start().await;

        // Mock token exchange
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "gh-test-token",
                "token_type": "bearer",
                "scope": "user:email"
            })))
            .mount(&server)
            .await;

        // Mock /user
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 12345,
                "login": "alice"
            })))
            .mount(&server)
            .await;

        // Mock /user/emails
        Mock::given(method("GET"))
            .and(path("/user/emails"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"email": "alice@x.com", "primary": true, "verified": true}
            ])))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let oauth = OAuthConfig {
            github: Some(ProviderConfig {
                client_id: "gh-id".to_string(),
                client_secret: "gh-secret".to_string(),
            }),
            github_token_base: Some(server_uri.clone()),
            github_api_base: Some(server_uri.clone()),
            ..Default::default()
        };
        // User is NOT in DB — should return 403 account_not_invited
        let state = build_state_with_oauth(oauth).await;
        let app = build_github_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/github/callback?code=testcode&state=teststate")
            .header("cookie", "litegen_oauth_state=teststate")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "account_not_invited");
    }

    #[tokio::test]
    async fn github_callback_existing_user_creates_session() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "gh-test-token",
                "token_type": "bearer",
                "scope": "user:email"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 12345,
                "login": "alice"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/user/emails"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"email": "alice@x.com", "primary": true, "verified": true}
            ])))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let oauth = OAuthConfig {
            github: Some(ProviderConfig {
                client_id: "gh-id".to_string(),
                client_secret: "gh-secret".to_string(),
            }),
            github_token_base: Some(server_uri.clone()),
            github_api_base: Some(server_uri.clone()),
            ..Default::default()
        };
        let state = build_state_with_oauth(oauth).await;

        // Pre-insert user with oauth_github_id = "12345"
        create_user(&state, "alice@x.com", Some("12345"), None).await;

        let app = build_github_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/github/callback?code=testcode&state=teststate")
            .header("cookie", "litegen_oauth_state=teststate")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        // Check Location: /
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(location, "/");
        // Check Set-Cookie for litegen_session
        let cookies: Vec<_> = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect();
        assert!(
            cookies.iter().any(|c| c.contains("litegen_session=")),
            "Should set session cookie; got: {:?}",
            cookies
        );
    }

    // ─── Google tests ─────────────────────────────────────────────────────────

    fn build_google_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/v1/auth/oauth/google/start", get(google_start))
            .route("/v1/auth/oauth/google/callback", get(google_callback))
            .with_state(state)
    }

    // ─── OAuth invitation-accept helpers ──────────────────────────────────────

    /// Create an inviter + their org, then invite `invitee_email` into it.
    /// Returns the org id the invitee should join.
    async fn seed_org_and_invite(
        state: &Arc<AppState>,
        invitee_email: &str,
        token: &str,
        role: Role,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> String {
        create_user(state, "inviter@x.com", None, Some("inviter-oauth")).await;
        let inviter = state.db.get_user_by_email("inviter@x.com").await.unwrap().unwrap();
        crate::api::handlers::auth_password::create_org_for_user(
            &state.db, &inviter.id, "inviter@x.com", Some("Acme".to_string()),
        ).await.unwrap();
        let org_id = state.db.list_orgs_for_user(&inviter.id).await.unwrap()[0].0.id.clone();
        let inv = Invitation {
            id: format!("inv-{}", uuid::Uuid::new_v4()),
            email: invitee_email.to_string(),
            role,
            token: token.to_string(),
            invited_by: Some(inviter.id.clone()),
            org_id: org_id.clone(),
            expires_at,
            used_at: None,
            created_at: chrono::Utc::now(),
        };
        state.db.create_invitation(&inv).await.unwrap();
        org_id
    }

    /// Mount Google token + userinfo wiremock returning `email` for `sub`.
    async fn google_mock(server: &MockServer, sub: &str, email: &str, verified: bool) -> OAuthConfig {
        Mock::given(method("POST")).and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "g", "id_token": "id", "expires_in": 3600, "token_type": "Bearer"})))
            .mount(server).await;
        Mock::given(method("GET")).and(path("/v1/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sub": sub, "email": email, "email_verified": verified, "name": "N"})))
            .mount(server).await;
        let uri = server.uri();
        OAuthConfig {
            google: Some(ProviderConfig { client_id: "g-id".into(), client_secret: "g-secret".into() }),
            google_token_base: Some(uri.clone()),
            google_userinfo_base: Some(uri),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn oauth_accept_invite_matching_email_joins_invited_org() {
        let server = MockServer::start().await;
        let oauth = google_mock(&server, "alice-sub", "alice@x.com", true).await;
        let state = build_state_with_oauth(oauth).await; // single_tenant: invite path still works
        let org_id = seed_org_and_invite(
            &state, "alice@x.com", "invtok", Role::Member,
            chrono::Utc::now() + chrono::Duration::days(7),
        ).await;
        let app = build_google_router(state.clone());

        let req = Request::builder().method("GET")
            .uri("/v1/auth/oauth/google/callback?code=c&state=s")
            .header("cookie", "litegen_oauth_state=s; litegen_oauth_invite=invtok")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::FOUND);
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string()).collect();
        assert!(cookies.iter().any(|c| c.contains("litegen_session=")), "session set; got {:?}", cookies);

        let alice = state.db.get_user_by_email("alice@x.com").await.unwrap().expect("alice created");
        assert_eq!(state.db.get_membership(&org_id, &alice.id).await.unwrap(), Some(Role::Member));
        let alice_orgs = state.db.list_orgs_for_user(&alice.id).await.unwrap();
        assert_eq!(alice_orgs.len(), 1, "alice is only in the invited org (no fresh org)");
        assert_eq!(alice_orgs[0].0.id, org_id);
        let inv = state.db.get_invitation("invtok").await.unwrap().unwrap();
        assert!(inv.used_at.is_some(), "invitation consumed");
    }

    #[tokio::test]
    async fn oauth_accept_invite_email_mismatch_is_rejected() {
        let server = MockServer::start().await;
        // Invitee signs in as eve@x.com but the invite is for alice@x.com.
        let oauth = google_mock(&server, "eve-sub", "eve@x.com", true).await;
        let state = build_state_with_oauth(oauth).await;
        seed_org_and_invite(&state, "alice@x.com", "invtok", Role::Member,
            chrono::Utc::now() + chrono::Duration::days(7)).await;
        let app = build_google_router(state.clone());

        let req = Request::builder().method("GET")
            .uri("/v1/auth/oauth/google/callback?code=c&state=s")
            .header("cookie", "litegen_oauth_state=s; litegen_oauth_invite=invtok")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::FOUND);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(loc.contains("/invite/invtok") && loc.contains("invite_error=email_mismatch"), "got {}", loc);
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string()).collect();
        assert!(!cookies.iter().any(|c| c.contains("litegen_session=")), "no session on mismatch");
        assert!(state.db.get_user_by_email("eve@x.com").await.unwrap().is_none(), "eve not created");
        let inv = state.db.get_invitation("invtok").await.unwrap().unwrap();
        assert!(inv.used_at.is_none(), "invite not consumed on mismatch");
    }

    #[tokio::test]
    async fn oauth_accept_invite_expired_is_rejected() {
        let server = MockServer::start().await;
        let oauth = google_mock(&server, "alice-sub", "alice@x.com", true).await;
        let state = build_state_with_oauth(oauth).await;
        seed_org_and_invite(&state, "alice@x.com", "invtok", Role::Member,
            chrono::Utc::now() - chrono::Duration::minutes(1)).await; // already expired
        let app = build_google_router(state.clone());

        let req = Request::builder().method("GET")
            .uri("/v1/auth/oauth/google/callback?code=c&state=s")
            .header("cookie", "litegen_oauth_state=s; litegen_oauth_invite=invtok")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(loc.contains("invite_error=invitation_invalid"), "got {}", loc);
        assert!(state.db.get_invitation("invtok").await.unwrap().unwrap().used_at.is_none());
    }

    #[tokio::test]
    async fn oauth_accept_invite_existing_user_joins_no_duplicate() {
        let server = MockServer::start().await;
        let oauth = google_mock(&server, "alice-sub", "alice@x.com", true).await;
        let state = build_state_with_oauth(oauth).await;
        let org_id = seed_org_and_invite(&state, "alice@x.com", "invtok", Role::Admin,
            chrono::Utc::now() + chrono::Duration::days(7)).await;
        // Alice already exists with this google id.
        create_user(&state, "alice@x.com", None, Some("alice-sub")).await;
        let app = build_google_router(state.clone());

        let req = Request::builder().method("GET")
            .uri("/v1/auth/oauth/google/callback?code=c&state=s")
            .header("cookie", "litegen_oauth_state=s; litegen_oauth_invite=invtok")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let alice = state.db.get_user_by_oauth("google", "alice-sub").await.unwrap().unwrap();
        assert_eq!(state.db.get_membership(&org_id, &alice.id).await.unwrap(), Some(Role::Admin));
        assert_eq!(alice.email, "alice@x.com");
    }

    #[tokio::test]
    async fn google_callback_state_mismatch_returns_400() {
        let oauth = OAuthConfig {
            google: Some(ProviderConfig {
                client_id: "g-id".to_string(),
                client_secret: "g-secret".to_string(),
            }),
            ..Default::default()
        };
        let state = build_state_with_oauth(oauth).await;
        let app = build_google_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/google/callback?code=someCode&state=wrong_state")
            .header("cookie", "litegen_oauth_state=correct_state")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "state_mismatch");
    }

    #[tokio::test]
    async fn google_callback_unknown_email_returns_403_account_not_invited() {
        let server = MockServer::start().await;

        // Mock token exchange
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "g-test-token",
                "id_token": "id-tok",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&server)
            .await;

        // Mock userinfo
        Mock::given(method("GET"))
            .and(path("/v1/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sub": "google-sub-12345",
                "email": "alice@x.com",
                "email_verified": true,
                "name": "Alice"
            })))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let oauth = OAuthConfig {
            google: Some(ProviderConfig {
                client_id: "g-id".to_string(),
                client_secret: "g-secret".to_string(),
            }),
            google_token_base: Some(server_uri.clone()),
            google_userinfo_base: Some(server_uri.clone()),
            ..Default::default()
        };
        // User is NOT in DB
        let state = build_state_with_oauth(oauth).await;
        let app = build_google_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/google/callback?code=testcode&state=teststate")
            .header("cookie", "litegen_oauth_state=teststate")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "account_not_invited");
    }

    #[tokio::test]
    async fn google_callback_existing_user_creates_session() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "g-test-token",
                "id_token": "id-tok",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v1/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sub": "google-sub-12345",
                "email": "alice@x.com",
                "email_verified": true,
                "name": "Alice"
            })))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let oauth = OAuthConfig {
            google: Some(ProviderConfig {
                client_id: "g-id".to_string(),
                client_secret: "g-secret".to_string(),
            }),
            google_token_base: Some(server_uri.clone()),
            google_userinfo_base: Some(server_uri.clone()),
            ..Default::default()
        };
        let state = build_state_with_oauth(oauth).await;

        // Pre-insert user with oauth_google_id = "google-sub-12345"
        create_user(&state, "alice@x.com", None, Some("google-sub-12345")).await;

        let app = build_google_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/google/callback?code=testcode&state=teststate")
            .header("cookie", "litegen_oauth_state=teststate")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(location, "/");
        let cookies: Vec<_> = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect();
        assert!(
            cookies.iter().any(|c| c.contains("litegen_session=")),
            "Should set session cookie; got: {:?}",
            cookies
        );
    }

    // ─── Security: OAuth state cookie cleared on error paths ──────────────────

    fn cookies_from_resp(resp: &axum::response::Response) -> Vec<String> {
        resp.headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect()
    }

    fn has_clear_state_cookie(cookies: &[String]) -> bool {
        cookies.iter().any(|c| {
            c.contains("litegen_oauth_state=") && c.contains("Max-Age=0")
        })
    }

    #[tokio::test]
    async fn github_callback_state_mismatch_clears_state_cookie() {
        let oauth = OAuthConfig {
            github: Some(ProviderConfig {
                client_id: "gh-id".to_string(),
                client_secret: "gh-secret".to_string(),
            }),
            ..Default::default()
        };
        let state = build_state_with_oauth(oauth).await;
        let app = build_github_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/github/callback?code=someCode&state=wrong_state")
            .header("cookie", "litegen_oauth_state=correct_state")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let cookies = cookies_from_resp(&resp);
        assert!(
            has_clear_state_cookie(&cookies),
            "State mismatch error must clear state cookie; got: {:?}", cookies
        );
    }

    #[tokio::test]
    async fn github_callback_account_not_invited_clears_state_cookie() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "gh-test-token", "token_type": "bearer", "scope": "user:email"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 99999, "login": "unknown"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/user/emails"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"email": "unknown@nowhere.com", "primary": true, "verified": true}
            ])))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let oauth = OAuthConfig {
            github: Some(ProviderConfig {
                client_id: "gh-id".to_string(),
                client_secret: "gh-secret".to_string(),
            }),
            github_token_base: Some(server_uri.clone()),
            github_api_base: Some(server_uri.clone()),
            ..Default::default()
        };
        let state = build_state_with_oauth(oauth).await;
        let app = build_github_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/github/callback?code=testcode&state=teststate")
            .header("cookie", "litegen_oauth_state=teststate")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let cookies = cookies_from_resp(&resp);
        assert!(
            has_clear_state_cookie(&cookies),
            "account_not_invited error must clear state cookie; got: {:?}", cookies
        );
    }

    #[tokio::test]
    async fn google_callback_state_mismatch_clears_state_cookie() {
        let oauth = OAuthConfig {
            google: Some(ProviderConfig {
                client_id: "g-id".to_string(),
                client_secret: "g-secret".to_string(),
            }),
            ..Default::default()
        };
        let state = build_state_with_oauth(oauth).await;
        let app = build_google_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/google/callback?code=someCode&state=wrong_state")
            .header("cookie", "litegen_oauth_state=correct_state")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let cookies = cookies_from_resp(&resp);
        assert!(
            has_clear_state_cookie(&cookies),
            "Google state mismatch must clear state cookie; got: {:?}", cookies
        );
    }

    #[tokio::test]
    async fn google_callback_account_not_invited_clears_state_cookie() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "g-test-token", "id_token": "id-tok",
                "expires_in": 3600, "token_type": "Bearer"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v1/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sub": "google-sub-unknown",
                "email": "unknown@nowhere.com",
                "email_verified": true,
                "name": "Unknown"
            })))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let oauth = OAuthConfig {
            google: Some(ProviderConfig {
                client_id: "g-id".to_string(),
                client_secret: "g-secret".to_string(),
            }),
            google_token_base: Some(server_uri.clone()),
            google_userinfo_base: Some(server_uri.clone()),
            ..Default::default()
        };
        let state = build_state_with_oauth(oauth).await;
        let app = build_google_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/google/callback?code=testcode&state=teststate")
            .header("cookie", "litegen_oauth_state=teststate")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let cookies = cookies_from_resp(&resp);
        assert!(
            has_clear_state_cookie(&cookies),
            "Google account_not_invited error must clear state cookie; got: {:?}", cookies
        );
    }

    // ─── Hosted auto-create ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn oauth_unknown_user_autocreates_in_hosted_mode() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "gh-test-token", "token_type": "bearer", "scope": "user:email"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 778899, "login": "newbie"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user/emails"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"email": "Newbie@Example.com", "primary": true, "verified": true}
            ])))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let oauth = OAuthConfig {
            github: Some(ProviderConfig {
                client_id: "gh-id".to_string(),
                client_secret: "gh-secret".to_string(),
            }),
            github_token_base: Some(server_uri.clone()),
            github_api_base: Some(server_uri.clone()),
            ..Default::default()
        };
        // Hosted mode — unknown user must be auto-created (no 403).
        let state = build_hosted_state_with_oauth(oauth).await;
        let app = build_github_router(state.clone());

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/github/callback?code=testcode&state=teststate")
            .header("cookie", "litegen_oauth_state=teststate")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND, "auto-create should redirect (200/302)");
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string())
            .collect();
        assert!(
            cookies.iter().any(|c| c.contains("litegen_session=")),
            "should set a session cookie; got: {:?}", cookies
        );

        // Email is lowercased on storage + lookup.
        let user = state.db.get_user_by_email("newbie@example.com").await.unwrap()
            .expect("auto-created user must exist");
        assert_eq!(user.oauth_github_id.as_deref(), Some("778899"));
        assert!(user.password_hash.is_none(), "OAuth user has no password");
        assert_eq!(user.role, Role::Owner);

        // Org + owner membership + first app were provisioned.
        let orgs = state.db.list_orgs_for_user(&user.id).await.unwrap();
        assert_eq!(orgs.len(), 1, "should have created exactly one org");
        let (org, role) = &orgs[0];
        assert_eq!(*role, Role::Owner);
        let apps = state.db.list_apps_for_org(&org.id).await.unwrap();
        assert_eq!(apps.len(), 1, "should create exactly one default application");
        assert_eq!(apps[0].slug, "default");
    }

    #[tokio::test]
    async fn oauth_existing_email_links_id_in_hosted_mode() {
        // A user already exists with this email (e.g. created by another flow);
        // OAuth login must LINK the github id rather than duplicate the account.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "gh-test-token", "token_type": "bearer", "scope": "user:email"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 4242, "login": "linker" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user/emails"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"email": "linker@example.com", "primary": true, "verified": true}
            ])))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let oauth = OAuthConfig {
            github: Some(ProviderConfig {
                client_id: "gh-id".to_string(),
                client_secret: "gh-secret".to_string(),
            }),
            github_token_base: Some(server_uri.clone()),
            github_api_base: Some(server_uri.clone()),
            ..Default::default()
        };
        let state = build_hosted_state_with_oauth(oauth).await;
        // Pre-existing user with this email, no github id.
        create_user(&state, "linker@example.com", None, None).await;

        let app = build_github_router(state.clone());
        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/github/callback?code=testcode&state=teststate")
            .header("cookie", "litegen_oauth_state=teststate")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);

        // Github id is now linked to the existing user; no duplicate created.
        let linked = state.db.get_user_by_oauth("github", "4242").await.unwrap()
            .expect("github id should be linked to the existing user");
        assert_eq!(linked.email, "linker@example.com");
    }

    // ─── Unified /auth/redirect callback ────────────────────────────────────────

    fn build_redirect_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/auth/redirect", get(oauth_redirect))
            .with_state(state)
    }

    /// Mount a GitHub-backed wiremock server (token + /user + /user/emails) and
    /// return its OAuthConfig, reused by the dispatch tests.
    async fn github_mock_oauth(server: &MockServer, gh_id: i64, email: &str) -> OAuthConfig {
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "gh-test-token", "token_type": "bearer", "scope": "user:email"
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": gh_id, "login": "u" })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user/emails"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"email": email, "primary": true, "verified": true}
            ])))
            .mount(server)
            .await;
        let uri = server.uri();
        OAuthConfig {
            github: Some(ProviderConfig {
                client_id: "gh-id".to_string(),
                client_secret: "gh-secret".to_string(),
            }),
            github_token_base: Some(uri.clone()),
            github_api_base: Some(uri),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn oauth_redirect_dispatches_by_provider_cookie() {
        // ── github cookie → github path (hosted auto-create → 302 + user) ──
        let gh_server = MockServer::start().await;
        let gh_oauth = github_mock_oauth(&gh_server, 5150, "gh@example.com").await;
        let gh_state = build_hosted_state_with_oauth(gh_oauth).await;
        let gh_app = build_redirect_router(gh_state.clone());

        let req = Request::builder()
            .method("GET")
            .uri("/auth/redirect?code=testcode&state=teststate")
            .header("cookie", "litegen_oauth_state=teststate; litegen_oauth_provider=github")
            .body(Body::empty())
            .unwrap();
        let resp = gh_app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND, "github dispatch should redirect");
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string()).collect();
        assert!(cookies.iter().any(|c| c.contains("litegen_session=")), "github path should set session; got {:?}", cookies);
        // provider cookie cleared on success
        assert!(
            cookies.iter().any(|c| c.contains("litegen_oauth_provider=") && c.contains("Max-Age=0")),
            "provider cookie must be cleared on success; got {:?}", cookies
        );
        let user = gh_state.db.get_user_by_email("gh@example.com").await.unwrap()
            .expect("github user auto-created via /auth/redirect");
        assert_eq!(user.oauth_github_id.as_deref(), Some("5150"));

        // ── google cookie → google path (hosted auto-create → 302 + user) ──
        let g_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "g-test-token", "id_token": "id-tok", "expires_in": 3600, "token_type": "Bearer"
            })))
            .mount(&g_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sub": "g-sub-9001", "email": "g@example.com", "email_verified": true, "name": "G"
            })))
            .mount(&g_server)
            .await;
        let g_uri = g_server.uri();
        let g_oauth = OAuthConfig {
            google: Some(ProviderConfig {
                client_id: "g-id".to_string(),
                client_secret: "g-secret".to_string(),
            }),
            google_token_base: Some(g_uri.clone()),
            google_userinfo_base: Some(g_uri),
            ..Default::default()
        };
        let g_state = build_hosted_state_with_oauth(g_oauth).await;
        let g_app = build_redirect_router(g_state.clone());

        let req = Request::builder()
            .method("GET")
            .uri("/auth/redirect?code=testcode&state=teststate")
            .header("cookie", "litegen_oauth_state=teststate; litegen_oauth_provider=google")
            .body(Body::empty())
            .unwrap();
        let resp = g_app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND, "google dispatch should redirect");
        let user = g_state.db.get_user_by_email("g@example.com").await.unwrap()
            .expect("google user auto-created via /auth/redirect");
        assert_eq!(user.oauth_google_id.as_deref(), Some("g-sub-9001"));

        // ── no provider cookie → 400 invalid_oauth_state ──
        let n_server = MockServer::start().await;
        let n_oauth = github_mock_oauth(&n_server, 1, "n@example.com").await;
        let n_state = build_hosted_state_with_oauth(n_oauth).await;
        let n_app = build_redirect_router(n_state);
        let req = Request::builder()
            .method("GET")
            .uri("/auth/redirect?code=testcode&state=teststate")
            .header("cookie", "litegen_oauth_state=teststate")
            .body(Body::empty())
            .unwrap();
        let resp = n_app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "missing provider cookie must be 400");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["type"], "invalid_oauth_state");
        assert_eq!(body["error"]["code"], 400);
    }

    #[tokio::test]
    async fn oauth_start_with_invite_sets_invite_cookie() {
        let oauth = OAuthConfig {
            google: Some(ProviderConfig { client_id: "g-id".into(), client_secret: "g-secret".into() }),
            callback_base: Some("https://app.example.com".into()),
            ..Default::default()
        };
        let state = build_state_with_oauth(oauth).await;
        let app = build_google_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/oauth/google/start?invite=invtok123&next=/")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter()
            .map(|v| v.to_str().unwrap_or("").to_string()).collect();
        assert!(
            cookies.iter().any(|c| c.starts_with("litegen_oauth_invite=invtok123")),
            "start must set the invite cookie; got {:?}", cookies
        );
    }

    #[tokio::test]
    async fn next_with_backslash_is_rejected() {
        // A `next` cookie of `/\evil.com` (backslash) must NOT be honored: browsers
        // normalize `\`→`/`, turning it into the external `//evil.com`. The callback
        // must fall back to `/`.
        let server = MockServer::start().await;
        let oauth = github_mock_oauth(&server, 7777, "back@example.com").await;
        let state = build_hosted_state_with_oauth(oauth).await;
        let app = build_redirect_router(state);

        let next = urlencoding::encode("/\\evil.com").into_owned();
        let req = Request::builder()
            .method("GET")
            .uri("/auth/redirect?code=testcode&state=teststate")
            .header(
                "cookie",
                format!(
                    "litegen_oauth_state=teststate; litegen_oauth_provider=github; litegen_oauth_next={}",
                    next
                ),
            )
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(location, "/", "backslash `next` must fall back to `/`, not redirect off-origin");
    }
}
