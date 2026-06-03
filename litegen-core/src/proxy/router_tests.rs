/// Unit tests for ProxyRouter routing strategies.
#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;

    use crate::capabilities::{CapabilityRegistry, ModelSchema};
    use crate::config::{AppConfig, CacheGlobalConfig, DeploymentConfig, ModelRouteConfig};
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::materializer::MaterializedRequest;
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::{ImageStore, ImageStoreError, LocalStore};
    use crate::providers::{
        GenerationOutput, HealthCheckResult, ImageExtras, ImageProvider,
        ProviderError, ProviderInstanceConfig, build_cost_estimate,
    };
    use crate::types::*;

    // ─── Helpers ──────────────────────────────────────────────────────────────

    fn shipped_schema(id: &str) -> ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = CapabilityRegistry::from_dir(&p).expect("load shipped models");
        r.get(id).expect("model not found in shipped models").clone()
    }

    fn empty_materialized() -> MaterializedRequest {
        MaterializedRequest {
            refs: vec![],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(),
            model: model.to_string(),
            n: 1,
            negative_prompt: None,
            seed: None,
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn make_image_extras() -> ImageExtras {
        ImageExtras {
            size: None,
            aspect_ratio: None,
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            extra: None,
        }
    }

    fn default_instance_config() -> ProviderInstanceConfig {
        ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: String::new(),
            api_keys: vec![],
            api_base: None,
            model_mapping: HashMap::new(),
            extra_headers: HashMap::new(),
            options: None,
        }
    }

    // ─── A provider that always fails (for testing fallback) ──────────────────

    struct AlwaysFailingProvider;

    #[async_trait]
    impl ImageProvider for AlwaysFailingProvider {
        fn name(&self) -> &str {
            "always-failing"
        }

        fn configure(&mut self, _config: ProviderInstanceConfig) {}

        fn is_configured(&self) -> bool {
            true
        }

        async fn generate(
            &self,
            _model: &ModelSchema,
            _base: &BaseGenerationRequest,
            _extras: &ImageExtras,
            _materialized: &MaterializedRequest,
        ) -> Result<GenerationOutput, ProviderError> {
            Err(ProviderError::RequestFailed {
                message: "Simulated failure for routing test".to_string(),
                status_code: Some(500),
                provider_error: None,
                retryable: true, // retryable so fallback moves to next deployment
            })
        }

        async fn estimate_cost(
            &self,
            model: &ModelSchema,
            _request: &ImageGenerationRequest,
        ) -> Result<CostEstimate, ProviderError> {
            Ok(build_cost_estimate(
                model.pricing.base_cost_usd,
                0.0,
                CostSource::Estimated,
                None,
            ))
        }

        async fn health_check(&self) -> HealthCheckResult {
            HealthCheckResult {
                healthy: false,
                message: "Always failing provider".to_string(),
                latency_ms: None,
            }
        }
    }

    // ─── A provider that counts calls and can be named anything ──────────────

    struct CountingProvider {
        name: &'static str,
        counter: Arc<AtomicUsize>,
    }

    impl CountingProvider {
        fn new(name: &'static str) -> (Self, Arc<AtomicUsize>) {
            let counter = Arc::new(AtomicUsize::new(0));
            (Self { name, counter: counter.clone() }, counter)
        }
    }

    #[async_trait]
    impl ImageProvider for CountingProvider {
        fn name(&self) -> &str {
            self.name
        }
        fn configure(&mut self, _config: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }

        async fn generate(
            &self,
            _model: &ModelSchema,
            _base: &BaseGenerationRequest,
            _extras: &ImageExtras,
            _materialized: &MaterializedRequest,
        ) -> Result<GenerationOutput, ProviderError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(GenerationOutput {
                data: vec![0u8; 4],
                content_type: "image/png".to_string(),
                metadata: HashMap::new(),
            })
        }

        async fn estimate_cost(
            &self,
            model: &ModelSchema,
            _request: &ImageGenerationRequest,
        ) -> Result<CostEstimate, ProviderError> {
            Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
        }

        async fn health_check(&self) -> HealthCheckResult {
            HealthCheckResult { healthy: true, message: "ok".into(), latency_ms: Some(0) }
        }
    }

    // ─── A provider that fails N times then succeeds ──────────────────────────

    struct FlakyProvider {
        name: &'static str,
        fail_count: u32,
        counter: Arc<AtomicUsize>,
    }

    impl FlakyProvider {
        fn new(name: &'static str, fail_count: u32) -> (Self, Arc<AtomicUsize>) {
            let counter = Arc::new(AtomicUsize::new(0));
            (Self { name, fail_count, counter: counter.clone() }, counter)
        }
    }

    #[async_trait]
    impl ImageProvider for FlakyProvider {
        fn name(&self) -> &str { self.name }
        fn configure(&mut self, _config: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }

        async fn generate(
            &self,
            _model: &ModelSchema,
            _base: &BaseGenerationRequest,
            _extras: &ImageExtras,
            _materialized: &MaterializedRequest,
        ) -> Result<GenerationOutput, ProviderError> {
            let attempt = self.counter.fetch_add(1, Ordering::SeqCst);
            if attempt < self.fail_count as usize {
                Err(ProviderError::Timeout { timeout_ms: 100 })
            } else {
                Ok(GenerationOutput {
                    data: vec![0u8; 4],
                    content_type: "image/png".to_string(),
                    metadata: HashMap::new(),
                })
            }
        }

        async fn estimate_cost(
            &self,
            model: &ModelSchema,
            _request: &ImageGenerationRequest,
        ) -> Result<CostEstimate, ProviderError> {
            Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
        }

        async fn health_check(&self) -> HealthCheckResult {
            HealthCheckResult { healthy: true, message: "ok".into(), latency_ms: Some(0) }
        }
    }

    // ─── SpyStore: records writes ─────────────────────────────────────────────

    #[derive(Default)]
    struct SpyStore {
        write_count: Arc<AtomicUsize>,
    }

    impl SpyStore {
        fn new() -> (Self, Arc<AtomicUsize>) {
            let counter = Arc::new(AtomicUsize::new(0));
            (Self { write_count: counter.clone() }, counter)
        }
    }

    #[async_trait::async_trait]
    impl ImageStore for SpyStore {
        async fn store(
            &self,
            _data: &[u8],
            _content_type: &str,
            _generation_id: &str,
        ) -> Result<String, ImageStoreError> {
            self.write_count.fetch_add(1, Ordering::SeqCst);
            Ok("https://spy-store/fake-url".to_string())
        }
    }

    // ─── Test: fallback strategy skips failing provider and uses second deployment

    #[tokio::test]
    async fn fallback_strategy_skips_failing_provider_and_uses_second() {
        let config = Arc::new(AppConfig {
            model_routes: vec![ModelRouteConfig {
                model: "mock/image-gen".to_string(),
                deployments: vec![
                    DeploymentConfig {
                        provider: "always-failing".to_string(),
                        weight: 1,
                        max_retries: 0,
                        timeout_seconds: 30,
                        rpm_limit: 0,
                    },
                    DeploymentConfig {
                        provider: "mock".to_string(),
                        weight: 1,
                        max_retries: 2,
                        timeout_seconds: 30,
                        rpm_limit: 0,
                    },
                ],
                strategy: Some("fallback".to_string()),
                cache: None,
            }],
            ..AppConfig::default()
        });

        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_image_provider_named("always-failing", Arc::new(AlwaysFailingProvider))
            .await;

        {
            use crate::providers::image::mock::MockProvider;
            let mut mp = MockProvider::new();
            mp.configure(default_instance_config());
            registry.register_mock_image(Arc::new(mp)).await;
        }

        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = ProxyRouter::new(registry, cache, config, image_store);

        let schema = shipped_schema("mock/image-gen");
        let base = make_base("a beautiful landscape", "mock/image-gen");
        let extras = make_image_extras();
        let materialized = empty_materialized();

        let result = router
            .generate_image(&schema, &base, &extras, &materialized, None)
            .await;

        assert!(
            result.is_ok(),
            "Expected generate_image to succeed via fallback, but got: {:?}",
            result.err()
        );

        let response = result.unwrap();
        assert_eq!(response.provider, "mock");
        assert_eq!(response.model, "mock/image-gen");
        assert!(!response.data.is_empty(), "Expected non-empty image data");
    }

    // ─── G1: lowest_latency routes to provider with lower average latency ─────

    #[tokio::test]
    async fn lowest_latency_routes_to_lowest_avg_latency_provider() {
        let config = Arc::new(AppConfig {
            model_routes: vec![ModelRouteConfig {
                model: "mock/image-gen".to_string(),
                deployments: vec![
                    DeploymentConfig {
                        provider: "mock_a".to_string(),
                        weight: 1,
                        max_retries: 0,
                        timeout_seconds: 30,
                        rpm_limit: 0,
                    },
                    DeploymentConfig {
                        provider: "mock_b".to_string(),
                        weight: 1,
                        max_retries: 0,
                        timeout_seconds: 30,
                        rpm_limit: 0,
                    },
                ],
                strategy: Some("lowest_latency".to_string()),
                cache: None,
            }],
            ..AppConfig::default()
        });

        let registry = Arc::new(ProviderRegistry::new());

        let (provider_a, _counter_a) = CountingProvider::new("mock_a");
        registry.register_image_provider_named("mock_a", Arc::new(provider_a)).await;

        let (provider_b, _counter_b) = CountingProvider::new("mock_b");
        registry.register_image_provider_named("mock_b", Arc::new(provider_b)).await;

        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = ProxyRouter::new(registry, cache, config, image_store);

        // Seed latency history: mock_a is slow (avg 550ms), mock_b is fast (avg 150ms)
        router.set_latency_history("mock_a", vec![500, 600]).await;
        router.set_latency_history("mock_b", vec![100, 200]).await;

        let schema = shipped_schema("mock/image-gen");
        let base = make_base("latency test prompt", "mock/image-gen");
        let extras = make_image_extras();
        let materialized = empty_materialized();

        let result = router
            .generate_image(&schema, &base, &extras, &materialized, None)
            .await;

        assert!(result.is_ok(), "Expected success, got: {:?}", result.err());
        let response = result.unwrap();
        assert_eq!(
            response.provider, "mock_b",
            "Expected mock_b (lower avg latency 150ms) but got '{}'",
            response.provider
        );
    }

    // ─── G2.1: Retry on retryable provider error ─────────────────────────────

    #[tokio::test]
    async fn retry_on_retryable_error_succeeds_after_failures() {
        // FlakyProvider fails twice (attempts 0, 1) then succeeds on attempt 2
        let (flaky, counter) = FlakyProvider::new("flaky", 2);

        let config = Arc::new(AppConfig {
            model_routes: vec![ModelRouteConfig {
                model: "mock/image-gen".to_string(),
                deployments: vec![
                    DeploymentConfig {
                        provider: "flaky".to_string(),
                        weight: 1,
                        max_retries: 3,
                        timeout_seconds: 30,
                        rpm_limit: 0,
                    },
                ],
                strategy: Some("fallback".to_string()),
                cache: None,
            }],
            ..AppConfig::default()
        });

        let registry = Arc::new(ProviderRegistry::new());
        registry.register_image_provider_named("flaky", Arc::new(flaky)).await;

        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = ProxyRouter::new(registry, cache, config, image_store);

        let schema = shipped_schema("mock/image-gen");
        let base = make_base("retry test", "mock/image-gen");
        let extras = make_image_extras();
        let materialized = empty_materialized();

        let result = router.generate_image(&schema, &base, &extras, &materialized, None).await;
        assert!(result.is_ok(), "Expected success after retries, got: {:?}", result.err());

        let attempts = counter.load(Ordering::SeqCst);
        assert_eq!(attempts, 3, "Expected 3 total attempts (0, 1 fail; 2 success), got {}", attempts);
    }

    // ─── G2.2: Cache hit returns cached response without calling provider ─────

    #[tokio::test]
    async fn cache_hit_does_not_call_provider_second_time() {
        let (counting_provider, call_counter) = CountingProvider::new("mock");

        let config = Arc::new(AppConfig::default());

        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_image(Arc::new(counting_provider)).await;

        // Enable cache
        let cache_config = CacheGlobalConfig {
            enabled: true,
            default_ttl_seconds: 3600,
            max_items: 100,
        };
        let cache = Arc::new(GenerationCache::new(&cache_config));
        let image_store = Arc::new(LocalStore);

        // Build a schema that uses "mock" provider directly (no route)
        let schema = shipped_schema("mock/image-gen");
        let router = ProxyRouter::new(registry, cache, config, image_store);

        let base = make_base("cache test prompt", "mock/image-gen");
        let extras = make_image_extras();
        let materialized = empty_materialized();

        // First call — provider is invoked
        let r1 = router.generate_image(&schema, &base, &extras, &materialized, None).await;
        assert!(r1.is_ok(), "First call failed: {:?}", r1.err());

        // Second call with same prompt — should hit cache
        let r2 = router.generate_image(&schema, &base, &extras, &materialized, None).await;
        assert!(r2.is_ok(), "Second call failed: {:?}", r2.err());

        let count = call_counter.load(Ordering::SeqCst);
        assert_eq!(count, 1, "Provider should have been called once (cache hit on 2nd call), got {}", count);
    }

    // ─── G2.3: Storage write of generated output bytes ────────────────────────

    #[tokio::test]
    async fn storage_receives_write_on_successful_generation() {
        let (spy_store, write_counter) = SpyStore::new();

        let config = Arc::new(AppConfig::default());
        let registry = Arc::new(ProviderRegistry::new());

        {
            use crate::providers::image::mock::MockProvider;
            let mut mp = MockProvider::new();
            mp.configure(default_instance_config());
            registry.register_mock_image(Arc::new(mp)).await;
        }

        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let router = ProxyRouter::new(registry, cache, config, Arc::new(spy_store));

        let schema = shipped_schema("mock/image-gen");
        let base = make_base("storage write test", "mock/image-gen");
        let extras = make_image_extras();
        let materialized = empty_materialized();

        let result = router.generate_image(&schema, &base, &extras, &materialized, None).await;
        assert!(result.is_ok(), "Expected success, got: {:?}", result.err());

        let writes = write_counter.load(Ordering::SeqCst);
        assert_eq!(writes, 1, "Expected 1 storage write, got {}", writes);
    }

    /// Regression test: two requests with same prompt but different size MUST
    /// produce different cached responses. The original cache_key was keyed only
    /// on (model, prompt) and collided across all other params.
    #[tokio::test]
    async fn cache_does_not_collide_when_only_size_differs() {
        let (counting_provider, call_counter) = CountingProvider::new("mock");
        let config = Arc::new(AppConfig::default());
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_image(Arc::new(counting_provider)).await;
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig {
            enabled: true, default_ttl_seconds: 3600, max_items: 100,
        }));
        let image_store = Arc::new(LocalStore);
        let schema = shipped_schema("mock/image-gen");
        let router = ProxyRouter::new(registry, cache, config, image_store);

        let base = make_base("same prompt", "mock/image-gen");
        let mut extras_a = make_image_extras();
        extras_a.size = Some("512x512".into());
        let mut extras_b = make_image_extras();
        extras_b.size = Some("1024x1024".into());
        let materialized = empty_materialized();

        let _ = router.generate_image(&schema, &base, &extras_a, &materialized, None).await.unwrap();
        let _ = router.generate_image(&schema, &base, &extras_b, &materialized, None).await.unwrap();
        // Different size → cache miss → provider called twice.
        assert_eq!(call_counter.load(Ordering::SeqCst), 2,
            "Different size must NOT share a cache entry");
    }

    #[tokio::test]
    async fn get_video_status_returns_not_found_for_unknown_id() {
        let config = Arc::new(AppConfig::default());
        let registry = Arc::new(ProviderRegistry::new());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = ProxyRouter::new(registry, cache, config, image_store);

        let result = router.get_video_status("does-not-exist").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.status_code(), 404, "Expected 404 for unknown video id");
    }

    // ─── Circuit breaker: breaker opens after threshold and routes to fallback ──

    #[tokio::test]
    async fn circuit_breaker_opens_and_routes_to_fallback() {
        // AlwaysFailingProvider (retryable) is listed first.
        // Threshold = 2. After 2 consecutive failures to "always-failing",
        // breaker opens. The next call should skip it and go straight to "mock".
        use crate::config::CircuitBreakerConfig;

        let config = Arc::new(AppConfig {
            model_routes: vec![ModelRouteConfig {
                model: "mock/image-gen".to_string(),
                deployments: vec![
                    DeploymentConfig {
                        provider: "always-failing".to_string(),
                        weight: 1,
                        max_retries: 0, // no retries per attempt so each call = 1 failure recorded
                        timeout_seconds: 30,
                        rpm_limit: 0,
                    },
                    DeploymentConfig {
                        provider: "mock".to_string(),
                        weight: 1,
                        max_retries: 0,
                        timeout_seconds: 30,
                        rpm_limit: 0,
                    },
                ],
                strategy: Some("fallback".to_string()),
                cache: None,
            }],
            circuit_breaker: CircuitBreakerConfig {
                threshold: 2,
                open_for_seconds: 60,
            },
            ..AppConfig::default()
        });

        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_image_provider_named("always-failing", Arc::new(AlwaysFailingProvider))
            .await;
        {
            use crate::providers::image::mock::MockProvider;
            let mut mp = MockProvider::new();
            mp.configure(default_instance_config());
            registry.register_mock_image(Arc::new(mp)).await;
        }

        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = ProxyRouter::new(registry, cache, config, image_store);

        let schema = shipped_schema("mock/image-gen");
        let base = make_base("circuit breaker test", "mock/image-gen");
        let extras = make_image_extras();
        let materialized = empty_materialized();

        // First two calls: "always-failing" is tried (and fails), "mock" picks up.
        // Each call records one failure on "always-failing".
        for i in 0..2 {
            let r = router.generate_image(&schema, &base, &extras, &materialized, None).await;
            assert!(r.is_ok(), "Call {} should succeed via fallback to mock: {:?}", i, r.err());
        }

        // At this point the breaker for "always-failing" has 2 failures and is open (threshold=2).
        assert!(
            router.circuit_breaker.is_open("always-failing").await,
            "Breaker for always-failing should be open after 2 failures"
        );

        // Third call: breaker is open, "always-failing" is skipped, "mock" serves directly.
        let r = router.generate_image(&schema, &base, &extras, &materialized, None).await;
        assert!(r.is_ok(), "Third call should succeed immediately via fallback (breaker open): {:?}", r.err());
        let resp = r.unwrap();
        assert_eq!(resp.provider, "mock", "Should route to mock (always-failing breaker is open)");
    }
}
