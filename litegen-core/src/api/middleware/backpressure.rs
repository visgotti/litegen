use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, SemaphorePermit, TryAcquireError};

/// Global in-flight request limiter backed by a Tokio semaphore, and the
/// budget request bodies are read within.
///
/// The request extractor reads the (up to 25 MiB) body first, charging each
/// byte to [`InFlightLimit::body`] as it arrives, and takes the slot only once
/// the body is all there: a client that sends slowly holds the bytes it sent,
/// never a slot. The budget is `capacity` × 25 MiB, so the bodies in memory
/// stay bounded as the slots bounded them before. With every slot taken, or
/// the budget spent, the caller gets `503 Service Unavailable` +
/// `Retry-After: 1`; a body slower than the deadline gets 408 `body_timeout`.
///
/// The permit is dropped automatically when the request completes, freeing a
/// slot for the next waiting caller.
pub struct InFlightLimit {
    sem: Arc<Semaphore>,
    cap: usize,
    body: super::body::BodyBudget,
}

/// How long a request body may take to arrive when the config does not say
/// (`server.request_body_timeout_secs`).
pub const DEFAULT_BODY_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(crate::config::DEFAULT_REQUEST_BODY_TIMEOUT_SECS);

impl InFlightLimit {
    /// Create a new limiter with the given capacity.
    pub fn new(cap: usize) -> Self {
        Self::with_body_timeout(cap, DEFAULT_BODY_TIMEOUT)
    }

    /// [`InFlightLimit::new`], with request bodies given `body_timeout` to
    /// arrive.
    pub fn with_body_timeout(cap: usize, body_timeout: std::time::Duration) -> Self {
        Self {
            sem: Arc::new(Semaphore::new(cap)),
            cap,
            body: super::body::BodyBudget::new(cap, super::body::MAX_GENERATION_BODY, body_timeout),
        }
    }

    /// The budget request bodies are read within.
    pub fn body(&self) -> &super::body::BodyBudget {
        &self.body
    }

    /// Whether every slot is taken.
    pub fn is_full(&self) -> bool {
        self.sem.available_permits() == 0
    }

    /// Try to acquire one slot (borrowed permit, tied to `&self`).
    pub fn try_acquire(&self) -> Option<SemaphorePermit<'_>> {
        match self.sem.try_acquire() {
            Ok(permit) => Some(permit),
            Err(TryAcquireError::NoPermits) => None,
            Err(TryAcquireError::Closed) => None,
        }
    }

    /// Try to acquire one slot as an owned (`'static`) permit that can be held
    /// across body extraction and the handler by moving it into the request's
    /// validated data. Returns `None` when the semaphore is exhausted.
    pub fn try_acquire_owned(&self) -> Option<OwnedSemaphorePermit> {
        self.sem.clone().try_acquire_owned().ok()
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
        build_test_state_with_limit(InFlightLimit::new(cap)).await
    }

    async fn build_test_state_with_limit(in_flight: InFlightLimit) -> Arc<AppState> {
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
            in_flight: Arc::new(in_flight),

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

    // ─── P9: bodies are read before a slot is taken, within a deadline ────────

    use futures::StreamExt;

    const MIB: usize = 1024 * 1024;

    fn image_request(body: Body) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/images/generations")
            .header("content-type", "application/json")
            .body(body)
            .unwrap()
    }

    fn complete_image() -> Body {
        Body::from(serde_json::to_vec(&serde_json::json!({ "model": "mock/image-gen", "prompt": "test" })).unwrap())
    }

    /// The first bytes of a body, then nothing more; `polled` fires when the
    /// server first reads it.
    fn stalled_body(polled: tokio::sync::oneshot::Sender<()>) -> Body {
        let first = futures::stream::once(async move {
            let _ = polled.send(());
            Ok::<_, std::io::Error>(Bytes::from_static(b"{\"model\":"))
        });
        Body::from_stream(first.chain(futures::stream::pending()))
    }

    /// `mib` MiB, then nothing more; `taken_in` fires when the server asks for
    /// more after the last chunk, so it has taken in (and charged) all of it.
    fn mib_then_stall(mib: usize, taken_in: tokio::sync::oneshot::Sender<()>) -> Body {
        let chunks = futures::stream::iter((0..mib).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; MIB]))));
        let stall = futures::stream::once(async move {
            let _ = taken_in.send(());
            futures::future::pending::<Result<Bytes, std::io::Error>>().await
        });
        Body::from_stream(chunks.chain(stall))
    }

    async fn status_and_code(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn a_stalled_body_holds_no_generation_slot() {
        use crate::api::handlers::create_router;
        let app = create_router(build_test_state_with_backpressure(1).await);
        let (polled_tx, polled_rx) = tokio::sync::oneshot::channel();
        let stalled = tokio::spawn(app.clone().oneshot(image_request(stalled_body(polled_tx))));
        polled_rx.await.unwrap();
        let (status, body) = status_and_code(app.clone().oneshot(image_request(complete_image())).await.unwrap()).await;
        stalled.abort();
        assert_eq!(status, StatusCode::OK, "a stalled body held the only generation slot: {body}");
    }

    #[tokio::test]
    async fn a_body_that_stalls_past_the_deadline_is_answered_408() {
        use crate::api::handlers::create_router;
        let limit = InFlightLimit::with_body_timeout(1, std::time::Duration::from_millis(300));
        let app = create_router(build_test_state_with_limit(limit).await);
        let (polled_tx, _polled_rx) = tokio::sync::oneshot::channel();
        let stalled = tokio::spawn(app.clone().oneshot(image_request(stalled_body(polled_tx))));
        let Ok(answered) = tokio::time::timeout(std::time::Duration::from_secs(5), stalled).await else {
            panic!("the stalled request was never answered");
        };
        let (status, body) = status_and_code(answered.unwrap().unwrap()).await;
        assert_eq!(status, StatusCode::REQUEST_TIMEOUT, "{body}");
        assert_eq!(body["error"]["code"], "body_timeout", "{body}");
        let (status, body) = status_and_code(app.clone().oneshot(image_request(complete_image())).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK, "the budget came back: {body}");
    }

    /// A guard, green before and after the body budget: bodies in memory stay
    /// bounded by `max_in_flight` × 25 MiB, as the slot bounded them before.
    #[tokio::test]
    async fn bodies_in_memory_stay_within_the_budget() {
        use crate::api::handlers::create_router;
        let app = create_router(build_test_state_with_backpressure(1).await);
        let (taken_in_tx, taken_in_rx) = tokio::sync::oneshot::channel();
        let held = tokio::spawn(app.clone().oneshot(image_request(mib_then_stall(24, taken_in_tx))));
        taken_in_rx.await.unwrap();
        let padded = serde_json::json!({ "model": "mock/image-gen", "prompt": "x", "pad": "x".repeat(2 * MIB) });
        let (status, body) = status_and_code(app.clone().oneshot(image_request(Body::from(padded.to_string()))).await.unwrap()).await;
        held.abort();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(body["error"]["code"], "server_at_capacity", "{body}");
    }

    /// N2: a body that breaks off is not "too large".
    #[tokio::test]
    async fn a_body_that_breaks_off_is_answered_400_body_unreadable() {
        use crate::api::handlers::create_router;
        let app = create_router(build_test_state_with_backpressure(4).await);
        let chunks = vec![
            Ok(Bytes::from_static(b"{\"model\":")),
            Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "the client went away")),
        ];
        let (status, body) = status_and_code(app.oneshot(image_request(Body::from_stream(futures::stream::iter(chunks)))).await.unwrap()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"]["code"], "body_unreadable", "{body}");
    }
}
