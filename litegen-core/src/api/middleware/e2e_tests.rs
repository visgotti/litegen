/// End-to-end HTTP handler tests using `tower::ServiceExt::oneshot`.
/// These tests wire up the full axum Router with mock providers and an
/// InMemoryStorage backend, exercising the validator + materializer stack
/// without spinning up a real HTTP server.
#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use bytes::Bytes;
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::api::handlers::{estimate_image_cost, generate_image, generate_video};
    use crate::api::middleware::AppState;
    use crate::capabilities::CapabilityRegistry;
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::db::DatabaseStore;
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::materializer::{Materializer, MaterializeError, TempStorage};
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::{ImageStore, LocalStore};
    use crate::providers::{ImageProvider, ProviderInstanceConfig, VideoProvider};
    use crate::providers::image::mock::MockProvider;
    use crate::providers::video::mock::MockVideoProvider;
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
            _id: &str,
            _model: &str,
            _provider: &str,
            _status: &str,
            _media_type: &str,
            _cost_usd: f64,
            _latency_ms: i64,
            _error: Option<&str>,
            _metadata: Option<&serde_json::Value>,
        ) -> Result<(), sqlx::Error> {
            Ok(())
        }

        async fn get_request_logs(
            &self,
            _page: u32,
            _per_page: u32,
        ) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
            Ok((vec![], 0))
        }

        async fn get_request_logs_filtered(&self, _filters: &crate::types::LogFilters, _page: u32, _per_page: u32)
            -> Result<(Vec<RequestLog>, u64), sqlx::Error> { Ok((vec![], 0)) }

        async fn create_api_key(
            &self,
            name: &str,
            key_hash: &str,
            key_prefix: &str,
            _token_quota: Option<f64>,
            _rpm_limit: Option<u32>,
            _scopes: &str,
            _webhook_url: Option<&str>,
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
                org_id: None,
                app_id: None,
                public_id: None,
            })
        }

        async fn get_api_key(&self, _id: &Uuid) -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }

        async fn update_api_key(&self, _id: &Uuid, _req: &crate::types::UpdateApiKeyRequest) -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }

        async fn lookup_api_key_by_hash(&self, _key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> { Ok(None) }

        async fn atomic_charge_tokens(&self, _id: &Uuid, _cost_usd: f64) -> Result<f64, sqlx::Error> { Ok(0.0) }

        async fn validate_api_key(
            &self,
            _key_hash: &str,
        ) -> Result<Option<ApiKey>, sqlx::Error> {
            Ok(None)
        }

        async fn list_api_keys(&self) -> Result<Vec<ApiKey>, sqlx::Error> {
            Ok(vec![])
        }

        async fn revoke_api_key(&self, _id: &Uuid) -> Result<bool, sqlx::Error> {
            Ok(false)
        }

        async fn get_stats(&self) -> Result<ProxyStats, sqlx::Error> {
            Ok(ProxyStats {
                total_requests: 0,
                successful_requests: 0,
                failed_requests: 0,
                total_cost_usd: 0.0,
                avg_latency_ms: 0.0,
                requests_per_minute: 0.0,
                models_used: vec![],
                providers_used: vec![],
                latency_percentiles: LatencyPercentiles { p50_ms: 0.0, p95_ms: 0.0, p99_ms: 0.0, sample_count: 0, window_minutes: 60 },
            })
        }
    }

    // ─── InMemoryStorage (same pattern as materializer_tests) ─────────────────

    #[derive(Default)]
    struct InMemoryStorage {
        uploaded: std::sync::Mutex<Vec<String>>,
        deleted: std::sync::Mutex<Vec<String>>,
    }

    impl InMemoryStorage {
        fn uploaded_count(&self) -> usize {
            self.uploaded.lock().unwrap().len()
        }
        fn deleted_count(&self) -> usize {
            self.deleted.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl TempStorage for InMemoryStorage {
        async fn put(
            &self,
            key: &str,
            _bytes: Bytes,
            _ct: &str,
        ) -> Result<String, MaterializeError> {
            self.uploaded.lock().unwrap().push(key.to_string());
            Ok(format!("https://fake/{}", key))
        }

        async fn delete(&self, key: &str) -> Result<(), MaterializeError> {
            self.deleted.lock().unwrap().push(key.to_string());
            Ok(())
        }
    }

    // ─── Helper: build a CapabilityRegistry from the shipped models/ dir ──────

    fn shipped_registry() -> CapabilityRegistry {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        CapabilityRegistry::from_dir(&p).expect("shipped models must load")
    }

    /// Build a CapabilityRegistry with just a single model defined inline.
    /// Used for the multipart test where we need `provider_format: url`.
    fn registry_with_url_format() -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("e2e.yaml", r#"
models:
  - id: mock/image-gen-url
    provider: mock
    media_type: image
    display_name: Mock Image URL
    description: Mock image provider with URL ref format for e2e tests.
    pricing: { base_cost_usd: 0.0 }
    capabilities: { text_to_image: true, image_to_image: true }
    prompt: { required: true, max_length: 1000 }
    params:
      seed: { kind: seed, min: 0, max: 4294967294 }
      size:
        kind: size
        mode: enum
        values: [[512,512],[1024,1024]]
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [mock, test]
"#)]).expect("inline yaml must parse")
    }

    // ─── Helper: build AppState with a mock ProviderRegistry ─────────────────

    async fn build_state_with_registry(
        registry: CapabilityRegistry,
        storage: Arc<InMemoryStorage>,
    ) -> Arc<AppState> {
        let provider_registry = Arc::new(ProviderRegistry::new());

        // Register the mock image provider
        {
            let mut ip = MockProvider::new();
            ip.configure(ProviderInstanceConfig {
            credentials: Default::default(),
                api_key: String::new(),
                api_keys: vec![],
                api_base: None,
                model_mapping: HashMap::new(),
                extra_headers: HashMap::new(),
                options: None,
            });
            provider_registry.register_mock_image(Arc::new(ip)).await;
        }

        // Register the mock video provider
        {
            let mut vp = MockVideoProvider::new();
            vp.configure(ProviderInstanceConfig {
            credentials: Default::default(),
                api_key: String::new(),
                api_keys: vec![],
                api_base: None,
                model_mapping: HashMap::new(),
                extra_headers: HashMap::new(),
                options: None,
            });
            provider_registry.register_mock_video(Arc::new(vp)).await;
        }

        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store: Arc<dyn ImageStore> = Arc::new(LocalStore);
        let proxy_router = Arc::new(ProxyRouter::new(provider_registry, cache, config, image_store));

        let materializer = Arc::new(Materializer::new(
            storage,
            reqwest::Client::new(),
        ));

        Arc::new(AppState {
            router: proxy_router,
            db: Arc::new(NoopDb),
            master_key: None,
            registry: Arc::new(registry),
            materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
        })
    }

    // ─── Minimal test router (avoids the `:id` syntax panic in create_router) ───

    fn build_test_router(state: Arc<AppState>) -> axum::Router {
        use axum::routing::post;
        axum::Router::new()
            .route("/v1/images/generations", post(generate_image))
            .route("/v1/images/cost", post(estimate_image_cost))
            .route("/v1/videos/generations", post(generate_video))
            .with_state(state)
    }

    // ─── Test 1: Strict rejection — guidance_scale unsupported on mock/image-gen

    #[tokio::test]
    async fn test_strict_rejects_unsupported_param() {
        let registry = shipped_registry();
        let storage = Arc::new(InMemoryStorage::default());
        let state = build_state_with_registry(registry, storage).await;

        let app = build_test_router(state);

        let body = serde_json::to_vec(&serde_json::json!({
            "model": "mock/image-gen",
            "prompt": "a red apple",
            "strict": true,
            "guidance_scale": 7.5
        }))
        .unwrap();

        let req = Request::builder()
            .method("POST")
            .uri("/v1/images/generations")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(
            json["error"]["code"],
            "param_unsupported",
            "unexpected error body: {}",
            json
        );
        assert_eq!(
            json["error"]["param"],
            "guidance_scale",
            "unexpected error body: {}",
            json
        );
    }

    // ─── Test 2: Lax drop — guidance_scale dropped, X-Litegen-Dropped-Params set

    #[tokio::test]
    async fn test_lax_drops_unsupported_param_and_sets_header() {
        let registry = shipped_registry();
        let storage = Arc::new(InMemoryStorage::default());
        let state = build_state_with_registry(registry, storage).await;

        let app = build_test_router(state);

        let body = serde_json::to_vec(&serde_json::json!({
            "model": "mock/image-gen",
            "prompt": "a blue sky",
            "strict": false,
            "guidance_scale": 7.5
        }))
        .unwrap();

        let req = Request::builder()
            .method("POST")
            .uri("/v1/images/generations")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "expected 200, got {}",
            resp.status()
        );

        let dropped_header = resp.headers().get("x-litegen-dropped-params");
        assert!(
            dropped_header.is_some(),
            "X-Litegen-Dropped-Params header missing"
        );
        let header_val = dropped_header.unwrap().to_str().unwrap();
        assert!(
            header_val.contains("guidance_scale"),
            "expected 'guidance_scale' in dropped header, got '{}'",
            header_val
        );
    }

    // ─── Test 3: Multipart blob → URL materializer round-trip ─────────────────
    //
    // The model `mock/image-gen-url` uses provider_format=url.
    // The client sends a blob ref `{type: "blob", value: "img"}` + an `img` file part.
    // The materializer should (a) upload the bytes to InMemoryStorage, (b) the provider
    // gets a URL form, and (c) cleanup deletes the key after the response.

    #[tokio::test]
    async fn test_multipart_blob_to_url_uploads_and_cleans_up() {
        let registry = registry_with_url_format();
        let storage = Arc::new(InMemoryStorage::default());
        let storage_clone = storage.clone();

        let state = build_state_with_registry(registry, storage_clone).await;
        let app = build_test_router(state);

        // Build multipart body manually
        let boundary = "testboundary123";
        let request_json = serde_json::to_string(&serde_json::json!({
            "model": "mock/image-gen-url",
            "prompt": "a reference image test",
            "strict": true,
            "reference_images": [
                { "type": "blob", "value": "img", "role": "init" }
            ]
        }))
        .unwrap();

        // Fake image bytes (tiny PNG header)
        let image_bytes: &[u8] = &[0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

        let mut body_bytes: Vec<u8> = Vec::new();

        // Part 1: request JSON
        body_bytes.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"request\"\r\nContent-Type: application/json\r\n\r\n{request_json}\r\n",
            )
            .as_bytes(),
        );

        // Part 2: image blob
        body_bytes.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"img\"; filename=\"img.png\"\r\nContent-Type: image/png\r\n\r\n",
            )
            .as_bytes(),
        );
        body_bytes.extend_from_slice(image_bytes);
        body_bytes.extend_from_slice(b"\r\n");

        // End boundary
        body_bytes.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let req = Request::builder()
            .method("POST")
            .uri("/v1/images/generations")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body_bytes))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "expected 200, got {}",
            resp.status()
        );

        // Verify the blob was uploaded to temp storage
        assert_eq!(
            storage.uploaded_count(),
            1,
            "expected 1 upload to temp storage"
        );

        // Drop response (triggers Cleanup via tokio::spawn)
        drop(resp);

        // Give the spawned cleanup task a moment to run
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(
            storage.deleted_count(),
            1,
            "expected 1 delete from temp storage (cleanup)"
        );
    }

    // ─── Test 4: POST /v1/images/cost returns total_cost_usd: 0.0 for mock/image-gen

    #[tokio::test]
    async fn test_image_cost_endpoint_returns_zero_for_mock() {
        let registry = shipped_registry();
        let storage = Arc::new(InMemoryStorage::default());
        let state = build_state_with_registry(registry, storage).await;
        let app = build_test_router(state);

        let body = serde_json::to_vec(&serde_json::json!({
            "model": "mock/image-gen",
            "prompt": "a beautiful sunset",
            "strict": false
        }))
        .unwrap();

        let req = Request::builder()
            .method("POST")
            .uri("/v1/images/cost")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "expected 200 from /v1/images/cost"
        );

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(
            json["total_cost_usd"],
            serde_json::Value::Number(serde_json::Number::from_f64(0.0).unwrap()),
            "expected total_cost_usd: 0.0 for mock provider, got: {}",
            json
        );
    }

    // ─── Test 5: POST /v1/images/cost with strict:true + unsupported param → 400

    #[tokio::test]
    async fn test_image_cost_endpoint_strict_rejects_unsupported_param() {
        let registry = shipped_registry();
        let storage = Arc::new(InMemoryStorage::default());
        let state = build_state_with_registry(registry, storage).await;
        let app = build_test_router(state);

        let body = serde_json::to_vec(&serde_json::json!({
            "model": "mock/image-gen",
            "prompt": "a beautiful sunset",
            "strict": true,
            "guidance_scale": 7.5
        }))
        .unwrap();

        let req = Request::builder()
            .method("POST")
            .uri("/v1/images/cost")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "expected 400 when strict=true and guidance_scale unsupported, got {}",
            resp.status()
        );

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(
            json["error"]["code"],
            "param_unsupported",
            "unexpected error body: {}",
            json
        );
        assert_eq!(
            json["error"]["param"],
            "guidance_scale",
            "unexpected error body: {}",
            json
        );
    }

    // ─── SpyDb: records log_request calls ─────────────────────────────────────

    struct SpyDb {
        log_count: Arc<AtomicUsize>,
    }

    impl SpyDb {
        fn new() -> (Self, Arc<AtomicUsize>) {
            let counter = Arc::new(AtomicUsize::new(0));
            (Self { log_count: counter.clone() }, counter)
        }
    }

    #[async_trait::async_trait]
    impl DatabaseStore for SpyDb {
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
        ) -> Result<(), sqlx::Error> {
            self.log_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

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
                org_id: None,
                app_id: None,
                public_id: None,
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

    // ─── G2.6: Async log persistence ──────────────────────────────────────────
    //
    // After a successful POST /v1/images/generations, the handler spawns a
    // tokio task that calls db.log_request. We verify it is called exactly once.

    async fn build_state_with_spy_db(
        registry: CapabilityRegistry,
        storage: Arc<InMemoryStorage>,
        db: Arc<dyn DatabaseStore>,
    ) -> Arc<AppState> {
        let provider_registry = Arc::new(ProviderRegistry::new());

        {
            let mut ip = MockProvider::new();
            ip.configure(ProviderInstanceConfig {
            credentials: Default::default(),
                api_key: String::new(),
                api_keys: vec![],
                api_base: None,
                model_mapping: HashMap::new(),
                extra_headers: HashMap::new(),
                options: None,
            });
            provider_registry.register_mock_image(Arc::new(ip)).await;
        }

        {
            let mut vp = MockVideoProvider::new();
            vp.configure(ProviderInstanceConfig {
            credentials: Default::default(),
                api_key: String::new(),
                api_keys: vec![],
                api_base: None,
                model_mapping: HashMap::new(),
                extra_headers: HashMap::new(),
                options: None,
            });
            provider_registry.register_mock_video(Arc::new(vp)).await;
        }

        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store: Arc<dyn ImageStore> = Arc::new(LocalStore);
        let proxy_router = Arc::new(ProxyRouter::new(provider_registry, cache, config, image_store));

        let materializer = Arc::new(Materializer::new(
            storage,
            reqwest::Client::new(),
        ));

        Arc::new(AppState {
            router: proxy_router,
            db,
            master_key: None,
            registry: Arc::new(registry),
            materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
        })
    }

    #[tokio::test]
    async fn test_async_log_persistence_after_generate_image() {
        let registry = shipped_registry();
        let storage = Arc::new(InMemoryStorage::default());

        let (spy_db, log_counter) = SpyDb::new();
        let state = build_state_with_spy_db(registry, storage, Arc::new(spy_db)).await;
        let app = build_test_router(state);

        let body = serde_json::to_vec(&serde_json::json!({
            "model": "mock/image-gen",
            "prompt": "async log test",
            "strict": false,
        }))
        .unwrap();

        let req = Request::builder()
            .method("POST")
            .uri("/v1/images/generations")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "expected 200 for generate_image, got {}",
            resp.status()
        );

        // Give the spawned tokio task a moment to settle
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let count = log_counter.load(Ordering::SeqCst);
        assert_eq!(count, 1, "Expected db.log_request to be called exactly once, got {}", count);
    }

    // ─── G2.7: Prometheus /metrics endpoint smoke ─────────────────────────────
    //
    // The /metrics route is now wired into the lib router via `create_router`.
    // This test verifies it returns 200 and a Prometheus text body.

    #[tokio::test]
    async fn test_metrics_endpoint_returns_prometheus_body() {
        // Ensure the recorder is installed before the router is built
        crate::api::metrics::init_prometheus();

        // Record a marker counter so we have something to assert on
        metrics::counter!("litegen_e2e_test_total").increment(1);

        let registry = shipped_registry();
        let storage = Arc::new(InMemoryStorage::default());
        let state = build_state_with_registry(registry, storage).await;

        // Use the full create_router so /metrics is included
        let app = crate::api::handlers::create_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "expected 200 from GET /metrics"
        );

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();

        assert!(
            body.contains("# TYPE") || body.contains("# HELP") || body.contains("litegen_e2e_test_total"),
            "expected Prometheus format text in /metrics body, got: {:?}",
            &body[..body.len().min(400)]
        );
    }

    // ─── Helpers for generation + webhook e2e tests ───────────────────────────

    async fn build_state_with_sqlite(registry: CapabilityRegistry, storage: Arc<InMemoryStorage>) -> Arc<AppState> {
        use crate::db::sqlite::SqliteDatabase;
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory sqlite"));
        let provider_registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());

        {
            let mut ip = MockProvider::new();
            ip.configure(ProviderInstanceConfig {
            credentials: Default::default(),
                api_key: String::new(), api_keys: vec![], api_base: None,
                model_mapping: HashMap::new(), extra_headers: HashMap::new(), options: None,
            });
            provider_registry.register_mock_image(Arc::new(ip)).await;
        }
        {
            let mut vp = MockVideoProvider::new();
            vp.configure(ProviderInstanceConfig {
            credentials: Default::default(),
                api_key: String::new(), api_keys: vec![], api_base: None,
                model_mapping: HashMap::new(), extra_headers: HashMap::new(), options: None,
            });
            provider_registry.register_mock_video(Arc::new(vp)).await;
        }

        let config = Arc::new(crate::config::AppConfig::default());
        let cache = Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default()));
        let image_store: Arc<dyn ImageStore> = Arc::new(LocalStore);
        let proxy_router = Arc::new(ProxyRouter::new(provider_registry, cache, config, image_store));

        let materializer = Arc::new(Materializer::new(storage, reqwest::Client::new()));

        Arc::new(AppState {
            router: proxy_router,
            db,
            master_key: None,
            registry: Arc::new(registry),
            materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
        })
    }

    fn build_generation_router(state: Arc<AppState>) -> axum::Router {
        use axum::routing::{get, post};
        use crate::api::handlers::{generate_video, get_generation};
        axum::Router::new()
            .route("/v1/videos/generations", post(generate_video))
            .route("/v1/generations/{id}", get(get_generation))
            .with_state(state)
    }

    // ─── G3.1: POST video + GET /v1/generations/{id} polling flow ────────────

    #[tokio::test]
    async fn test_video_generation_db_polling_flow() {
        let registry = shipped_registry();
        let storage = Arc::new(InMemoryStorage::default());
        let state = build_state_with_sqlite(registry, storage).await;

        let app = build_generation_router(state.clone());

        // POST /v1/videos/generations
        let body = serde_json::to_vec(&serde_json::json!({
            "model": "mock/video-gen",
            "prompt": "a cinematic sunrise",
            "strict": false,
        })).unwrap();

        let req = Request::builder()
            .method("POST")
            .uri("/v1/videos/generations")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "POST /v1/videos/generations should be 200");

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let vid_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let gen_id = vid_resp["id"].as_str().unwrap().to_string();
        assert!(gen_id.starts_with("litegen-vid-"), "id should start with litegen-vid-");

        // Give the spawn task time to insert the row
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // GET /v1/generations/{id} — should be pending or processing
        let req = Request::builder()
            .method("GET")
            .uri(format!("/v1/generations/{}", gen_id))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "GET /v1/generations/{{id}} should be 200");

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let gen: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let initial_status = gen["status"].as_str().unwrap();
        assert!(
            initial_status == "pending" || initial_status == "processing",
            "initial status should be pending or processing, got {initial_status}"
        );

        // Run one poller iteration
        let db_arc = state.db.clone();
        let reg_arc = state.router.registry.clone();
        crate::proxy::poller::poll_once(&db_arc, &reg_arc, &reqwest::Client::new()).await;

        // GET again — should be completed
        let req = Request::builder()
            .method("GET")
            .uri(format!("/v1/generations/{}", gen_id))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "GET after poll should be 200");

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let gen: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(gen["status"].as_str().unwrap(), "completed", "status should be completed after poll");
        assert!(gen["result_url"].as_str().is_some(), "result_url should be present");
    }

    // ─── G3.2: GET /v1/generations/{id} for unknown id → 404 ────────────────

    #[tokio::test]
    async fn test_get_generation_unknown_id_returns_404() {
        let registry = shipped_registry();
        let storage = Arc::new(InMemoryStorage::default());
        let state = build_state_with_sqlite(registry, storage).await;
        let app = build_generation_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/generations/litegen-vid-does-not-exist")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "unknown generation should return 404");
    }

    // ─── G3.3: Webhook delivery on terminal video status ─────────────────────

    #[tokio::test]
    async fn test_webhook_delivered_on_video_completion() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/gen-webhook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let registry = shipped_registry();
        let storage = Arc::new(InMemoryStorage::default());
        let state = build_state_with_sqlite(registry, storage).await;

        // Create a key with webhook_url
        let key = state.db.create_api_key(
            "wh-e2e-key",
            "wh-e2e-hash-unique",
            "lg-we",
            None, None,
            "generate,read",
            Some(&format!("{}/gen-webhook", server.uri())),
        ).await.unwrap();

        let _app = build_generation_router(state.clone());

        // Insert a generation tied to this key
        state.db.insert_generation(
            "litegen-vid-wh-e2e-1",
            Some(&key.id),
            "mock/video-gen",
            "mock",
            "video",
            Some("mock-video-job-1"),
            0.0,
        ).await.unwrap();

        // Run one poll iteration
        let db_arc = state.db.clone();
        let reg_arc = state.router.registry.clone();
        crate::proxy::poller::poll_once(&db_arc, &reg_arc, &reqwest::Client::new()).await;

        // Give webhook dispatch task time to fire
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        server.verify().await;
    }

    // ─── G4.1: mock/all-params-image — all params pass, real PNG > 1000 bytes ─

    #[tokio::test]
    async fn test_all_params_image_generates_real_png() {
        let registry = shipped_registry();
        let storage = Arc::new(InMemoryStorage::default());
        let state = build_state_with_registry(registry, storage).await;
        let app = build_test_router(state);

        let body = serde_json::to_vec(&serde_json::json!({
            "model": "mock/all-params-image",
            "prompt": "a vibrant mountain landscape",
            "strict": true,
            "seed": 42,
            "size": "1024x1024",
            "steps": 30,
            "guidance_scale": 7.5
        }))
        .unwrap();

        let req = Request::builder()
            .method("POST")
            .uri("/v1/images/generations")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "mock/all-params-image should return 200 with valid params"
        );

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        // Verify b64_json is present and long enough to be a real image
        let b64 = json["data"][0]["b64_json"]
            .as_str()
            .expect("expected b64_json in response");
        // A real PNG encoded in base64 is considerably > 1000 chars
        assert!(
            b64.len() > 1000,
            "expected real PNG b64 > 1000 chars, got {}",
            b64.len()
        );
    }

    // ─── G4.2: mock/expensive-image — quota deducted and 402 on exhaustion ────

    #[tokio::test]
    async fn test_expensive_image_quota_enforcement() {
        use crate::db::sqlite::SqliteDatabase;
        use sha2::{Digest, Sha256};

        let db = Arc::new(
            SqliteDatabase::connect("sqlite::memory:")
                .await
                .expect("in-memory sqlite"),
        );

        // Create a key with quota = 10.0 (enough for 2 × $5.00 = $10.00)
        let raw_key = "test-expensive-key-quota";
        let hash = {
            let mut h = Sha256::new();
            h.update(raw_key.as_bytes());
            hex::encode(h.finalize())
        };
        let _key = db
            .create_api_key("expensive-test", &hash, "lg-exp", Some(10.0), None, "generate,read", None)
            .await
            .unwrap();

        // Build state with this sqlite DB and a master_key so auth is enforced
        let provider_registry = Arc::new(ProviderRegistry::new());
        {
            let mut ip = MockProvider::new();
            ip.configure(ProviderInstanceConfig {
            credentials: Default::default(),
                api_key: String::new(),
                api_keys: vec![],
                api_base: None,
                model_mapping: HashMap::new(),
                extra_headers: HashMap::new(),
                options: None,
            });
            provider_registry.register_mock_image(Arc::new(ip)).await;
        }

        let config = Arc::new(crate::config::AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store: Arc<dyn ImageStore> = Arc::new(LocalStore);
        let proxy_router = Arc::new(ProxyRouter::new(provider_registry, cache, config, image_store));
        let storage = Arc::new(InMemoryStorage::default());
        let materializer = Arc::new(Materializer::new(
            storage,
            reqwest::Client::new(),
        ));
        let registry = shipped_registry();
        let state = Arc::new(AppState {
            router: proxy_router,
            db: db.clone(),
            master_key: Some("master".to_string()),
            registry: Arc::new(registry),
            materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
        });

        // Build a router with auth middleware + the generate handler
        use axum::middleware;
        use axum::routing::post;
        use crate::api::middleware::auth_middleware;
        use crate::api::handlers::generate_image;

        let state_for_mw = state.clone();
        let app = axum::Router::new()
            .route("/v1/images/generations", post(generate_image))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: axum::middleware::Next| {
                let s = state_for_mw.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state.clone());

        let make_req = || {
            let body = serde_json::to_vec(&serde_json::json!({
                "model": "mock/expensive-image",
                "prompt": "short prompt",
                "strict": false,
            }))
            .unwrap();
            Request::builder()
                .method("POST")
                .uri("/v1/images/generations")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", raw_key))
                .body(Body::from(body))
                .unwrap()
        };

        // Request 1: should succeed (tokens_used goes to 5.0)
        let resp1 = app.clone().oneshot(make_req()).await.unwrap();
        assert_eq!(
            resp1.status(),
            StatusCode::OK,
            "first expensive request should succeed"
        );
        drop(resp1);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Request 2: should succeed (tokens_used goes to 10.0)
        let resp2 = app.clone().oneshot(make_req()).await.unwrap();
        assert_eq!(
            resp2.status(),
            StatusCode::OK,
            "second expensive request should succeed"
        );
        drop(resp2);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Verify tokens_used == 10.0 in DB
        let fetched = db.lookup_api_key_by_hash(&hash).await.unwrap().expect("key must exist");
        assert!(
            (fetched.tokens_used - 10.0).abs() < 0.01,
            "expected tokens_used == 10.0, got {}",
            fetched.tokens_used
        );

        // Request 3: pre-flight quota check should reject with 402
        let resp3 = app.clone().oneshot(make_req()).await.unwrap();
        assert_eq!(
            resp3.status(),
            StatusCode::PAYMENT_REQUIRED,
            "third expensive request should return 402 quota_exceeded"
        );
        let bytes = axum::body::to_bytes(resp3.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            json["error"]["type"],
            "quota_exceeded",
            "error type should be quota_exceeded, got: {}",
            json
        );
    }

    // ─── G4.3: mock/strict-duration-video — duration=3.0 rejected (min==max==5) ─

    #[tokio::test]
    async fn test_strict_duration_video_rejects_out_of_range() {
        let registry = shipped_registry();
        let storage = Arc::new(InMemoryStorage::default());
        let state = build_state_with_registry(registry, storage).await;
        let app = build_test_router(state);

        let body = serde_json::to_vec(&serde_json::json!({
            "model": "mock/strict-duration-video",
            "prompt": "a slow pan across a canyon",
            "strict": true,
            "duration_seconds": 3.0,
        }))
        .unwrap();

        let req = Request::builder()
            .method("POST")
            .uri("/v1/videos/generations")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "duration_seconds=3.0 should be rejected when min==max==5.0, got {}",
            resp.status()
        );

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            json["error"]["code"].as_str().is_some() || json["error"].as_object().is_some(),
            "expected error body, got: {}",
            json
        );
    }
}
