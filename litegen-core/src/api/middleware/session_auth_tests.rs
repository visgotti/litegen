/// Tests for session-cookie auth, CSRF middleware, and require_permission.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        extract::Request,
        http::StatusCode,
        middleware::{self, Next},
        response::IntoResponse,
        routing::{get, post},
        Router,
    };
    use tower::ServiceExt;

    use crate::api::middleware::{
        auth_middleware, check_permission, csrf_middleware,
        AppState, KeyContext,
    };
    use crate::auth::permissions::Permission;
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
        async fn delete(&self, _key: &str) -> Result<(), MaterializeError> { Ok(()) }
    }

    async fn build_state_with_db(db: Arc<SqliteDatabase>) -> Arc<AppState> {
        let registry = Arc::new(ProviderRegistry::new());
        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = Arc::new(ProxyRouter::new(registry, cache, config, image_store));
        let materializer = Arc::new(Materializer::new(
            Arc::new(NoopStorage),
            reqwest::Client::new(),
        ));
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("load shipped models"));
        Arc::new(AppState {
            router,
            db,
            master_key: Some("master-key".to_string()),
            registry: cap_registry,
            materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
        })
    }

    async fn build_test_db() -> Arc<SqliteDatabase> {
        Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"))
    }

    async fn seed_user_and_session(
        db: &Arc<SqliteDatabase>,
        role: Role,
    ) -> (User, Session) {
        let user = User {
            id: format!("user-{}", uuid::Uuid::new_v4()),
            email: "test@example.com".to_string(),
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
            csrf_token,
        };
        db.create_session(&sess).await.expect("create session");
        (user, sess)
    }

    fn build_auth_router(state: Arc<AppState>) -> Router {
        async fn noop_handler() -> impl IntoResponse {
            StatusCode::OK
        }
        let state_for_mw = state.clone();
        Router::new()
            .route("/test", get(noop_handler))
            .layer(middleware::from_fn(move |req: Request, next: Next| {
                let s = state_for_mw.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
    }

    fn build_csrf_router(state: Arc<AppState>) -> Router {
        async fn noop_handler() -> impl IntoResponse {
            StatusCode::OK
        }
        let state_for_auth = state.clone();
        let state_for_csrf = state.clone();
        Router::new()
            .route("/test", post(noop_handler))
            .layer(middleware::from_fn_with_state(
                state_for_csrf.clone(),
                csrf_middleware,
            ))
            .layer(middleware::from_fn(move |req: Request, next: Next| {
                let s = state_for_auth.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
    }

    fn build_permission_router(state: Arc<AppState>, perm: Permission) -> Router {
        async fn noop_handler() -> impl IntoResponse {
            StatusCode::OK
        }
        let state_for_auth = state.clone();
        Router::new()
            .route("/test", get(noop_handler))
            .layer(middleware::from_fn(move |req: Request, next: Next| {
                check_permission(perm, req, next)
            }))
            .layer(middleware::from_fn(move |req: Request, next: Next| {
                let s = state_for_auth.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
    }

    // ─── Cookie session auth builds UserContext ───────────────────────────────

    #[tokio::test]
    async fn cookie_session_auth_builds_user_context() {
        let db = build_test_db().await;
        let (_user, sess) = seed_user_and_session(&db, Role::Owner).await;
        let state = build_state_with_db(db).await;

        // Handler that inspects the extension
        async fn ctx_handler(
            axum::extract::Extension(ctx): axum::extract::Extension<KeyContext>,
        ) -> impl IntoResponse {
            if ctx.user.is_some() {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            }
        }

        let state_for_mw = state.clone();
        let app = Router::new()
            .route("/test", get(ctx_handler))
            .layer(middleware::from_fn(move |req: Request, next: Next| {
                let s = state_for_mw.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }));

        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("cookie", format!("litegen_session={}", sess.id))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Cookie auth should build user context");
    }

    // ─── Expired session returns 401 ─────────────────────────────────────────

    #[tokio::test]
    async fn expired_session_returns_401() {
        let db = build_test_db().await;
        let user = User {
            id: format!("user-{}", uuid::Uuid::new_v4()),
            email: "expired@example.com".to_string(),
            password_hash: None,
            role: Role::Member,
            oauth_github_id: None,
            oauth_google_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: true,
        };
        db.create_user(&user).await.unwrap();

        let session_token = generate_session_token();
        let sess = Session {
            id: session_token.clone(),
            user_id: user.id.clone(),
            created_at: chrono::Utc::now() - chrono::Duration::days(10),
            expires_at: chrono::Utc::now() - chrono::Duration::days(1), // expired!
            ip: None,
            user_agent: None,
            csrf_token: generate_csrf_token(),
        };
        db.create_session(&sess).await.unwrap();

        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("cookie", format!("litegen_session={}", session_token))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "Expired session should return 401");
    }

    // ─── CSRF POST without header returns 403 ────────────────────────────────

    #[tokio::test]
    async fn csrf_post_without_header_returns_403() {
        let db = build_test_db().await;
        let (_user, sess) = seed_user_and_session(&db, Role::Member).await;
        let state = build_state_with_db(db).await;
        let app = build_csrf_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/test")
            .header("cookie", format!("litegen_session={}", sess.id))
            // No X-CSRF-Token header
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "POST without CSRF should return 403");
    }

    // ─── CSRF POST with valid token passes ───────────────────────────────────

    #[tokio::test]
    async fn csrf_post_with_valid_token_passes() {
        let db = build_test_db().await;
        let (_user, sess) = seed_user_and_session(&db, Role::Member).await;
        let state = build_state_with_db(db).await;
        let app = build_csrf_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/test")
            .header("cookie", format!("litegen_session={}", sess.id))
            .header("x-csrf-token", &sess.csrf_token)
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "POST with valid CSRF token should pass");
    }

    // ─── CSRF is skipped for Bearer-token requests ────────────────────────────

    #[tokio::test]
    async fn csrf_skipped_for_bearer_auth() {
        let db = build_test_db().await;
        let state = build_state_with_db(db).await;
        let app = build_csrf_router(state);

        // No session cookie, use master key — CSRF should be skipped
        let req = Request::builder()
            .method("POST")
            .uri("/test")
            .header("authorization", "Bearer master-key")
            // No X-CSRF-Token header
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Bearer POST without CSRF should pass");
    }

    // ─── require_permission denies missing perm with 403 ─────────────────────

    #[tokio::test]
    async fn require_permission_denies_missing_perm() {
        let db = build_test_db().await;
        // Viewer doesn't have UserReadAny
        let (_user, sess) = seed_user_and_session(&db, Role::Viewer).await;
        let state = build_state_with_db(db).await;
        let app = build_permission_router(state, Permission::UserReadAny);

        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("cookie", format!("litegen_session={}", sess.id))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "Missing permission should return 403");
    }

    // ─── require_permission allows matching perm ──────────────────────────────

    #[tokio::test]
    async fn require_permission_allows_matching_perm() {
        let db = build_test_db().await;
        // Owner has UserReadAny
        let (_user, sess) = seed_user_and_session(&db, Role::Owner).await;
        let state = build_state_with_db(db).await;
        let app = build_permission_router(state, Permission::UserReadAny);

        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("cookie", format!("litegen_session={}", sess.id))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Owner with UserReadAny perm should pass");
    }
}
