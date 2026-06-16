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
    use crate::types::{Application, Organization, Role, Session, User};
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
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
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

    // ─── Session user requesting an org they aren't a member of → 403 ────────

    #[tokio::test]
    async fn session_non_member_org_header_403() {
        let db = build_test_db().await;
        // User is created but added to NO organization.
        let (_user, sess) = seed_user_and_session(&db, Role::Owner).await;
        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("cookie", format!("litegen_session={}", sess.id))
            // An org the user is not a member of.
            .header("x-litegen-org-id", "00000000-0000-0000-0000-0000deadbeef")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "Requesting an org the user is not a member of must be 403"
        );
    }

    // ─── App belonging to a different org → 403 ──────────────────────────────

    #[tokio::test]
    async fn session_app_wrong_org_header_403() {
        let db = build_test_db().await;
        let (user, sess) = seed_user_and_session(&db, Role::Owner).await;

        // Create org A and add the user as a member.
        let org_a = Organization {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Org A".to_string(),
            slug: "org-a".to_string(),
            plan: "free".to_string(),
            status: "active".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        db.create_organization(&org_a).await.expect("create org A");
        db.add_org_member(&org_a.id, &user.id, Role::Owner).await.expect("add member to org A");

        // Create org B (user is NOT a member) and an application belonging to org B.
        let org_b = Organization {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Org B".to_string(),
            slug: "org-b".to_string(),
            plan: "free".to_string(),
            status: "active".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        db.create_organization(&org_b).await.expect("create org B");

        let app_b = Application {
            id: uuid::Uuid::new_v4().to_string(),
            org_id: org_b.id.clone(),
            name: "App B".to_string(),
            slug: "app-b".to_string(),
            status: "active".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        db.create_application(&app_b).await.expect("create app B");

        let state = build_state_with_db(db).await;
        let router = build_auth_router(state);

        // Request: session cookie + org A header + app B header (app belongs to org B, not A).
        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("cookie", format!("litegen_session={}", sess.id))
            .header("x-litegen-org-id", &org_a.id)
            .header("x-litegen-app-id", &app_b.id)
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "App belonging to a different org must be 403"
        );
    }

    // ─── CSRF protection on admin/state-changing routes (router assembly) ─────
    //
    // These tests drive the REAL `create_router` so they exercise the actual
    // layer wiring on `generate_routes` / `read_routes` / `admin_routes`, not a
    // synthetic `/test` route. The target is `DELETE /v1/cache` (Admin scope,
    // no request body) — the simplest mutating admin route to drive.

    /// RED → GREEN: a SESSION-authenticated (cookie) request to a mutating admin
    /// route WITHOUT a CSRF token must be rejected with 403. Before the fix the
    /// admin routes had no CSRF layer, so this mutation succeeded (200).
    #[tokio::test]
    async fn session_admin_mutation_without_csrf_is_rejected() {
        let db = build_test_db().await;
        // Owner so there is no doubt about scope/permission — cookie sessions
        // are granted all scopes (incl. Admin) by auth_middleware regardless.
        let (_user, sess) = seed_user_and_session(&db, Role::Owner).await;
        let state = build_state_with_db(db).await;
        let app = crate::api::handlers::create_router(state);

        let req = Request::builder()
            .method("DELETE")
            .uri("/v1/cache")
            .header("cookie", format!("litegen_session={}", sess.id))
            // No X-CSRF-Token header — this is the cross-site forgery scenario.
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "Cookie session admin mutation without CSRF token must be 403"
        );
    }

    /// Regression (a): the SAME cookie request WITH a valid CSRF token succeeds.
    /// Proves legit dashboard mutations still work AND that the RED test above
    /// was not failing on scope/auth (the session does carry Admin scope).
    #[tokio::test]
    async fn session_admin_mutation_with_valid_csrf_succeeds() {
        let db = build_test_db().await;
        let (_user, sess) = seed_user_and_session(&db, Role::Owner).await;
        let state = build_state_with_db(db).await;
        let app = crate::api::handlers::create_router(state);

        let req = Request::builder()
            .method("DELETE")
            .uri("/v1/cache")
            .header("cookie", format!("litegen_session={}", sess.id))
            .header("x-csrf-token", &sess.csrf_token)
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "Cookie session admin mutation WITH valid CSRF token must succeed"
        );
    }

    /// Regression (b) — THE CRITICAL ONE: a Bearer (API-key) request to a
    /// mutating admin route is NOT blocked by CSRF (no session cookie → no
    /// session_id → csrf_middleware no-ops). Proves API-key clients are not
    /// broken by adding the CSRF layer to the admin routes.
    #[tokio::test]
    async fn bearer_admin_mutation_not_blocked_by_csrf() {
        let db = build_test_db().await;
        let state = build_state_with_db(db).await;
        let app = crate::api::handlers::create_router(state);

        let req = Request::builder()
            .method("DELETE")
            .uri("/v1/cache")
            // Master key → all scopes, no session cookie, no CSRF token.
            .header("authorization", "Bearer master-key")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "Bearer/API-key admin mutation must NOT be blocked by CSRF"
        );
    }

    /// Regression (c): a safe cookie GET (no CSRF token) still works — CSRF
    /// middleware no-ops on safe methods (GET/HEAD/OPTIONS).
    #[tokio::test]
    async fn session_safe_get_without_csrf_succeeds() {
        let db = build_test_db().await;
        let (_user, sess) = seed_user_and_session(&db, Role::Owner).await;
        let state = build_state_with_db(db).await;
        let app = crate::api::handlers::create_router(state);

        // GET /v1/keys is an Admin-scope read route; no CSRF token provided.
        let req = Request::builder()
            .method("GET")
            .uri("/v1/keys")
            .header("cookie", format!("litegen_session={}", sess.id))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "Safe cookie GET without CSRF token must still succeed"
        );
    }

    // ─── Hosted mode: the single-tenant dev bypass must be disabled ─────────────
    //
    // The dev bypass (accept any/no credential as full-admin) is gated on
    // `master_key.is_none() && mode == SingleTenant`. In hosted mode it must
    // never fire — a master-key-less hosted deploy fails CLOSED (401).

    async fn build_state_hosted_no_master(db: Arc<SqliteDatabase>) -> Arc<AppState> {
        let registry = Arc::new(ProviderRegistry::new());
        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = Arc::new(ProxyRouter::new(registry, cache, config, image_store));
        let materializer = Arc::new(Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("load shipped models"));
        Arc::new(AppState {
            router,
            db,
            master_key: None,
            registry: cap_registry,
            materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::Hosted,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    /// Seed an active API key with the given scopes/quota and return its raw bearer token + id.
    async fn seed_api_key(
        db: &Arc<SqliteDatabase>,
        scopes: &str,
        token_quota: Option<f64>,
    ) -> (String, uuid::Uuid) {
        let token = format!("sk-test-{}", generate_session_token());
        let hash = crate::auth::secrets::sha256_hex(&token);
        let key = db
            .create_api_key("test-key", &hash, "sk-test", token_quota, None, scopes, None)
            .await
            .expect("create api key");
        (token, key.id)
    }

    #[tokio::test]
    async fn hosted_mode_rejects_bearer_without_real_key() {
        let db = build_test_db().await;
        let state = build_state_hosted_no_master(db).await;
        let app = build_auth_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("authorization", "Bearer anything-goes")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "hosted mode must not honor the dev bypass for an arbitrary Bearer token"
        );
    }

    #[tokio::test]
    async fn hosted_mode_rejects_unauthenticated() {
        let db = build_test_db().await;
        let state = build_state_hosted_no_master(db).await;
        let app = build_auth_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "hosted mode must reject unauthenticated requests (no dev bypass)"
        );
    }

    // ─── API key lifecycle ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn revoked_api_key_is_rejected() {
        let db = build_test_db().await;
        let (token, id) = seed_api_key(&db, "generate,read", None).await;
        // Revoke (sets is_active = false). lookup_api_key_by_hash filters on
        // is_active, so the key must no longer authenticate.
        assert!(db.revoke_api_key(&id).await.unwrap(), "revoke should report success");
        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "a revoked (is_active=false) API key must be rejected"
        );
    }

    #[tokio::test]
    async fn quota_exhausted_api_key_is_rejected_402() {
        let db = build_test_db().await;
        let (token, id) = seed_api_key(&db, "generate,read", Some(1.0)).await;
        // Consume the entire quota → tokens_used (1.0) >= token_quota (1.0).
        db.atomic_charge_tokens(&id, 1.0).await.unwrap();
        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::PAYMENT_REQUIRED,
            "an over-quota API key must be rejected with 402"
        );
    }

    // ─── Scope enforcement (Bearer keys) ──────────────────────────────────────────

    #[tokio::test]
    async fn read_scope_key_cannot_reach_admin_or_generate_routes() {
        let db = build_test_db().await;
        let (token, _id) = seed_api_key(&db, "read", None).await;
        let state = build_state_with_db(db).await;
        let app = crate::api::handlers::create_router(state);

        // Admin-scope route.
        let req = Request::builder()
            .method("POST")
            .uri("/v1/keys")
            .header("authorization", format!("Bearer {}", token))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::FORBIDDEN,
            "a read-scope key must be denied on an Admin-scope route"
        );

        // Generate-scope route.
        let req = Request::builder()
            .method("POST")
            .uri("/v1/images/generations")
            .header("authorization", format!("Bearer {}", token))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::FORBIDDEN,
            "a read-scope key must be denied on a Generate-scope route"
        );
    }

    // ─── Cross-org header: a session cannot act on an org it doesn't belong to ────

    #[tokio::test]
    async fn session_with_foreign_org_header_is_rejected() {
        let db = build_test_db().await;
        let (_user, sess) = seed_user_and_session(&db, Role::Owner).await;
        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("cookie", format!("litegen_session={}", sess.id))
            // An org the session user is not a member of.
            .header("x-litegen-org-id", "00000000-0000-0000-0000-0000000000ff")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "supplying an org id the session user is not a member of must be 403"
        );
    }

    /// A still-valid session for a user whose account has been deactivated
    /// (is_active=false) must be rejected (401), so deactivation takes effect
    /// immediately without waiting for the session to expire.
    #[tokio::test]
    async fn inactive_user_session_returns_401() {
        let db = build_test_db().await;
        let user = User {
            id: format!("user-{}", uuid::Uuid::new_v4()),
            email: "inactive@example.com".to_string(),
            password_hash: None,
            role: Role::Owner,
            oauth_github_id: None,
            oauth_google_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: false,
        };
        db.create_user(&user).await.expect("create user");
        let sess = Session {
            id: generate_session_token(),
            user_id: user.id.clone(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            ip: None,
            user_agent: None,
            csrf_token: generate_csrf_token(),
        };
        db.create_session(&sess).await.expect("create session");
        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("cookie", format!("litegen_session={}", sess.id))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "a deactivated user's session must be rejected with 401"
        );
    }

    /// The unauthenticated config route is reachable without any credential
    /// (it is intentionally outside the auth layer).
    #[tokio::test]
    async fn unauth_config_route_requires_no_auth() {
        let db = build_test_db().await;
        let state = build_state_with_db(db).await;
        let app = crate::api::handlers::create_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/config")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "/v1/auth/config must be reachable with no auth"
        );
    }
}
