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
    use crate::proxy::router::{OrgCredentials, ProxyRouter};
    use crate::proxy::storage::{ImageStore, ImageStoreError, LocalStore};
    use crate::providers::{
        GenerationOutput, HealthCheckResult, ImageExtras, ImageProvider,
        ProviderError, ProviderInstanceConfig, build_cost_estimate,
    };
    use crate::types::*;

    /// A database for the calls that take one (a video poll's claim): an
    /// empty one, in memory.
    async fn test_db() -> Arc<dyn crate::db::DatabaseStore> {
        Arc::new(crate::db::SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory sqlite"))
    }

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
            vendor_job: Default::default(),
            size: None,
            resolution: None, aspect_ratio: None,
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            edit: None, views: Vec::new(),
            extra: None,
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
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
                more: Vec::new(),
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
                // A refused submit (the vendor took nothing), the one kind of
                // retryable failure litegen re-sends. A timeout never is.
                Err(ProviderError::RequestFailed {
                    message: "503 from the vendor at submit".into(),
                    status_code: Some(503),
                    provider_error: None,
                    retryable: true,
                })
            } else {
                Ok(GenerationOutput {
                    data: vec![0u8; 4],
                    content_type: "image/png".to_string(),
                    metadata: HashMap::new(),
                    more: Vec::new(),
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
            .generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, None)
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

    // ─── Billing: charge for images delivered, not requested n ────────────────

    #[tokio::test]
    async fn image_billing_charges_for_images_returned_not_requested_n() {
        // The proxy returns a single image per generation (GenerationOutput
        // carries one image), so billing must reflect the images actually
        // delivered — NOT the requested `n`, or an n>1 request is over-charged
        // for images it never receives.
        let registry = Arc::new(ProviderRegistry::new());
        {
            use crate::providers::image::mock::MockProvider;
            let mut mp = MockProvider::new();
            mp.configure(default_instance_config());
            registry.register_mock_image(Arc::new(mp)).await;
        }
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        // Default config => no routes (direct dispatch) and 0% markup.
        let config = Arc::new(AppConfig::default());
        let router = ProxyRouter::new(registry, cache, config, image_store);

        let mut schema = shipped_schema("mock/image-gen");
        schema.pricing.base_cost_usd = 0.10; // non-zero so delivered vs n differ

        let mut base = make_base("billing test", "mock/image-gen");
        base.n = 2; // request 2 — but the proxy only returns 1

        let extras = make_image_extras();
        let materialized = empty_materialized();

        let response = router
            .generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, None)
            .await
            .expect("generate_image should succeed");

        let returned = response.data.len();
        assert_eq!(returned, 1, "proxy returns a single image per generation");

        let cost = response.usage.as_ref().expect("usage present").cost_usd;
        let expected = 0.10 * returned as f64; // bill for delivered (1), not n (2)
        assert!(
            (cost - expected).abs() < 1e-9,
            "billed cost {cost} must equal per-image 0.10 × {returned} delivered, not × n=2"
        );
    }

    // ─── Billing: video charges for the single video delivered, not requested n ─

    #[tokio::test]
    async fn video_billing_charges_for_one_video_not_requested_n() {
        // generate_video submits exactly one provider job and returns one
        // VideoGenerationResponse (a single video_url), so billing must reflect
        // the one video actually delivered — NOT the requested `n`, which is
        // meaningless for video and unbounded on the wire. This mirrors the
        // image-path billing fix (charge for delivered, not requested).
        let registry = Arc::new(ProviderRegistry::new());
        {
            use crate::providers::video::mock::MockVideoProvider;
            use crate::providers::VideoProvider as _;
            let mut mp = MockVideoProvider::new();
            mp.configure(default_instance_config());
            registry.register_mock_video(Arc::new(mp)).await;
        }
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        // Default config => direct dispatch and 0% markup.
        let config = Arc::new(AppConfig::default());
        let router = ProxyRouter::new(registry, cache, config, image_store);

        let mut schema = shipped_schema("mock/video-gen");
        schema.pricing.base_cost_usd = 1.00; // non-zero so 1 vs n differ

        let mut base = make_base("billing test", "mock/video-gen");
        base.n = 5; // request 5 — but the proxy submits/returns exactly one video

        let extras = crate::providers::VideoExtras {
            duration_seconds: 5.0,
            aspect_ratio: None,
            resolution: None,
            fps: None,
            extra: None,
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
        };
        let materialized = empty_materialized();

        let response = router
            .generate_video(&schema, &base, &extras, &materialized, OrgCredentials::default())
            .await
            .expect("generate_video should succeed");

        let cost = response.usage.as_ref().expect("usage present").cost_usd;
        assert!(
            (cost - 1.00).abs() < 1e-9,
            "billed cost {cost} must equal per-video 1.00 × 1 delivered, not × n=5"
        );
    }

    /// A video job the router took is let go once it is older than any
    /// observer can need it (P3): the poller reaps a job still active at 2 h,
    /// so a job past the 3 h age has a terminal row. Before, only a client's
    /// GET on this replica or a cancel let a job go, and the map grew until a
    /// restart, each job with its provider instance.
    #[tokio::test(start_paused = true)]
    async fn a_video_job_past_the_reap_age_is_let_go() {
        let registry = Arc::new(ProviderRegistry::new());
        {
            use crate::providers::video::mock::MockVideoProvider;
            use crate::providers::VideoProvider as _;
            let mut mp = MockVideoProvider::new();
            mp.configure(default_instance_config());
            registry.register_mock_video(Arc::new(mp)).await;
        }
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let router = ProxyRouter::new(registry, cache, Arc::new(AppConfig::default()), Arc::new(LocalStore));
        let schema = shipped_schema("mock/video-gen");
        let extras = || crate::providers::VideoExtras {
            duration_seconds: 5.0,
            aspect_ratio: None,
            resolution: None,
            fps: None,
            extra: None,
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
        };

        let old = router
            .generate_video(&schema, &make_base("old", "mock/video-gen"), &extras(), &empty_materialized(), OrgCredentials::default())
            .await
            .expect("the first video is submitted");
        assert!(router.get_provider_job_id(&old.id).await.is_some(), "the job is tracked");
        tokio::time::advance(std::time::Duration::from_secs(3 * 3600 + 1)).await;
        let new = router
            .generate_video(&schema, &make_base("new", "mock/video-gen"), &extras(), &empty_materialized(), OrgCredentials::default())
            .await
            .expect("the second video is submitted");
        assert!(router.get_provider_job_id(&new.id).await.is_some(), "the new job is tracked");
        assert_eq!(router.get_provider_job_id(&old.id).await, None, "a job past the reap age is let go");
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
            .generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, None)
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

        let result = router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, None).await;
        assert!(result.is_ok(), "Expected success after retries, got: {:?}", result.err());

        let attempts = counter.load(Ordering::SeqCst);
        assert_eq!(attempts, 3, "Expected 3 total attempts (0, 1 fail; 2 success), got {}", attempts);
    }

    // ─── G2.2: Cache hit returns cached response without calling provider ─────

    #[tokio::test]
    async fn cache_hit_does_not_call_provider_second_time() {
        let (counting_provider, call_counter) = CountingProvider::new("mock");

        let config = Arc::new(cache_on());

        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_image(Arc::new(counting_provider)).await;

        // Enable cache
        let cache_config = CacheGlobalConfig {
            enabled: true,
            default_ttl_seconds: 3600,
            max_items: 100,
            ..CacheGlobalConfig::default()
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
        let r1 = router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, None).await;
        assert!(r1.is_ok(), "First call failed: {:?}", r1.err());

        // Second call with same prompt — should hit cache
        let r2 = router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, None).await;
        assert!(r2.is_ok(), "Second call failed: {:?}", r2.err());

        let count = call_counter.load(Ordering::SeqCst);
        assert_eq!(count, 1, "Provider should have been called once (cache hit on 2nd call), got {}", count);
    }

    /// Answers once the test opens its gate, and says when it has the request.
    struct GatedProvider {
        reached: Arc<tokio::sync::Notify>,
        open: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl ImageProvider for GatedProvider {
        fn name(&self) -> &str {
            "mock"
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
            self.reached.notify_one();
            self.open.notified().await;
            Ok(GenerationOutput {
                data: vec![0u8; 4],
                content_type: "image/png".to_string(),
                metadata: HashMap::new(),
                more: Vec::new(),
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

    /// An operator who turns the cache off while a request is with the vendor
    /// must not find that request's answer stored anyway: a later switch-on
    /// would serve it.
    #[tokio::test]
    async fn an_answer_that_arrives_after_the_cache_was_turned_off_is_not_kept() {
        let (reached, open) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_image(Arc::new(GatedProvider { reached: reached.clone(), open: open.clone() })).await;
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig {
            enabled: true,
            default_ttl_seconds: 3600,
            max_items: 100,
            ..CacheGlobalConfig::default()
        }));
        let router = Arc::new(ProxyRouter::new(registry, cache.clone(), Arc::new(cache_on()), Arc::new(LocalStore)));
        let generating = tokio::spawn({
            let router = router.clone();
            async move {
                let (schema, base) = (shipped_schema("mock/image-gen"), make_base("in flight", "mock/image-gen"));
                let (extras, materialized) = (make_image_extras(), empty_materialized());
                router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, None).await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), reached.notified()).await.expect("the vendor has the request");
        // Turned off while the vendor works, as PUT /v1/admin/settings applies it.
        let mut overrides = router.overrides();
        overrides.gateway.cache_enabled = Some(false);
        router.set_overrides(overrides);
        open.notify_one();
        generating.await.unwrap().expect("the request itself succeeds");
        assert_eq!(cache.entry_count().await, 0, "an answer that came back after the cache was turned off must not be stored");
    }

    // ─── Cache is isolated per tenant (no cross-tenant cache hits) ─────────────

    #[tokio::test]
    async fn cache_isolates_by_tenant() {
        let (counting_provider, call_counter) = CountingProvider::new("mock");
        let config = Arc::new(cache_on());
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_image(Arc::new(counting_provider)).await;
        let cache_config = CacheGlobalConfig { enabled: true, default_ttl_seconds: 3600, max_items: 100, ..CacheGlobalConfig::default() };
        let cache = Arc::new(GenerationCache::new(&cache_config));
        let router = ProxyRouter::new(registry, cache, config, Arc::new(LocalStore));

        let schema = shipped_schema("mock/image-gen");
        let base = make_base("same prompt across tenants", "mock/image-gen");
        let extras = make_image_extras();
        let materialized = empty_materialized();

        // Tenant A: provider invoked, result cached under app-a's key.
        assert!(router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, Some("app-a")).await.is_ok());
        // Tenant B, identical request: MUST NOT receive A's cached result.
        assert!(router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, Some("app-b")).await.is_ok());
        // Tenant A again: hits A's own cache entry (no new provider call).
        assert!(router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, Some("app-a")).await.is_ok());

        let count = call_counter.load(Ordering::SeqCst);
        assert_eq!(count, 2, "one provider call per distinct tenant (A,B); A's 2nd is a cache hit; got {}", count);
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

        let result = router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, None).await;
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
        let config = Arc::new(cache_on());
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_image(Arc::new(counting_provider)).await;
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig {
            enabled: true, default_ttl_seconds: 3600, max_items: 100, ..CacheGlobalConfig::default()
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

        let _ = router.generate_image(&schema, &base, &extras_a, &materialized, OrgCredentials::default(), None, None).await.unwrap();
        let _ = router.generate_image(&schema, &base, &extras_b, &materialized, OrgCredentials::default(), None, None).await.unwrap();
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

        let result = router.get_video_status("does-not-exist", None, None, true, &test_db().await).await;
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
            let r = router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, None).await;
            assert!(r.is_ok(), "Call {} should succeed via fallback to mock: {:?}", i, r.err());
        }

        // At this point the breaker for "always-failing" has 2 failures and is open (threshold=2).
        assert!(
            router.circuit_breaker.is_open("always-failing").await,
            "Breaker for always-failing should be open after 2 failures"
        );

        // Third call: breaker is open, "always-failing" is skipped, "mock" serves directly.
        let r = router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, None).await;
        assert!(r.is_ok(), "Third call should succeed immediately via fallback (breaker open): {:?}", r.err());
        let resp = r.unwrap();
        assert_eq!(resp.provider, "mock", "Should route to mock (always-failing breaker is open)");
    }

    // ─── E9: what a refusal is, and what opens a breaker ─────────────────────

    /// An image provider named `name` whose every call fails with what
    /// `fail` builds.
    struct FailsWith {
        name: &'static str,
        fail: fn() -> ProviderError,
    }

    #[async_trait]
    impl ImageProvider for FailsWith {
        fn name(&self) -> &str {
            self.name
        }
        fn configure(&mut self, _config: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool {
            true
        }
        async fn generate(&self, _m: &ModelSchema, _b: &BaseGenerationRequest, _e: &ImageExtras, _r: &MaterializedRequest) -> Result<GenerationOutput, ProviderError> {
            Err((self.fail)())
        }
        async fn estimate_cost(&self, model: &ModelSchema, _request: &ImageGenerationRequest) -> Result<CostEstimate, ProviderError> {
            Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
        }
        async fn health_check(&self) -> HealthCheckResult {
            HealthCheckResult { healthy: true, message: String::new(), latency_ms: None }
        }
    }

    /// The adapter's own refusal, before anything is sent.
    fn refuses_before_calling() -> ProviderError {
        ProviderError::InvalidRequest("cannot make this".into())
    }

    /// The vendor's refusal of this one request (a moderation 422, say).
    fn vendor_refuses_the_request() -> ProviderError {
        ProviderError::RequestFailed { message: "prompt flagged".into(), status_code: Some(422), provider_error: None, retryable: false }
    }

    /// A router whose `mock/image-gen` route tries `deployments` in order,
    /// once each, with a breaker that opens after `threshold` failures. The
    /// deployments are `always-failing` (a retryable 500), `mock` and `extra`.
    async fn routed(deployments: &[&str], threshold: u32, extra: Vec<FailsWith>) -> ProxyRouter {
        use crate::config::CircuitBreakerConfig;
        let config = Arc::new(AppConfig {
            model_routes: vec![ModelRouteConfig {
                model: "mock/image-gen".to_string(),
                deployments: deployments
                    .iter()
                    .map(|p| DeploymentConfig { provider: p.to_string(), weight: 1, max_retries: 0, timeout_seconds: 30, rpm_limit: 0 })
                    .collect(),
                strategy: Some("fallback".to_string()),
                cache: None,
            }],
            circuit_breaker: CircuitBreakerConfig { threshold, open_for_seconds: 60 },
            ..AppConfig::default()
        });
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_image_provider_named("always-failing", Arc::new(AlwaysFailingProvider)).await;
        for p in extra {
            registry.register_image_provider_named(p.name, Arc::new(p)).await;
        }
        {
            use crate::providers::image::mock::MockProvider;
            let mut mp = MockProvider::new();
            mp.configure(default_instance_config());
            registry.register_mock_image(Arc::new(mp)).await;
        }
        ProxyRouter::new(registry, Arc::new(GenerationCache::new(&CacheGlobalConfig::default())), config, Arc::new(LocalStore))
    }

    async fn generate(router: &ProxyRouter) -> Result<ImageGenerationResponse, crate::proxy::router::ProxyError> {
        let schema = shipped_schema("mock/image-gen");
        router
            .generate_image(&schema, &make_base("e9", "mock/image-gen"), &make_image_extras(), &empty_materialized(), OrgCredentials::default(), None, None)
            .await
    }

    /// E9: an adapter's refusal before any call is the caller's request,
    /// not the deployment's health: it fails over, and never opens the
    /// breaker every tenant shares.
    #[tokio::test]
    async fn an_adapter_refusal_does_not_count_against_the_breaker() {
        let router = routed(&["refuses", "mock"], 2, vec![FailsWith { name: "refuses", fail: refuses_before_calling }]).await;
        for i in 0..2 {
            let r = generate(&router).await;
            assert!(r.is_ok(), "call {i} fails over to mock: {:?}", r.err());
        }
        assert!(!router.circuit_breaker.is_open("refuses").await, "two refusals of the caller's request opened the breaker");
    }

    /// E9 (finding 2): a vendor's refusal of one request (a 4xx such as a
    /// moderation 422) doesn't say the deployment is unwell either.
    #[tokio::test]
    async fn a_vendors_refusal_of_the_request_does_not_count_against_the_breaker() {
        let router = routed(&["refuses", "mock"], 2, vec![FailsWith { name: "refuses", fail: vendor_refuses_the_request }]).await;
        for i in 0..2 {
            let r = generate(&router).await;
            assert!(r.is_ok(), "call {i} fails over to mock: {:?}", r.err());
        }
        assert!(!router.circuit_breaker.is_open("refuses").await, "two vendor 422s opened the breaker");
    }

    /// E9b, then handlers review #5: a route that asked no deployment because
    /// every breaker is open answers a retryable 503 that says when to try
    /// again (`Retry-After`: the window's time left, at most its 60 s) and
    /// reached no vendor, so it spends no key. E9b made it a retryable 502;
    /// before that it was a 400 "Provider not configured" marked as a
    /// vendor attempt.
    #[tokio::test]
    async fn a_route_whose_every_breaker_is_open_answers_a_503_with_the_wait_and_spends_no_key() {
        let router = routed(&["always-failing"], 1, vec![]).await;
        let first = generate(&router).await.expect_err("the only deployment fails");
        assert!(first.reached_vendor() && first.status_code() == 502, "{first:?}");
        let skipped = generate(&router).await.expect_err("its breaker is open");
        assert_eq!(skipped.status_code(), 503, "{skipped:?}");
        assert!(skipped.retryable(), "{skipped:?}");
        assert!(!skipped.reached_vendor(), "nothing was sent: {skipped:?}");
        assert!(skipped.retry_after_secs().is_some_and(|s| (1..=60).contains(&s)), "{skipped:?}");
        assert!(!skipped.to_string().contains("Some("), "{skipped}");
    }

    /// Handlers review #5: a route none of whose providers is configured
    /// answers 400 `provider_not_configured`, as a direct request to that
    /// provider does: no retry can help. It answered a retryable 502.
    #[tokio::test]
    async fn a_route_with_no_configured_deployment_answers_provider_not_configured() {
        let router = routed(&["nowhere"], 5, vec![]).await;
        let err = generate(&router).await.expect_err("no deployment can serve it");
        assert!(matches!(&err, crate::proxy::router::ProxyError::ProviderNotConfigured { provider, .. } if provider == "nowhere"), "{err:?}");
        assert_eq!((err.status_code(), err.retryable(), err.reached_vendor()), (400, false, false), "{err:?}");
    }

    /// Handlers review #4: once the primary's breaker is open, a later
    /// deployment's refusal no longer ends the request with a final 400:
    /// the primary may serve it when its breaker closes, so the answer is
    /// the retryable 502 that names the skip.
    #[tokio::test]
    async fn a_skipped_primary_keeps_a_later_refusal_retryable() {
        let router = routed(&["always-failing", "refuses"], 1, vec![FailsWith { name: "refuses", fail: refuses_before_calling }]).await;
        let first = generate(&router).await.expect_err("the primary fails, the second refuses");
        assert!(first.reached_vendor(), "the primary's vendor failure wins: {first:?}");
        let err = generate(&router).await.expect_err("the primary is skipped, the second refuses");
        assert_eq!((err.status_code(), err.retryable(), err.reached_vendor()), (502, true, false), "{err:?}");
        assert!(err.to_string().contains("always-failing: circuit open"), "{err}");
    }

    /// E9: a later deployment's refusal never hides an earlier deployment's
    /// real vendor failure, which may have billed: the answer is the vendor
    /// failure (its key spent), not the refusal.
    #[tokio::test]
    async fn a_vendor_failure_is_not_hidden_by_a_later_refusal() {
        let router = routed(&["always-failing", "refuses"], 5, vec![FailsWith { name: "refuses", fail: refuses_before_calling }]).await;
        let err = generate(&router).await.expect_err("both deployments fail");
        assert!(err.reached_vendor(), "{err:?}");
        assert!(matches!(err, crate::proxy::router::ProxyError::VendorFailed { .. }), "{err:?}");
        assert_eq!(err.status_code(), 502, "{err:?}");
    }

    // ─── L1: a vendor call is never re-sent once it may have been billed ─────

    /// Records the vendor's job id (as a submit-then-poll adapter does the
    /// moment the vendor accepts) and then fails retryably: a poll that broke
    /// after the vendor took the job.
    struct AcceptsThenFails {
        counter: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ImageProvider for AcceptsThenFails {
        fn name(&self) -> &str { "mock" }
        fn configure(&mut self, _config: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }
        async fn generate(
            &self,
            _model: &ModelSchema,
            _base: &BaseGenerationRequest,
            extras: &ImageExtras,
            _materialized: &MaterializedRequest,
        ) -> Result<GenerationOutput, ProviderError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            extras.vendor_job.record("vendor-job-7");
            Err(ProviderError::RequestFailed {
                message: "poll failed".into(),
                status_code: Some(502),
                provider_error: None,
                retryable: true,
            })
        }
        async fn estimate_cost(&self, model: &ModelSchema, _r: &ImageGenerationRequest) -> Result<CostEstimate, ProviderError> {
            Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
        }
        async fn health_check(&self) -> HealthCheckResult {
            HealthCheckResult { healthy: true, message: "ok".into(), latency_ms: None }
        }
    }

    /// Never answers within any test's vendor timeout.
    struct NeverAnswers {
        name: &'static str,
        counter: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ImageProvider for NeverAnswers {
        fn name(&self) -> &str { self.name }
        fn configure(&mut self, _config: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }
        async fn generate(
            &self,
            _model: &ModelSchema,
            _base: &BaseGenerationRequest,
            extras: &ImageExtras,
            _materialized: &MaterializedRequest,
        ) -> Result<GenerationOutput, ProviderError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            extras.vendor_job.record("vendor-job-slow");
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            unreachable!("the router gives up first")
        }
        async fn estimate_cost(&self, model: &ModelSchema, _r: &ImageGenerationRequest) -> Result<CostEstimate, ProviderError> {
            Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
        }
        async fn health_check(&self) -> HealthCheckResult {
            HealthCheckResult { healthy: true, message: "ok".into(), latency_ms: None }
        }
    }

    fn one_second_timeout() -> Arc<AppConfig> {
        let mut c = AppConfig::default();
        c.server.vendor_timeout_secs = Some(1);
        Arc::new(c)
    }

    #[tokio::test]
    async fn a_job_the_vendor_accepted_is_never_sent_again() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_image(Arc::new(AcceptsThenFails { counter: counter.clone() })).await;
        let router = ProxyRouter::new(
            registry,
            Arc::new(GenerationCache::new(&CacheGlobalConfig::default())),
            Arc::new(AppConfig::default()),
            Arc::new(LocalStore),
        );
        let err = router
            .generate_image(&shipped_schema("mock/image-gen"), &make_base("x", "mock/image-gen"), &make_image_extras(), &empty_materialized(), OrgCredentials::default(), None, None)
            .await
            .expect_err("the poll failed");
        assert_eq!(counter.load(Ordering::SeqCst), 1, "retryable, but the vendor already took the job");
        assert!(err.to_string().contains("vendor-job-7"), "{err}");
    }

    #[tokio::test]
    async fn a_timed_out_attempt_is_never_sent_again() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_image(Arc::new(NeverAnswers { name: "mock", counter: counter.clone() })).await;
        let router = ProxyRouter::new(
            registry,
            Arc::new(GenerationCache::new(&CacheGlobalConfig::default())),
            one_second_timeout(),
            Arc::new(LocalStore),
        );
        let err = router
            .generate_image(&shipped_schema("mock/image-gen"), &make_base("x", "mock/image-gen"), &make_image_extras(), &empty_materialized(), OrgCredentials::default(), None, None)
            .await
            .expect_err("timed out");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert_eq!(err.status_code(), 504, "{err}");
        assert!(matches!(&err, crate::proxy::router::ProxyError::VendorTimeout { vendor_job_id: Some(j), .. } if j == "vendor-job-slow"), "{err:?}");
    }

    #[tokio::test]
    async fn a_deployment_that_timed_out_does_not_fall_through_to_the_next() {
        let slow = Arc::new(AtomicUsize::new(0));
        let (fast, fast_count) = CountingProvider::new("fast");
        let config = Arc::new(AppConfig {
            model_routes: vec![ModelRouteConfig {
                model: "mock/image-gen".to_string(),
                deployments: vec![
                    DeploymentConfig { provider: "slow".into(), weight: 1, max_retries: 2, timeout_seconds: 1, rpm_limit: 0 },
                    DeploymentConfig { provider: "fast".into(), weight: 1, max_retries: 0, timeout_seconds: 30, rpm_limit: 0 },
                ],
                strategy: Some("fallback".to_string()),
                cache: None,
            }],
            ..AppConfig::default()
        });
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_image_provider_named("slow", Arc::new(NeverAnswers { name: "slow", counter: slow.clone() })).await;
        registry.register_image_provider_named("fast", Arc::new(fast)).await;
        let router = ProxyRouter::new(registry, Arc::new(GenerationCache::new(&CacheGlobalConfig::default())), config, Arc::new(LocalStore));
        let err = router
            .generate_image(&shipped_schema("mock/image-gen"), &make_base("x", "mock/image-gen"), &make_image_extras(), &empty_materialized(), OrgCredentials::default(), None, None)
            .await
            .expect_err("the first deployment may still bill its job");
        assert_eq!(err.status_code(), 504, "{err}");
        assert_eq!(slow.load(Ordering::SeqCst), 1, "not retried on its own deployment");
        assert_eq!(fast_count.load(Ordering::SeqCst), 0, "nor sent to the next");
    }

    /// A config with the cache on: whether answers are cached is the
    /// gateway's settings' call (`docs/gateway-settings.md`).
    fn cache_on() -> AppConfig {
        let mut config = AppConfig::default();
        config.cache.enabled = true;
        config
    }

    /// The platform's `mock` instance, as an org that brings its own key
    /// would find it: every poll fails, as the platform key would for a job
    /// it never made.
    struct PlatformKeyFails;

    #[async_trait::async_trait]
    impl crate::providers::VideoProvider for PlatformKeyFails {
        fn name(&self) -> &str {
            "mock"
        }
        fn configure(&mut self, _config: crate::providers::ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool {
            true
        }
        async fn generate(
            &self,
            _m: &ModelSchema,
            _b: &BaseGenerationRequest,
            _e: &crate::providers::VideoExtras,
            _r: &MaterializedRequest,
        ) -> Result<crate::providers::VideoGenerationHandle, ProviderError> {
            unreachable!("the job is submitted with the org's own key")
        }
        async fn poll_status(&self, _h: &crate::providers::VideoGenerationHandle) -> Result<crate::providers::VideoGenerationPollResult, ProviderError> {
            Err(ProviderError::RequestFailed { message: "the platform key knows no such job".into(), status_code: Some(404), provider_error: None, retryable: false })
        }
        async fn estimate_cost(&self, _m: &ModelSchema, _r: &crate::types::VideoGenerationRequest) -> Result<CostEstimate, ProviderError> {
            unreachable!()
        }
        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            unreachable!()
        }
    }

    /// A job is polled with the instance that took it: an org's own key, not
    /// the platform's (known gap: client polls used the platform instance).
    #[tokio::test]
    async fn a_video_is_polled_with_the_instance_that_took_it() {
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_video(Arc::new(PlatformKeyFails)).await;
        let router = ProxyRouter::new(
            registry,
            Arc::new(GenerationCache::new(&CacheGlobalConfig::default())),
            Arc::new(AppConfig::default()),
            Arc::new(LocalStore),
        );
        let schema = shipped_schema("mock/video-gen");
        let base = make_base("byo polling", "mock/video-gen");
        let extras = crate::providers::VideoExtras {
            duration_seconds: 5.0,
            aspect_ratio: None,
            resolution: None,
            fps: None,
            extra: None,
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
        };
        let byo = crate::providers::auth::ProviderCredentials { api_key: Some("the-orgs-own-key".into()), ..Default::default() };
        let submitted = router.generate_video(&schema, &base, &extras, &empty_materialized(), OrgCredentials::of("mock", Some(byo.clone()))).await.expect("submitted with the org's key");
        let polled = router.get_video_status(&submitted.id, None, None, true, &test_db().await).await;
        assert!(polled.is_ok(), "polled with the org's instance, not the platform's: {:?}", polled.err());

        // The same through a model route: the deployment's instance is kept.
        let routed_registry = Arc::new(ProviderRegistry::new());
        routed_registry.register_mock_video(Arc::new(PlatformKeyFails)).await;
        let routed = ProxyRouter::new(
            routed_registry,
            Arc::new(GenerationCache::new(&CacheGlobalConfig::default())),
            Arc::new(AppConfig {
                model_routes: vec![ModelRouteConfig {
                    model: "mock/video-gen".to_string(),
                    deployments: vec![DeploymentConfig { provider: "mock".to_string(), weight: 1, max_retries: 0, timeout_seconds: 30, rpm_limit: 0 }],
                    strategy: None,
                    cache: None,
                }],
                ..AppConfig::default()
            }),
            Arc::new(LocalStore),
        );
        let submitted = routed.generate_video(&schema, &base, &extras, &empty_materialized(), OrgCredentials::of("mock", Some(byo))).await.expect("routed, with the org's key");
        let polled = routed.get_video_status(&submitted.id, None, None, true, &test_db().await).await;
        assert!(polled.is_ok(), "a routed job keeps its deployment's instance: {:?}", polled.err());
    }

    /// What the log lines written while the guard lives said.
    #[derive(Clone, Default)]
    struct Logs(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Logs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
        type Writer = Logs;
        fn make_writer(&'a self) -> Logs {
            self.clone()
        }
    }

    /// Payloads review #8: a client's poll runs in a span naming the job
    /// (`id`), as it did before the poll handed back its delivery claim. The
    /// handlers call the `_claimed` polls; only the wrappers, which tests
    /// alone call, opened the span.
    #[tokio::test]
    async fn a_client_poll_runs_in_a_span_naming_its_job() {
        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NEW)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let router = ProxyRouter::new(
            Arc::new(ProviderRegistry::new()),
            Arc::new(GenerationCache::new(&CacheGlobalConfig::default())),
            Arc::new(AppConfig::default()),
            Arc::new(LocalStore),
        );
        let db = test_db().await;
        assert!(router.get_video_status_claimed("p8-video-job", None, None, true, &db).await.is_err(), "no such job");
        assert!(router.get_model3d_status_claimed("p8-3d-job", None, None, &db).await.is_err(), "no such job");
        let text = String::from_utf8_lossy(&logs.0.lock().unwrap()).into_owned();
        // A span's line names it, then its fields: `name{... id=<job> ...}`.
        let opened = |name: &str, id: &str| text.lines().any(|l| l.contains(&format!("{name}{{")) && l.contains(&format!("id={id}")));
        assert!(opened("get_video_status", "p8-video-job"), "a client's video poll opened no span naming its job: {text:?}");
        assert!(opened("get_model3d_status", "p8-3d-job"), "a client's 3D poll opened no span naming its job: {text:?}");
    }
}

