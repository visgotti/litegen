use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::shared::{rate_limit as shared_bucket, SharedError, SharedRedis, Throttle};

struct Bucket {
    last: Instant,
    tokens: f64,
}

/// Snapshot of the current rate-limit bucket state for a key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimitSnapshot {
    /// Remaining tokens (floored to 0).
    pub remaining: u32,
    /// Approximate duration until the bucket is full again.
    pub reset_in: Duration,
}

/// Why a request did not get a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitRefusal {
    /// The bucket is empty. The next token is `retry_after_secs` away.
    Exceeded { retry_after_secs: u64 },
    /// Redis cannot be reached and `LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT`
    /// is on, so the limit cannot be checked. Answer with
    /// [`unverifiable_response`].
    Unverifiable,
}

/// The fixed 503 for [`LimitRefusal::Unverifiable`].
pub fn unverifiable_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [("retry-after", "1")],
        Json(serde_json::json!({ "error": {
            "message": "Rate limit enforcement unavailable: request counters could not be verified \
                        against Redis, and LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT is on, so the \
                        request was refused to avoid exceeding the configured limit. Retry shortly.",
            "type": "rate_limit_unverifiable",
            "code": "rate_limit_unverifiable"
        } })),
    )
        .into_response()
}

/// Token-bucket rate limiter, keyed by API key UUID (or a principal's id).
///
/// With Redis (`with_shared`) the buckets live there and every replica draws
/// from the same ones. Without it, or while Redis cannot be reached and
/// fail-closed is off, they are this process's own: an in-memory map behind a
/// tokio RwLock.
pub struct RateLimiter {
    buckets: RwLock<HashMap<Uuid, Bucket>>,
    shared: Option<Arc<SharedRedis>>,
    /// Lets the log say that Redis is out once a minute, not once a request.
    outage_warning: Throttle,
    /// When a take last fell back to the local buckets (`None`: none has, or
    /// they were dropped since). Written under the buckets' lock.
    last_fallback: Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: RwLock::new(HashMap::new()),
            shared: None,
            outage_warning: Throttle::new(),
            last_fallback: Mutex::new(None),
        }
    }

    /// Buckets shared through `shared`, falling back to local ones while
    /// Redis cannot be reached (or refusing, if `shared.fail_closed()`).
    pub fn with_shared(shared: Arc<SharedRedis>) -> Self {
        Self {
            shared: Some(shared),
            ..Self::new()
        }
    }

    /// The Redis the buckets live in, if any.
    pub fn shared(&self) -> Option<&Arc<SharedRedis>> {
        self.shared.as_ref()
    }

    /// Attempt to consume one token for `key_id` at `rpm` rate.
    ///
    /// Returns the bucket as the take left it if allowed (what the response's
    /// `X-RateLimit-*` headers report, with no second look at the bucket), or
    /// why not: [`LimitRefusal::Exceeded`] with the seconds until one token is
    /// available, or [`LimitRefusal::Unverifiable`] when Redis is out and
    /// fail-closed is on.
    pub async fn try_take(&self, key_id: Uuid, rpm: u32) -> Result<RateLimitSnapshot, LimitRefusal> {
        self.try_take_clocked(key_id, rpm, Instant::now).await
    }

    /// [`Self::try_take`], reading the time that decides whether the local
    /// buckets have expired from `clock`, so a test can look from a minute
    /// later without an `Instant` a minute in the past (a host that booted less
    /// than a minute ago can't make one).
    async fn try_take_clocked(&self, key_id: Uuid, rpm: u32, clock: fn() -> Instant) -> Result<RateLimitSnapshot, LimitRefusal> {
        if let Some(shared) = &self.shared {
            match shared_bucket::take(shared, &key_id.to_string(), rpm).await {
                Ok((allowed, tokens)) => {
                    self.drop_expired_local_buckets(clock()).await;
                    return if allowed {
                        // `tokens` is what the script left in the bucket.
                        Ok(snapshot_of(tokens, rpm))
                    } else {
                        Err(LimitRefusal::Exceeded { retry_after_secs: retry_after_secs(tokens, rpm) })
                    };
                }
                Err(e) if shared.fail_closed() => {
                    self.warn_outage(&e, "rate-limited requests are refused with 503 until it works again");
                    return Err(LimitRefusal::Unverifiable);
                }
                Err(e) => self.warn_outage(&e, "limits are per replica until it works again"),
            }
        }
        self.take_local(key_id, rpm).await
    }

    /// Return a point-in-time snapshot of the bucket for `key_id` at `rpm`.
    /// If no bucket exists yet (key hasn't made a request), returns a full bucket.
    ///
    /// Takes nothing. If Redis cannot be reached it reads the local buckets,
    /// whatever fail-closed says. A served request's headers don't use this:
    /// they report the snapshot its own [`Self::try_take`] returned, which
    /// costs no second round trip.
    pub async fn snapshot(&self, key_id: Uuid, rpm: u32) -> RateLimitSnapshot {
        if let Some(shared) = &self.shared {
            if let Ok(tokens) = shared_bucket::peek(shared, &key_id.to_string(), rpm).await {
                return snapshot_of(tokens, rpm);
            }
        }
        self.snapshot_local(key_id, rpm).await
    }

    async fn take_local(&self, key_id: Uuid, rpm: u32) -> Result<RateLimitSnapshot, LimitRefusal> {
        let capacity = rpm as f64;
        let refill_rate = capacity / 60.0; // tokens per second
        let now = Instant::now();

        let mut buckets = self.buckets.write().await;
        self.note_fallback(now);
        let bucket = buckets.entry(key_id).or_insert(Bucket {
            last: now,
            tokens: capacity, // start full
        });

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * refill_rate).min(capacity);
        bucket.last = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(snapshot_of(bucket.tokens, rpm))
        } else {
            Err(LimitRefusal::Exceeded { retry_after_secs: retry_after_secs(bucket.tokens, rpm) })
        }
    }

    async fn snapshot_local(&self, key_id: Uuid, rpm: u32) -> RateLimitSnapshot {
        let capacity = rpm as f64;
        let refill_rate = capacity / 60.0; // tokens per second
        let now = Instant::now();

        let buckets = self.buckets.read().await;
        let tokens = if let Some(bucket) = buckets.get(&key_id) {
            let elapsed = now.duration_since(bucket.last).as_secs_f64();
            (bucket.tokens + elapsed * refill_rate).min(capacity)
        } else {
            capacity
        };
        snapshot_of(tokens, rpm)
    }

    /// Redis answered, so the shared buckets are in charge: forget what the
    /// local ones counted while it was out, but only once they have gone
    /// [`LOCAL_BUCKETS_KEPT_FOR`] unused. A Redis that flaps would otherwise
    /// hand every key a full local bucket at each failure. The two kinds of
    /// bucket are never merged.
    async fn drop_expired_local_buckets(&self, now: Instant) {
        // Most calls have never fallen back: no need for the buckets' lock.
        if !local_buckets_expired(*self.lock_last_fallback(), now) {
            return;
        }
        let mut buckets = self.buckets.write().await;
        // Looked at again under the lock: a take that fell back meanwhile has
        // set a newer time.
        let mut last = self.lock_last_fallback();
        if local_buckets_expired(*last, now) {
            buckets.clear();
            *last = None;
        }
    }

    /// Record that a take fell back to the local buckets at `now`. The time
    /// only moves forward: `now` was read before the buckets' lock was taken,
    /// so a take that read it earlier can get the lock later, and must not
    /// move the time back.
    fn note_fallback(&self, now: Instant) {
        let mut last = self.lock_last_fallback();
        *last = Some(last.map_or(now, |then| then.max(now)));
    }

    fn lock_last_fallback(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.last_fallback.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Say that Redis rate limiting failed, at most once a minute. The cause
    /// may be an unreachable Redis or an error reply from one that answers.
    /// `consequence` is what the operator should expect meanwhile. `error` is
    /// Redis's or the network's own text, never the URL.
    fn warn_outage(&self, error: &SharedError, consequence: &str) {
        if self.outage_warning.due(Instant::now()) {
            tracing::warn!(%error, "Redis rate limiting failed; {consequence}");
        }
    }
}

