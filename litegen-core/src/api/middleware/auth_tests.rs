/// Unit tests for auth_middleware — verifies that requests are accepted/rejected
/// based on the master key configuration.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        extract::Request,
        http::StatusCode,
        middleware::{self, Next},
        response::IntoResponse,
        routing::get,
        Router,
    };
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::api::middleware::{auth_middleware, AppState};
    use crate::capabilities::CapabilityRegistry;
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::db::DatabaseStore;
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::materializer::{Materializer, MaterializeError, TempStorage};
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::LocalStore;
    use crate::types::*;

    // ─── No-op database ────────────────────────────────────────────────────────

    struct NoopDb;

    #[async_trait::async_trait]
    impl DatabaseStore for NoopDb {
        async fn insert_generation(
            &self, _id: &str, _key_id: Option<&uuid::Uuid>, _model: &str, _provider: &str,
            _media_type: &str, _provider_job_id: Option<&str>, _cost_usd: f64,
        ) -> Result<(), sqlx::Error> { Ok(()) }
        async fn update_generation_status(
            &self, _id: &str, _status: &str, _progress: i32, _result_url: Option<&str>,
            _error: Option<&str>, _completed_at: Option<chrono::DateTime<chrono::Utc>>,
        ) -> Result<(), sqlx::Error> { Ok(()) }
        async fn get_generation(&self, _id: &str) -> Result<Option<crate::types::Generation>, sqlx::Error> { Ok(None) }
        async fn list_active_generations(&self, _limit: u32) -> Result<Vec<crate::types::Generation>, sqlx::Error> { Ok(vec![]) }
        async fn list_generations(&self, _key_id: Option<&Uuid>, _page: u32, _per_page: u32) -> Result<Vec<crate::types::Generation>, sqlx::Error> { Ok(vec![]) }
        async fn count_generations(&self, _key_id: Option<&Uuid>) -> Result<i64, sqlx::Error> { Ok(0) }
        async fn cancel_generation(&self, _id: &str) -> Result<Option<crate::types::Generation>, sqlx::Error> { Ok(None) }

        async fn log_request(
            &self,
            _id: &str, _model: &str, _provider: &str, _status: &str,
            _media_type: &str, _cost_usd: f64, _latency_ms: i64,
            _error: Option<&str>, _metadata: Option<&serde_json::Value>,
        ) -> Result<(), sqlx::Error> { Ok(()) }

        async fn get_request_logs(&self, _page: u32, _per_page: u32)
            -> Result<(Vec<RequestLog>, u64), sqlx::Error> { Ok((vec![], 0)) }

        async fn get_request_logs_filtered(&self, _filters: &crate::types::LogFilters, _page: u32, _per_page: u32)
            -> Result<(Vec<RequestLog>, u64), sqlx::Error> { Ok((vec![], 0)) }

        async fn create_api_key(
            &self, name: &str, key_hash: &str, key_prefix: &str,
            _token_quota: Option<f64>, _rpm_limit: Option<u32>, _scopes: &str, _webhook_url: Option<&str>,
        ) -> Result<ApiKey, sqlx::Error> {
            Ok(ApiKey {
                id: Uuid::new_v4(),
                name: name.to_string(),
                key_hash: key_hash.to_string(),
                key_prefix: key_prefix.to_string(),
                created_at: chrono::Utc::now(),
                expires_at: None,
                is_active: true,
                token_quota: None,
                tokens_used: 0.0,
                rpm_limit: None,
                scopes: "generate,read".to_string(),
                webhook_url: None,
                owner_user_id: None,
            })
        }

        async fn get_api_key(&self, _id: &Uuid) -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }

        async fn update_api_key(&self, _id: &Uuid, _req: &crate::types::UpdateApiKeyRequest) -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }

        async fn lookup_api_key_by_hash(&self, _key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }

        async fn atomic_charge_tokens(&self, _id: &Uuid, _cost_usd: f64) -> Result<f64, sqlx::Error> { Ok(0.0) }

        async fn validate_api_key(&self, _key_hash: &str)
            -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }

        async fn list_api_keys(&self) -> Result<Vec<ApiKey>, sqlx::Error> { Ok(vec![]) }

        async fn revoke_api_key(&self, _id: &Uuid) -> Result<bool, sqlx::Error> { Ok(false) }

        async fn get_stats(&self) -> Result<ProxyStats, sqlx::Error> {
            Ok(ProxyStats {
                total_requests: 0, successful_requests: 0, failed_requests: 0,
                total_cost_usd: 0.0, avg_latency_ms: 0.0, requests_per_minute: 0.0,
                models_used: vec![], providers_used: vec![],
                latency_percentiles: LatencyPercentiles { p50_ms: 0.0, p95_ms: 0.0, p99_ms: 0.0, sample_count: 0, window_minutes: 60 },
            })
        }
    }

    // ─── Minimal TempStorage ──────────────────────────────────────────────────

    struct NoopStorage;

    #[async_trait::async_trait]
    impl TempStorage for NoopStorage {
        async fn put(&self, key: &str, _bytes: bytes::Bytes, _ct: &str)
            -> Result<String, MaterializeError> { Ok(format!("local://{}", key)) }
        async fn delete(&self, _key: &str) -> Result<(), MaterializeError> { Ok(()) }
    }

    // ─── Build a minimal AppState with a given master_key ────────────────────

    async fn build_state(master_key: Option<&str>) -> Arc<AppState> {
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
        let cap_registry = Arc::new(
            CapabilityRegistry::from_dir(&p).expect("load shipped models")
        );

        Arc::new(AppState {
            router,
            db: Arc::new(NoopDb),
            master_key: master_key.map(|s| s.to_string()),
            registry: cap_registry,
            materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
        })
    }

    /// Build a test router that wraps auth_middleware via a closure so we can
    /// capture the AppState without needing axum's State extractor.
    fn build_auth_router(state: Arc<AppState>) -> Router {
        async fn noop_handler() -> impl IntoResponse {
            StatusCode::OK
        }

        let state_for_mw = state.clone();
        Router::new()
            .route("/test", get(noop_handler))
            .layer(middleware::from_fn(move |req: Request, next: Next| {
                let state = state_for_mw.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, state, req, next).await
                }
            }))
    }

    // ─── Test: no master key → all requests allowed ───────────────────────────

    #[tokio::test]
    async fn no_master_key_allows_all_requests() {
        let state = build_state(None).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "No master key should allow unauthenticated requests");
    }

    // ─── Test: master key set + no auth header → 401 ──────────────────────────

    #[tokio::test]
    async fn master_key_set_rejects_missing_auth_header() {
        let state = build_state(Some("secret-master-key")).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "Missing auth should be 401");
    }

    // ─── Test: master key set + wrong key → 401 ──────────────────────────────

    #[tokio::test]
    async fn master_key_set_rejects_wrong_key() {
        let state = build_state(Some("correct-key")).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("authorization", "Bearer wrong-key")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "Wrong key should be 401");
    }

    // ─── Test: master key set + correct key → 200 ────────────────────────────

    #[tokio::test]
    async fn master_key_set_accepts_correct_key() {
        let state = build_state(Some("correct-key")).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/test")
            .header("authorization", "Bearer correct-key")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Correct key should be 200");
    }

    // ─── ValidKeyDb: returns a configurable key for lookup_api_key_by_hash ─────

    struct ValidKeyDb {
        key_hash: String,
        scopes: String,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
        is_active: bool,
        token_quota: Option<f64>,
        tokens_used: f64,
        rpm_limit: Option<u32>,
    }

    impl ValidKeyDb {
        fn active(hash: &str, scopes: &str) -> Self {
            Self {
                key_hash: hash.to_string(),
                scopes: scopes.to_string(),
                expires_at: None,
                is_active: true,
                token_quota: None,
                tokens_used: 0.0,
                rpm_limit: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl DatabaseStore for ValidKeyDb {
        async fn insert_generation(
            &self, _id: &str, _key_id: Option<&uuid::Uuid>, _model: &str, _provider: &str,
            _media_type: &str, _provider_job_id: Option<&str>, _cost_usd: f64,
        ) -> Result<(), sqlx::Error> { Ok(()) }
        async fn update_generation_status(
            &self, _id: &str, _status: &str, _progress: i32, _result_url: Option<&str>,
            _error: Option<&str>, _completed_at: Option<chrono::DateTime<chrono::Utc>>,
        ) -> Result<(), sqlx::Error> { Ok(()) }
        async fn get_generation(&self, _id: &str) -> Result<Option<crate::types::Generation>, sqlx::Error> { Ok(None) }
        async fn list_active_generations(&self, _limit: u32) -> Result<Vec<crate::types::Generation>, sqlx::Error> { Ok(vec![]) }
        async fn list_generations(&self, _key_id: Option<&Uuid>, _page: u32, _per_page: u32) -> Result<Vec<crate::types::Generation>, sqlx::Error> { Ok(vec![]) }
        async fn count_generations(&self, _key_id: Option<&Uuid>) -> Result<i64, sqlx::Error> { Ok(0) }
        async fn cancel_generation(&self, _id: &str) -> Result<Option<crate::types::Generation>, sqlx::Error> { Ok(None) }

        async fn log_request(&self, _id: &str, _model: &str, _provider: &str, _status: &str,
            _media_type: &str, _cost_usd: f64, _latency_ms: i64,
            _error: Option<&str>, _metadata: Option<&serde_json::Value>) -> Result<(), sqlx::Error> { Ok(()) }

        async fn get_request_logs(&self, _page: u32, _per_page: u32)
            -> Result<(Vec<RequestLog>, u64), sqlx::Error> { Ok((vec![], 0)) }

        async fn get_request_logs_filtered(&self, _filters: &crate::types::LogFilters, _page: u32, _per_page: u32)
            -> Result<(Vec<RequestLog>, u64), sqlx::Error> { Ok((vec![], 0)) }

        async fn create_api_key(&self, name: &str, key_hash: &str, key_prefix: &str,
            _token_quota: Option<f64>, _rpm_limit: Option<u32>, _scopes: &str, _webhook_url: Option<&str>,
        ) -> Result<ApiKey, sqlx::Error> {
            Ok(ApiKey {
                id: Uuid::new_v4(), name: name.to_string(), key_hash: key_hash.to_string(),
                key_prefix: key_prefix.to_string(), created_at: chrono::Utc::now(),
                expires_at: None, is_active: true, token_quota: None, tokens_used: 0.0,
                rpm_limit: None, scopes: "generate,read".to_string(), webhook_url: None,
                owner_user_id: None,
            })
        }

        async fn get_api_key(&self, _id: &Uuid) -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }
        async fn update_api_key(&self, _id: &Uuid, _req: &crate::types::UpdateApiKeyRequest) -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }

        async fn lookup_api_key_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
            if key_hash == self.key_hash && self.is_active {
                Ok(Some(ApiKey {
                    id: Uuid::nil(),
                    name: "test".to_string(),
                    key_hash: key_hash.to_string(),
                    key_prefix: "lg-test".to_string(),
                    created_at: chrono::Utc::now(),
                    expires_at: self.expires_at,
                    is_active: self.is_active,
                    token_quota: self.token_quota,
                    tokens_used: self.tokens_used,
                    rpm_limit: self.rpm_limit,
                    scopes: self.scopes.clone(),
                    webhook_url: None,
                    owner_user_id: None,
                }))
            } else {
                Ok(None)
            }
        }

        async fn atomic_charge_tokens(&self, _id: &Uuid, _cost_usd: f64) -> Result<f64, sqlx::Error> { Ok(0.0) }
        async fn validate_api_key(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
            self.lookup_api_key_by_hash(key_hash).await
        }
        async fn list_api_keys(&self) -> Result<Vec<ApiKey>, sqlx::Error> { Ok(vec![]) }
        async fn revoke_api_key(&self, _id: &Uuid) -> Result<bool, sqlx::Error> { Ok(false) }
        async fn get_stats(&self) -> Result<ProxyStats, sqlx::Error> {
            Ok(ProxyStats {
                total_requests: 0, successful_requests: 0, failed_requests: 0,
                total_cost_usd: 0.0, avg_latency_ms: 0.0, requests_per_minute: 0.0,
                models_used: vec![], providers_used: vec![],
                latency_percentiles: LatencyPercentiles { p50_ms: 0.0, p95_ms: 0.0, p99_ms: 0.0, sample_count: 0, window_minutes: 60 },
            })
        }
    }

    /// Hash a plaintext key to SHA-256 hex.
    fn sha256_hex(s: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(s.as_bytes());
        hex::encode(h.finalize())
    }

    /// Build a state that enforces auth (master_key = Some("master")) but uses a custom DB.
    async fn build_state_with_db(db: impl DatabaseStore + 'static) -> Arc<AppState> {
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(crate::proxy::storage::LocalStore);
        let router = Arc::new(ProxyRouter::new(registry, cache, config, image_store));
        let materializer = Arc::new(Materializer::new(
            Arc::new(NoopStorage), reqwest::Client::new(),
        ));
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); p.push("models");
        let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("load shipped models"));
        Arc::new(AppState {
            router, db: Arc::new(db),
            master_key: Some("master".to_string()),
            registry: cap_registry, materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
        })
    }

    // ─── Test: DB key with generate,read scopes → 200 ────────────────────────

    #[tokio::test]
    async fn db_key_with_generate_scope_passes_auth() {
        let hash = sha256_hex("test-api-key");
        let db = ValidKeyDb::active(&hash, "generate,read");
        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET").uri("/test")
            .header("authorization", "Bearer test-api-key")
            .body(Body::empty()).unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Valid DB key should pass auth");
    }

    // ─── Test: expired key → 401 ────────────────────────────────────────────

    #[tokio::test]
    async fn expired_key_is_rejected_with_401() {
        let hash = sha256_hex("expiring-key");
        let mut db = ValidKeyDb::active(&hash, "generate,read");
        // Set expiry in the past
        db.expires_at = Some(chrono::Utc::now() - chrono::Duration::hours(1));

        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET").uri("/test")
            .header("authorization", "Bearer expiring-key")
            .body(Body::empty()).unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "Expired key should return 401");
    }

    // ─── Test: inactive key → 401 ────────────────────────────────────────────
    // When is_active = false, lookup_api_key_by_hash returns None → 401

    #[tokio::test]
    async fn inactive_key_is_rejected_with_401() {
        let hash = sha256_hex("inactive-key");
        let mut db = ValidKeyDb::active(&hash, "generate,read");
        db.is_active = false;

        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET").uri("/test")
            .header("authorization", "Bearer inactive-key")
            .body(Body::empty()).unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "Inactive key should return 401");
    }

    // ─── Test: quota exceeded → 402 ─────────────────────────────────────────

    #[tokio::test]
    async fn quota_exceeded_key_returns_402() {
        let hash = sha256_hex("quota-exceeded-key");
        let mut db = ValidKeyDb::active(&hash, "generate,read");
        db.token_quota = Some(1.0);
        db.tokens_used = 2.0; // already over quota

        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET").uri("/test")
            .header("authorization", "Bearer quota-exceeded-key")
            .body(Body::empty()).unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED, "Quota-exceeded key should return 402");
    }

    // ─── Test: DB key with rpm_limit gets X-RateLimit-* headers ───────────────

    #[tokio::test]
    async fn rate_limited_key_gets_x_ratelimit_headers() {
        let hash = sha256_hex("rpm-test-key");
        let mut db = ValidKeyDb::active(&hash, "generate,read");
        db.rpm_limit = Some(10);

        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET").uri("/test")
            .header("authorization", "Bearer rpm-test-key")
            .body(Body::empty()).unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers().contains_key("x-ratelimit-limit"),
            "x-ratelimit-limit header should be present for rate-limited key"
        );
        assert!(
            resp.headers().contains_key("x-ratelimit-remaining"),
            "x-ratelimit-remaining header should be present"
        );
        assert!(
            resp.headers().contains_key("x-ratelimit-reset"),
            "x-ratelimit-reset header should be present"
        );
        let limit = resp.headers().get("x-ratelimit-limit").unwrap().to_str().unwrap();
        assert_eq!(limit, "10", "x-ratelimit-limit should match rpm_limit");
    }

    // ─── Test: master key does NOT get X-RateLimit-* headers ──────────────────

    #[tokio::test]
    async fn master_key_has_no_ratelimit_headers() {
        let db = NoopDb;
        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);

        let req = Request::builder()
            .method("GET").uri("/test")
            .header("authorization", "Bearer master") // the master key set in build_state_with_db
            .body(Body::empty()).unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            !resp.headers().contains_key("x-ratelimit-limit"),
            "master key should NOT have x-ratelimit-limit header"
        );
        assert!(
            !resp.headers().contains_key("x-ratelimit-remaining"),
            "master key should NOT have x-ratelimit-remaining header"
        );
    }

    // ─── Test: exhausted bucket returns 429 with X-RateLimit headers ──────────

    #[tokio::test]
    async fn exhausted_rpm_returns_429_with_ratelimit_headers() {
        let hash = sha256_hex("exhausted-key");
        let mut db = ValidKeyDb::active(&hash, "generate,read");
        db.rpm_limit = Some(1); // only 1 request/minute

        let state = build_state_with_db(db).await;
        let app = build_auth_router(state);

        // First request — should succeed (bucket has 1 token)
        let req = Request::builder()
            .method("GET").uri("/test")
            .header("authorization", "Bearer exhausted-key")
            .body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "First request should succeed");

        // Second request — should be rate-limited
        let req = Request::builder()
            .method("GET").uri("/test")
            .header("authorization", "Bearer exhausted-key")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS, "Second request should be 429");
        assert!(
            resp.headers().contains_key("x-ratelimit-remaining"),
            "429 should have x-ratelimit-remaining"
        );
        let remaining = resp.headers().get("x-ratelimit-remaining").unwrap().to_str().unwrap();
        assert_eq!(remaining, "0", "remaining should be 0 on exhausted bucket");
        assert!(
            resp.headers().contains_key("retry-after"),
            "429 should have Retry-After header"
        );
    }
}
