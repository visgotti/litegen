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

use crate::api::middleware::{create_session_cookies, cookie_value, AppState};
use crate::auth::tokens::{constant_time_eq, generate_session_token};

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

// ─── Query params ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct StartParams {
    #[serde(default)]
    pub next: Option<String>,
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
    Query(_params): Query<StartParams>,
) -> Response {
    let Some(cfg) = &state.oauth.github else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": {"code": "oauth_not_configured", "message": "GitHub OAuth is not configured"}}))).into_response();
    };

    let oauth_state = generate_session_token();
    let callback_base = state.oauth.callback_base.as_deref().unwrap_or("");
    let redirect_uri = format!("{}/v1/auth/oauth/github/callback", callback_base);

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
    // 1. Verify state cookie
    let Some(cookie_state) = cookie_value(&headers, "litegen_oauth_state") else {
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

    // 5. Look up user by oauth_github_id first, then by email
    let user = if let Ok(Some(u)) = state.db.get_user_by_oauth("github", &gh_id).await {
        u
    } else if let Ok(Some(u)) = state.db.get_user_by_email(&email).await {
        let _ = state.db.link_oauth(&u.id, "github", &gh_id).await;
        u
    } else {
        return error_resp_clear_state(
            StatusCode::FORBIDDEN,
            "account_not_invited",
            "No account exists for this email. Ask an admin to invite you.",
        );
    };

    if !user.is_active {
        return error_resp_clear_state(StatusCode::FORBIDDEN, "account_inactive", "Account is inactive");
    }

    let _ = state.db.touch_last_login(&user.id).await;

    // 6. Create session
    match create_session_cookies(&state.db, &user.id, None, None).await {
        Ok((_st, _ct, sc, cc)) => {
            let mut resp = StatusCode::FOUND.into_response();
            resp.headers_mut()
                .insert("location", axum::http::HeaderValue::from_static("/"));
            resp.headers_mut().append("set-cookie", sc);
            resp.headers_mut().append("set-cookie", cc);
            resp.headers_mut()
                .append("set-cookie", clear_oauth_state_cookie());
            resp
        }
        Err(e) => error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "session_error", &e.to_string()),
    }
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
    Query(_params): Query<StartParams>,
) -> Response {
    let Some(cfg) = &state.oauth.google else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": {"code": "oauth_not_configured", "message": "Google OAuth is not configured"}}))).into_response();
    };

    let oauth_state = generate_session_token();
    let callback_base = state.oauth.callback_base.as_deref().unwrap_or("");
    let redirect_uri = format!("{}/v1/auth/oauth/google/callback", callback_base);

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
    // 1. Verify state cookie
    let Some(cookie_state) = cookie_value(&headers, "litegen_oauth_state") else {
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
    let redirect_uri = format!("{}/v1/auth/oauth/google/callback", callback_base);
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

    // 4. Look up user by oauth_google_id first, then by email
    let user = if let Ok(Some(u)) = state.db.get_user_by_oauth("google", &google_id).await {
        u
    } else if let Ok(Some(u)) = state.db.get_user_by_email(&email).await {
        let _ = state.db.link_oauth(&u.id, "google", &google_id).await;
        u
    } else {
        return error_resp_clear_state(
            StatusCode::FORBIDDEN,
            "account_not_invited",
            "No account exists for this email. Ask an admin to invite you.",
        );
    };

    if !user.is_active {
        return error_resp_clear_state(StatusCode::FORBIDDEN, "account_inactive", "Account is inactive");
    }

    let _ = state.db.touch_last_login(&user.id).await;

    // 5. Create session
    match create_session_cookies(&state.db, &user.id, None, None).await {
        Ok((_st, _ct, sc, cc)) => {
            let mut resp = StatusCode::FOUND.into_response();
            resp.headers_mut()
                .insert("location", axum::http::HeaderValue::from_static("/"));
            resp.headers_mut().append("set-cookie", sc);
            resp.headers_mut().append("set-cookie", cc);
            resp.headers_mut()
                .append("set-cookie", clear_oauth_state_cookie());
            resp
        }
        Err(e) => error_resp_clear_state(StatusCode::INTERNAL_SERVER_ERROR, "session_error", &e.to_string()),
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
    use crate::types::{Role, User};
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
        })
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
}
