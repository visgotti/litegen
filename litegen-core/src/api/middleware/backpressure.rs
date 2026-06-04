use tokio::sync::{Semaphore, SemaphorePermit, TryAcquireError};

/// Global in-flight request limiter backed by a Tokio semaphore.
///
/// Handlers call `try_acquire()` at the top of their function.  If it returns
/// `None` the semaphore is exhausted and the handler should immediately respond
/// with `503 Service Unavailable` + `Retry-After: 1`.
///
/// The permit is dropped automatically when the handler returns, freeing a
/// slot for the next waiting caller.
pub struct InFlightLimit {
    sem: Semaphore,
    cap: usize,
}

impl InFlightLimit {
    /// Create a new limiter with the given capacity.
    pub fn new(cap: usize) -> Self {
        Self {
            sem: Semaphore::new(cap),
            cap,
        }
    }

    /// Try to acquire one slot.  Returns `Some(permit)` if a slot was
    /// available, or `None` if the semaphore is exhausted.
    pub fn try_acquire(&self) -> Option<SemaphorePermit<'_>> {
        match self.sem.try_acquire() {
            Ok(permit) => Some(permit),
            Err(TryAcquireError::NoPermits) => None,
            Err(TryAcquireError::Closed) => None,
        }
    }

    /// Return the configured capacity (for diagnostics / tests).
    pub fn capacity(&self) -> usize {
        self.cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use crate::db::sqlite::SqliteDatabase;
    use crate::capabilities::CapabilityRegistry;
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::LocalStore;
    use crate::proxy::materializer::{Materializer, MaterializeError, TempStorage};
    use crate::api::middleware::{AppState, rate_limit::RateLimiter};
    use bytes::Bytes;

    #[test]
    fn semaphore_limits_to_cap() {
        let limiter = InFlightLimit::new(2);
        let p1 = limiter.try_acquire();
        let p2 = limiter.try_acquire();
        let p3 = limiter.try_acquire();

        assert!(p1.is_some(), "first permit should succeed");
        assert!(p2.is_some(), "second permit should succeed");
        assert!(p3.is_none(), "third permit should fail (cap=2)");

        drop(p1);
        // After releasing one, should be able to acquire again
        let p4 = limiter.try_acquire();
        assert!(p4.is_some(), "permit after release should succeed");
    }

    struct NoopStorage;

    #[async_trait::async_trait]
    impl TempStorage for NoopStorage {
        async fn put(&self, key: &str, _bytes: Bytes, _ct: &str) -> Result<String, MaterializeError> {
            Ok(format!("local://{}", key))
        }
        async fn delete(&self, _key: &str) -> Result<(), MaterializeError> { Ok(()) }
    }

    async fn build_test_state_with_backpressure(cap: usize) -> Arc<AppState> {
        use crate::providers::image::mock::MockProvider;
        use crate::providers::{ImageProvider, ProviderInstanceConfig};

        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
        let provider_registry = Arc::new(ProviderRegistry::new());

        // Register the mock image provider so `mock/image-gen` requests succeed.
        {
            let mut ip = MockProvider::new();
            ip.configure(ProviderInstanceConfig {
            credentials: Default::default(),
                api_key: String::new(),
                api_keys: vec![],
                api_base: None,
                model_mapping: Default::default(),
                extra_headers: Default::default(),
                options: None,
            });
            provider_registry.register_mock_image(Arc::new(ip)).await;
        }

        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = Arc::new(ProxyRouter::new(provider_registry, cache, config, image_store));
        let materializer = Arc::new(Materializer::new(
            Arc::new(NoopStorage), reqwest::Client::new(),
        ));
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); p.push("models");
        let cap_registry = Arc::new(
            CapabilityRegistry::from_dir(&p).expect("load shipped models")
        );
        Arc::new(AppState {
            router,
            db,
            master_key: None,
            registry: cap_registry,
            materializer,
            rate_limiter: Arc::new(RateLimiter::new()),
            in_flight: Arc::new(InFlightLimit::new(cap)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    /// With cap=1 and 5 concurrent image-generation requests, exactly 1 should
    /// succeed (200) and the remaining 4 should get 503 with Retry-After: 1.
    ///
    /// We hold the first permit manually to guarantee the other four see a full
    /// semaphore, then fire all five requests sequentially (the held permit
    /// simulates a slow in-flight request occupying the single slot).
    #[tokio::test]
    async fn backpressure_returns_503_when_at_capacity() {
        use crate::api::handlers::create_router;
        use axum::body::to_bytes;

        let state = build_test_state_with_backpressure(1).await;

        // Keep a separate Arc reference to the limiter so we can hold a permit
        // independently of the state's ownership move into create_router.
        let limiter = Arc::clone(&state.in_flight);
        let app = create_router(state);

        // Manually hold the one slot so all HTTP requests see the semaphore full.
        let held_permit = limiter.try_acquire().expect("should get initial permit");

        let make_request = || {
            Request::builder()
                .method("POST")
                .uri("/v1/images/generations")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&serde_json::json!({
                    "model": "mock/image-gen",
                    "prompt": "test"
                })).unwrap()))
                .unwrap()
        };

        // With the permit held, send 4 requests — all should get 503.
        let mut statuses = Vec::new();
        let mut retry_afters = Vec::new();
        for _ in 0..4 {
            let resp = app.clone().oneshot(make_request()).await.unwrap();
            let status = resp.status();
            let ra = resp.headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let _body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            statuses.push(status);
            retry_afters.push(ra);
        }

        // Release the held permit — the next request should succeed.
        drop(held_permit);
        let resp = app.clone().oneshot(make_request()).await.unwrap();
        let ok_status = resp.status();
        let _body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();

        // All 4 pre-release requests must be 503.
        for (i, s) in statuses.iter().enumerate() {
            assert_eq!(
                *s, StatusCode::SERVICE_UNAVAILABLE,
                "request {i} should be 503 while at capacity"
            );
            assert_eq!(
                retry_afters[i].as_deref(),
                Some("1"),
                "503 response {i} should carry Retry-After: 1"
            );
        }

        // Post-release request must succeed.
        assert_eq!(
            ok_status, StatusCode::OK,
            "request after permit release should succeed"
        );
    }
}