/// How long the local buckets are kept after the last request they served.
const LOCAL_BUCKETS_KEPT_FOR: Duration = Duration::from_secs(60);

/// Whether the local buckets may go at `now`, given when a take last fell back
/// to them. A bucket refills completely in 60 s, so after that long without a
/// request every local bucket is full again and dropping them changes no
/// limit. Sooner, it would.
fn local_buckets_expired(last_fallback: Option<Instant>, now: Instant) -> bool {
    last_fallback.is_some_and(|then| now.saturating_duration_since(then) >= LOCAL_BUCKETS_KEPT_FOR)
}

/// Whole seconds until a bucket holding `tokens` has one at `rpm`. A bucket
/// at 0 rpm never refills, so it answers a minute: the longest wait any
/// other limit gives, and LiteLLM's wait for the next minute.
pub(crate) fn retry_after_secs(tokens: f64, rpm: u32) -> u64 {
    if rpm == 0 {
        return 60;
    }
    let refill_rate = rpm as f64 / 60.0; // tokens per second
    let secs_to_refill = (1.0 - tokens) / refill_rate;
    secs_to_refill.ceil() as u64
}

/// What a bucket holding `tokens` at `rpm` reports.
fn snapshot_of(tokens: f64, rpm: u32) -> RateLimitSnapshot {
    let capacity = rpm as f64;
    let refill_rate = capacity / 60.0; // tokens per second
    let remaining = tokens.floor() as u32;
    let deficit = (capacity - tokens).max(0.0);
    let reset_in_secs = if deficit <= 0.0 || refill_rate <= 0.0 {
        0.0
    } else {
        deficit / refill_rate
    };

    RateLimitSnapshot {
        remaining,
        reset_in: Duration::from_secs_f64(reset_in_secs),
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_bucket_allows_requests() {
        let limiter = RateLimiter::new();
        let id = Uuid::new_v4();
        // rpm=60 → 1 token/second capacity; fresh bucket starts full
        let result = limiter.try_take(id, 60).await;
        assert!(result.is_ok(), "fresh bucket should allow the first request");
    }

    #[tokio::test]
    async fn bucket_exhaustion_returns_retry_after() {
        let limiter = RateLimiter::new();
        let id = Uuid::new_v4();
        let rpm: u32 = 3;

        // First rpm requests should succeed (bucket capacity = rpm)
        for i in 0..rpm {
            assert!(
                limiter.try_take(id, rpm).await.is_ok(),
                "request {} should succeed",
                i + 1
            );
        }

        // rpm+1 th should fail with retry_after > 0
        let err = limiter.try_take(id, rpm).await;
        assert!(err.is_err(), "expected rate limit error on request {}", rpm + 1);
        let Err(LimitRefusal::Exceeded { retry_after_secs: secs }) = err else {
            panic!("expected Exceeded, got {err:?}");
        };
        assert!(secs > 0, "retry_after should be > 0 seconds, got {}", secs);
    }

    #[tokio::test]
    async fn different_keys_have_independent_buckets() {
        let limiter = RateLimiter::new();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        // Exhaust id_a (rpm=1)
        let _ = limiter.try_take(id_a, 1).await;
        assert!(
            limiter.try_take(id_a, 1).await.is_err(),
            "id_a should be exhausted"
        );

        // id_b should still work
        assert!(
            limiter.try_take(id_b, 1).await.is_ok(),
            "id_b should have its own independent bucket"
        );
    }

    /// What the response's headers say comes from the take itself: the tokens
    /// it left, and when the bucket is full again. A later look would be a
    /// second round trip, and would be wrong if Redis went away in between.
    #[tokio::test]
    async fn a_take_reports_the_bucket_it_left() {
        let limiter = RateLimiter::new();
        let id = Uuid::new_v4();
        let rpm: u32 = 3;

        let mut left = Vec::new();
        for _ in 0..rpm {
            left.push(limiter.try_take(id, rpm).await.unwrap().remaining);
        }
        assert_eq!(left, [2, 1, 0], "what each take left");

        // 3 a minute is a token every 20 s: one taken from a full bucket is
        // 20 s from full again.
        let fresh = limiter.try_take(Uuid::new_v4(), rpm).await.unwrap();
        assert_eq!(fresh.remaining, 2);
        assert!(
            (Duration::from_secs(19)..=Duration::from_secs(20)).contains(&fresh.reset_in),
            "one of three taken, so one token (20 s) to refill; got {:?}",
            fresh.reset_in
        );
    }

    // ─── Shared (Redis) buckets ───────────────────────────────────────────────
    //
    // These need `LITEGEN_REDIS_TEST_URL` and skip themselves without it; the
    // ones on `dead_redis` need no container.

    /// The shared script's own count is the snapshot, whichever replica takes.
    #[tokio::test]
    async fn a_shared_take_reports_the_bucket_it_left() {
        let Some((a, b)) = two_shared_limiters().await else { return };
        let k = Uuid::new_v4();
        assert_eq!(a.try_take(k, 3).await.unwrap().remaining, 2);
        let second = b.try_take(k, 3).await.unwrap();
        assert_eq!(second.remaining, 1, "b sees what a's take left");
        assert!(
            (Duration::from_secs(39)..=Duration::from_secs(40)).contains(&second.reset_in),
            "two of three taken, so two tokens (40 s) to refill; got {:?}",
            second.reset_in
        );
        assert_eq!(a.try_take(k, 3).await.unwrap().remaining, 0);
    }

    use crate::config::connections::RedisSettings;
    use crate::shared::testing::{addr_of, dead_redis, flush_scripts, test_prefix, test_url, two_shared_limiters, CutProxy};

    /// A limiter on the Redis behind `proxy`, with a breaker that opens at the
    /// first failure and probes again after the 500 ms floor.
    async fn limiter_behind(proxy: &CutProxy) -> RateLimiter {
        let mut settings = RedisSettings::from_url(&proxy.url());
        settings.breaker_failure_threshold = 1;
        settings.breaker_recovery = Duration::from_millis(500);
        RateLimiter::with_shared(SharedRedis::connect(&settings, &test_prefix()).await.unwrap())
    }

    /// Wait until the limiter's Redis answers again. The breaker lets one probe
    /// through every 500 ms and a probe can lose to a load spike, so this
    /// tries a few times instead of once.
    async fn wait_for_redis(l: &RateLimiter) {
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(600)).await;
            if crate::shared::rate_limit::peek(l.shared().unwrap(), "probe", 1).await.is_ok() {
                return;
            }
        }
        panic!("Redis did not answer again");
    }

    /// Wait until the limiter's Redis has gone: a cut takes a moment to drop
    /// the connections already open.
    async fn wait_for_outage(l: &RateLimiter) {
        for _ in 0..40 {
            if crate::shared::rate_limit::peek(l.shared().unwrap(), "probe", 1).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("Redis did not go away");
    }

    #[tokio::test]
    async fn two_limiters_share_one_bucket() {
        let Some((a, b)) = two_shared_limiters().await else { return };
        let k = Uuid::new_v4();
        assert!(a.try_take(k, 3).await.is_ok());
        assert!(b.try_take(k, 3).await.is_ok());
        assert!(a.try_take(k, 3).await.is_ok());
        assert!(matches!(b.try_take(k, 3).await, Err(LimitRefusal::Exceeded { .. })));
        assert_eq!(a.snapshot(k, 3).await.remaining, 0);
    }

    #[tokio::test]
    async fn a_burst_across_two_limiters_allows_exactly_the_limit() {
        let Some((a, b)) = two_shared_limiters().await else { return };
        let k = Uuid::new_v4();
        let tries = (0..50).map(|i| {
            let l = if i % 2 == 0 { a.clone() } else { b.clone() };
            async move { l.try_take(k, 10).await.is_ok() }
        });
        assert_eq!(futures::future::join_all(tries).await.into_iter().filter(|ok| *ok).count(), 10);
    }

    #[tokio::test]
    async fn script_flush_is_survived() {
        use crate::shared::rate_limit::take;
        let Some((a, _)) = two_shared_limiters().await else { return };
        let shared = a.shared().unwrap();
        let bucket = Uuid::new_v4().to_string();
        assert!(matches!(take(shared, &bucket, 5).await, Ok((true, _))), "the script is loaded");
        flush_scripts(shared).await;
        // The script call itself, not the limiter: a limiter that could not
        // run the script would fall back to its local buckets and still say yes.
        assert!(matches!(take(shared, &bucket, 5).await, Ok((true, _))), "the script is loaded again");
        assert!(a.try_take(Uuid::new_v4(), 5).await.is_ok());
    }

    #[tokio::test]
    async fn redis_down_falls_back_to_local() {
        let l = RateLimiter::with_shared(dead_redis(false).await);
        let k = Uuid::new_v4();
        assert!(l.try_take(k, 2).await.is_ok());
        assert!(l.try_take(k, 2).await.is_ok());
        assert!(matches!(l.try_take(k, 2).await, Err(LimitRefusal::Exceeded { .. })));
    }

    #[tokio::test]
    async fn fail_closed_refuses_when_redis_is_down() {
        let l = RateLimiter::with_shared(dead_redis(true).await);
        assert!(matches!(l.try_take(Uuid::new_v4(), 2).await, Err(LimitRefusal::Unverifiable)));
    }

    #[tokio::test]
    async fn fail_closed_takes_nothing_locally() {
        let l = RateLimiter::with_shared(dead_redis(true).await);
        let k = Uuid::new_v4();
        for _ in 0..3 {
            assert_eq!(l.try_take(k, 1).await, Err(LimitRefusal::Unverifiable));
        }
        assert!(l.buckets.read().await.is_empty(), "no local bucket was touched");
    }

    #[tokio::test]
    async fn a_snapshot_with_redis_down_reads_the_local_buckets() {
        for fail_closed in [false, true] {
            let l = RateLimiter::with_shared(dead_redis(fail_closed).await);
            // No local bucket yet: a full one, never an error or a panic.
            assert_eq!(l.snapshot(Uuid::new_v4(), 7).await.remaining, 7);
        }
    }

    #[test]
    fn scripts_use_the_redis_clock() {
        let source = crate::shared::rate_limit::SCRIPT_SOURCE;
        assert!(source.contains("redis.call('TIME')"));
        assert!(!source.contains("ARGV[3]"), "the time is never an argument: replicas' clocks differ");
    }

    #[tokio::test]
    async fn the_shared_bucket_refills_on_the_redis_clock() {
        use crate::shared::rate_limit::peek;
        let Some((a, b)) = two_shared_limiters().await else { return };
        let shared = a.shared().unwrap();
        let k = Uuid::new_v4();
        let name = k.to_string();
        // 120 a minute is 2 a second. Take ten, one after the other (each call
        // has its own time limit): well below the capacity, so the refill
        // below is not cut off at it.
        for _ in 0..10 {
            a.try_take(k, 120).await.unwrap();
        }
        let before = peek(shared, &name, 120).await.unwrap();
        assert!(before < 119.0, "ten were taken, {before} are left");

        // What a sleep adds is what the clock says: 2 tokens a second, fraction included.
        let started = Instant::now();
        tokio::time::sleep(Duration::from_millis(600)).await;
        let after = peek(shared, &name, 120).await.unwrap();
        let expected = before + 2.0 * started.elapsed().as_secs_f64();
        assert!((after - expected).abs() < 0.5, "refilled to {after}, expected about {expected}");
        assert!(b.try_take(k, 120).await.is_ok());
    }

    #[tokio::test]
    async fn a_refusal_says_when_the_next_token_comes() {
        let Some((a, b)) = two_shared_limiters().await else { return };
        let k = Uuid::new_v4();
        // 3 a minute is one token every 20 seconds.
        for _ in 0..3 {
            assert!(a.try_take(k, 3).await.is_ok());
        }
        let Err(LimitRefusal::Exceeded { retry_after_secs }) = b.try_take(k, 3).await else {
            panic!("the bucket should be empty")
        };
        assert!((19..=20).contains(&retry_after_secs), "got {retry_after_secs}");
    }

    #[tokio::test]
    async fn a_lower_rpm_clamps_the_stored_tokens() {
        let Some((a, b)) = two_shared_limiters().await else { return };
        let k = Uuid::new_v4();
        assert!(a.try_take(k, 100).await.is_ok()); // 99 left
        assert!(b.try_take(k, 2).await.is_ok()); // clamped to 2, then 1 left
        assert!(a.try_take(k, 2).await.is_ok());
        assert!(matches!(b.try_take(k, 2).await, Err(LimitRefusal::Exceeded { .. })));
    }

    #[tokio::test]
    async fn a_snapshot_takes_nothing_and_creates_nothing() {
        let Some((a, b)) = two_shared_limiters().await else { return };
        let k = Uuid::new_v4();
        let fresh = a.snapshot(k, 3).await;
        assert_eq!((fresh.remaining, fresh.reset_in), (3, Duration::ZERO));
        assert!(a.try_take(k, 3).await.is_ok());
        for _ in 0..4 {
            assert_eq!(b.snapshot(k, 3).await.remaining, 2);
        }
        assert!(b.snapshot(k, 3).await.reset_in > Duration::ZERO);
        assert!(a.try_take(k, 3).await.is_ok());
        assert!(b.try_take(k, 3).await.is_ok());
        assert!(a.try_take(k, 3).await.is_err(), "the looks did not add tokens");
    }

    #[tokio::test]
    async fn the_bucket_is_a_hash_under_the_prefix_that_expires() {
        let Some((a, _)) = two_shared_limiters().await else { return };
        let shared = a.shared().unwrap();
        let k = Uuid::new_v4();
        assert!(a.try_take(k, 5).await.is_ok());
        let key = shared.key(&["rl", &k.to_string()]);
        let (fields, ttl): (std::collections::HashMap<String, String>, i64) = shared
            .run(|mut c| async move {
                let fields = redis::cmd("HGETALL").arg(&key).query_async(&mut c).await?;
                let ttl = redis::cmd("PTTL").arg(&key).query_async(&mut c).await?;
                Ok((fields, ttl))
            })
            .await
            .unwrap();
        let mut names: Vec<_> = fields.keys().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["tokens", "ts_ms"]);
        assert!((100_000..=120_000).contains(&ttl), "PEXPIRE 120000, got {ttl}");
    }

    #[tokio::test]
    async fn the_shared_bucket_takes_over_again_and_the_local_one_goes_a_minute_later() {
        let Some(url) = test_url() else { return };
        let proxy = CutProxy::start(addr_of(&url)).await;
        proxy.cut();
        let l = limiter_behind(&proxy).await;
        let k = Uuid::new_v4();

        // Redis is out: this replica counts on its own.
        assert!(l.try_take(k, 2).await.is_ok());
        assert!(l.try_take(k, 2).await.is_ok());
        assert!(l.try_take(k, 2).await.is_err());

        // Redis returns: the shared bucket (full: nobody used it) takes over.
        // The local bucket stays, in case Redis goes again.
        proxy.restore();
        wait_for_redis(&l).await;
        assert!(l.try_take(k, 2).await.is_ok());
        assert_eq!(l.buckets.read().await.len(), 1);
        assert!(l.try_take(k, 2).await.is_ok());
        assert!(matches!(l.try_take(k, 2).await, Err(LimitRefusal::Exceeded { .. })), "the shared bucket decides now");

        // A minute after the last request the local buckets served, the next
        // answer from Redis drops them, not merged into the shared ones.
        // Looked at from 61 s later: the clock is moved forward, since an
        // `Instant` 61 s back doesn't exist on a host that booted a minute ago.
        fn a_minute_on() -> Instant {
            Instant::now() + Duration::from_secs(61)
        }
        assert!(l.try_take_clocked(k, 2, a_minute_on).await.is_err());
        assert!(l.buckets.read().await.is_empty());
        assert!(l.last_fallback.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_flapping_redis_does_not_reset_the_local_bucket() {
        let Some(url) = test_url() else { return };
        let proxy = CutProxy::start(addr_of(&url)).await;
        proxy.cut();
        let l = limiter_behind(&proxy).await;
        let k = Uuid::new_v4();

        // Redis is out, and this replica spends the key's two requests.
        assert!(l.try_take(k, 2).await.is_ok());
        assert!(l.try_take(k, 2).await.is_ok());
        assert!(l.try_take(k, 2).await.is_err());

        // Redis answers once ...
        proxy.restore();
        wait_for_redis(&l).await;
        assert!(l.try_take(k, 2).await.is_ok(), "the shared bucket is full: nobody used it");

        // ... and is gone again. The local bucket is as empty as it was: a
        // flap must not hand the key a fresh bucket each time.
        proxy.cut();
        wait_for_outage(&l).await;
        assert!(
            matches!(l.try_take(k, 2).await, Err(LimitRefusal::Exceeded { .. })),
            "the local bucket was reset by the one answer from Redis"
        );
    }

    #[test]
    fn the_last_fallback_never_moves_back() {
        let l = RateLimiter::new();
        let t0 = Instant::now();
        l.note_fallback(t0 + Duration::from_secs(5));
        l.note_fallback(t0); // a take that read its time earlier, and got the lock later
        assert_eq!(*l.last_fallback.lock().unwrap(), Some(t0 + Duration::from_secs(5)));
        l.note_fallback(t0 + Duration::from_secs(9));
        assert_eq!(*l.last_fallback.lock().unwrap(), Some(t0 + Duration::from_secs(9)));
    }

    #[test]
    fn the_local_buckets_go_a_minute_after_the_last_fallback() {
        let t0 = Instant::now();
        assert!(!local_buckets_expired(None, t0 + Duration::from_secs(600)), "none were used");
        assert!(!local_buckets_expired(Some(t0), t0));
        assert!(!local_buckets_expired(Some(t0), t0 + Duration::from_secs(59)));
        assert!(local_buckets_expired(Some(t0), t0 + Duration::from_secs(60)), "refilled to full by now");
        assert!(local_buckets_expired(Some(t0), t0 + Duration::from_secs(3600)));
    }

    #[tokio::test]
    async fn the_unverifiable_answer_is_the_fixed_503() {
        let resp = unverifiable_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers().get("retry-after").unwrap(), "1");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "error": {
                "message": "Rate limit enforcement unavailable: request counters could not be verified \
                            against Redis, and LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT is on, so the \
                            request was refused to avoid exceeding the configured limit. Retry shortly.",
                "type": "rate_limit_unverifiable",
                "code": "rate_limit_unverifiable"
            } })
        );
    }

    // ─── A limit of 0 ─────────────────────────────────────────────────────────

    /// A bucket at 0 rpm never refills, so its refusal says to wait a minute,
    /// the longest any other limit asks for, rather than `u64::MAX` seconds.
    #[tokio::test]
    async fn a_zero_rpm_bucket_refuses_with_a_one_minute_retry_after() {
        assert_eq!(retry_after_secs(0.0, 0), 60);
        let limiter = RateLimiter::new();
        assert_eq!(limiter.try_take(Uuid::new_v4(), 0).await, Err(LimitRefusal::Exceeded { retry_after_secs: 60 }));
    }

    #[tokio::test]
    async fn a_zero_rpm_shared_bucket_refuses_with_a_one_minute_retry_after() {
        let Some((a, _)) = two_shared_limiters().await else { return };
        assert_eq!(a.try_take(Uuid::new_v4(), 0).await, Err(LimitRefusal::Exceeded { retry_after_secs: 60 }));
    }
}
