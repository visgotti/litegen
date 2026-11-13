//! The provider circuit breaker that all replicas share: the Redis side of
//! `proxy::circuit_breaker::BreakerStore` (`docs/scaling.md`).
//!
//! Each provider's streak is a hash under `key(["breaker", provider])` with
//! the fields `failures` (the consecutive failures) and `opened_ms` (when the
//! breaker last opened, in Redis's own clock). It expires 24 hours after its
//! last failure, and a success deletes it, unless a window is running: a
//! success then is from a request sent before the breaker opened, and changes
//! nothing, on any replica.
//!
//! Two rules keep the replicas honest:
//!
//! - **One step.** Counting a failure and deciding whether it opens the breaker
//!   happen in one `EVAL`. Two replicas failing at the same instant cannot
//!   both open a window, and cannot both miss the threshold.
//! - **One clock.** The scripts take the time from `redis.call('TIME')`,
//!   never from a replica. Replicas' clocks differ, and a replica a second
//!   ahead would close the breaker a second early for all of them. For that
//!   reason `is_open` is a script too: it compares `opened_ms` with Redis's
//!   own time. So is `open_remaining`, the time a window has left, which a
//!   route whose every breaker is open answers as `Retry-After`.
//!
//! The scripts read the clock and then write. Redis 5 and later (and Valkey)
//! allow that, because they replicate a script's effects, not its text. They
//! run through [`redis::Script`], like the token bucket in
//! [`rate_limit`](super::rate_limit).
//!
//! **While Redis cannot be reached** the breaker is this replica's own.
//! [`RedisBreakerStore`] keeps a [`LocalBreakerStore`] and records every
//! outcome in it too, whether Redis answers or not. So the local streak is
//! always this replica's real streak: a streak from an old outage cannot
//! linger, because the success that ended it was recorded here too, and the
//! first failure of a new outage does not open the breaker on the strength of
//! the old one. That holds for the local mirror only: a success whose `DEL` was
//! lost while Redis was down leaves the Redis hash with the old failures, so
//! the first failure after Redis returns re-opens the breaker for one window;
//! the first success after that window clears it.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use super::{SharedError, SharedRedis, Throttle};
use crate::proxy::circuit_breaker::{BreakerStore, LocalBreakerStore};

/// Counts one failure for the provider in `KEYS[1]`, and opens the breaker if
/// that makes the streak long enough and no window is running.
///
/// - `ARGV[1]`: the threshold.
/// - `ARGV[2]`: how long a window lasts, in milliseconds.
///
/// The breaker opens when `failures >= threshold` and `opened_ms` is missing or
/// at least a window old: a failed half-open probe opens it again, and a
/// failure during a window leaves the window's end where it was.
pub const SCRIPT_SOURCE: &str = r#"
local threshold = tonumber(ARGV[1])
local open_for = tonumber(ARGV[2])
local t = redis.call('TIME')
local now = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
local failures = redis.call('HINCRBY', KEYS[1], 'failures', 1)
local opened = tonumber(redis.call('HGET', KEYS[1], 'opened_ms'))
if failures >= threshold and (opened == nil or now - opened >= open_for) then
  redis.call('HSET', KEYS[1], 'opened_ms', string.format('%.0f', now))
end
redis.call('PEXPIRE', KEYS[1], 86400000)
"#;

/// Whether the breaker for the provider in `KEYS[1]` is open: 1 or 0. It is
/// while `now` is before `opened_ms` plus the window in `ARGV[1]`
/// (milliseconds). Nothing is written.
pub const IS_OPEN_SCRIPT_SOURCE: &str = r#"
local opened = tonumber(redis.call('HGET', KEYS[1], 'opened_ms'))
if opened == nil then
  return 0
end
local t = redis.call('TIME')
local now = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
if now < opened + tonumber(ARGV[1]) then
  return 1
end
return 0
"#;

/// How long the window of the provider in `KEYS[1]` has left, in
/// milliseconds: 0 when none runs. `ARGV[1]` is how long a window lasts, in
/// milliseconds. Nothing is written. What a route whose every breaker is open
/// puts in `Retry-After`.
pub const OPEN_REMAINING_SCRIPT_SOURCE: &str = r#"
local opened = tonumber(redis.call('HGET', KEYS[1], 'opened_ms'))
if opened == nil then
  return 0
end
local t = redis.call('TIME')
local now = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
local left = opened + tonumber(ARGV[1]) - now
if left > 0 then
  return left
end
return 0
"#;

/// Clears the streak of the provider in `KEYS[1]` after a success, unless a
/// window is running: then it changes nothing, since the success is from a
/// request sent before the breaker opened. `ARGV[1]` is how long a window
/// lasts, in milliseconds. Returns 1 if it cleared, 0 if a window runs.
pub const RECORD_SUCCESS_SCRIPT_SOURCE: &str = r#"
local opened = tonumber(redis.call('HGET', KEYS[1], 'opened_ms'))
if opened ~= nil then
  local t = redis.call('TIME')
  local now = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
  if now < opened + tonumber(ARGV[1]) then
    return 0
  end
end
redis.call('DEL', KEYS[1])
return 1
"#;

/// Built once, so their SHA-1s are computed once.
static RECORD_FAILURE: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(SCRIPT_SOURCE));
static IS_OPEN: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(IS_OPEN_SCRIPT_SOURCE));
static OPEN_REMAINING: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(OPEN_REMAINING_SCRIPT_SOURCE));
static RECORD_SUCCESS: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(RECORD_SUCCESS_SCRIPT_SOURCE));

/// The breaker state the replicas share through Redis. See the module docs.
pub struct RedisBreakerStore {
    shared: Arc<SharedRedis>,
    /// This replica's own outcomes: the answer while Redis is out.
    local: LocalBreakerStore,
    fallback_warning: Throttle,
}

impl RedisBreakerStore {
    pub fn new(shared: Arc<SharedRedis>) -> RedisBreakerStore {
        RedisBreakerStore {
            shared,
            local: LocalBreakerStore::default(),
            fallback_warning: Throttle::new(),
        }
    }

    fn key(&self, provider: &str) -> String {
        self.shared.key(&["breaker", provider])
    }

    /// Say, at most once a minute, that the shared breaker did not answer.
    fn note_fallback(&self, error: &SharedError) {
        if self.fallback_warning.due(Instant::now()) {
            tracing::warn!(
                %error,
                "Redis circuit-breaker sharing failed; this replica uses its own breaker until it works again"
            );
        }
    }
}

#[async_trait]
impl BreakerStore for RedisBreakerStore {
    async fn is_open(&self, provider: &str, open_for: Duration) -> bool {
        let key = self.key(provider);
        let asked: Result<i64, SharedError> = self
            .shared
            .run(|mut connection| async move {
                let mut invocation = IS_OPEN.prepare_invoke();
                invocation.key(key.as_str()).arg(open_for.as_millis() as u64);
                invocation.invoke_async(&mut connection).await
            })
            .await;
        match asked {
            Ok(open) => open == 1,
            Err(error) => {
                self.note_fallback(&error);
                self.local.is_open(provider, open_for).await
            }
        }
    }

    async fn open_remaining(&self, provider: &str, open_for: Duration) -> Option<Duration> {
        let key = self.key(provider);
        let asked: Result<i64, SharedError> = self
            .shared
            .run(|mut connection| async move {
                let mut invocation = OPEN_REMAINING.prepare_invoke();
                invocation.key(key.as_str()).arg(open_for.as_millis() as u64);
                invocation.invoke_async(&mut connection).await
            })
            .await;
        match asked {
            Ok(left_ms) => u64::try_from(left_ms).ok().filter(|ms| *ms > 0).map(Duration::from_millis),
            Err(error) => {
                self.note_fallback(&error);
                self.local.open_remaining(provider, open_for).await
            }
        }
    }

    async fn record_success(&self, provider: &str, open_for: Duration) {
        self.local.record_success(provider, open_for).await;
        let key = self.key(provider);
        let cleared: Result<i64, SharedError> = self
            .shared
            .run(|mut connection| async move {
                let mut invocation = RECORD_SUCCESS.prepare_invoke();
                invocation.key(key.as_str()).arg(open_for.as_millis() as u64);
                invocation.invoke_async(&mut connection).await
            })
            .await;
        if let Err(error) = cleared {
            self.note_fallback(&error);
        }
    }

    async fn record_failure(&self, provider: &str, threshold: u32, open_for: Duration) {
        self.local.record_failure(provider, threshold, open_for).await;
        let key = self.key(provider);
        let counted: Result<(), SharedError> = self
            .shared
            .run(|mut connection| async move {
                let mut invocation = RECORD_FAILURE.prepare_invoke();
                invocation.key(key.as_str()).arg(threshold).arg(open_for.as_millis() as u64);
                invocation.invoke_async(&mut connection).await
            })
            .await;
        if let Err(error) = counted {
            self.note_fallback(&error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::circuit_breaker::CircuitBreaker;
    use crate::shared::testing::{addr_of, dead_redis, flush_keys, test_prefix, test_url, two_shared, CutProxy};
    use crate::config::connections::RedisSettings;
    use crate::config::{AppConfig, CacheGlobalConfig, CircuitBreakerConfig};
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::store::LocalStore;

    fn breaker(shared: Arc<SharedRedis>, threshold: u32, open_for: Duration) -> CircuitBreaker {
        CircuitBreaker::with_store(threshold, open_for, Arc::new(RedisBreakerStore::new(shared)))
    }

    /// Connects now: ping until Redis answers, with a deadline. A test that
    /// counts on every one of its calls reaching Redis connects first, because
    /// a call that connects under load can run past its 500 ms and fall back to
    /// the local breaker without saying so.
    async fn warm_up(shared: &SharedRedis) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while shared.ping().await.is_err() {
            assert!(Instant::now() < deadline, "the test Redis did not answer within 10 s");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[test]
    fn scripts_use_the_redis_clock() {
        for source in [SCRIPT_SOURCE, IS_OPEN_SCRIPT_SOURCE, OPEN_REMAINING_SCRIPT_SOURCE, RECORD_SUCCESS_SCRIPT_SOURCE] {
            assert!(source.contains("redis.call('TIME')"), "{source}");
        }
    }

    #[tokio::test]
    async fn two_breakers_share_state() {
        let Some((sa, sb)) = two_shared().await else { return };
        warm_up(&sa).await;
        warm_up(&sb).await;
        let a = breaker(sa.clone(), 2, Duration::from_millis(400));
        let b = breaker(sb, 2, Duration::from_millis(400));
        a.record_failure("v").await;
        b.record_failure("v").await;
        assert!(a.is_open("v").await && b.is_open("v").await, "one streak, two replicas");
        tokio::time::sleep(Duration::from_millis(460)).await; // past the window
        b.record_success("v").await;
        assert!(!a.is_open("v").await, "a success on one replica closes it for both");
        // Without the clear, the old streak plus one failure would open it again.
        a.record_failure("v").await;
        assert!(!a.is_open("v").await && !b.is_open("v").await, "the success restarted the streak for both");
        assert!(!b.is_open("other").await, "other providers are unaffected");
        flush_keys(&sa).await;
    }

    /// A request one replica sent before the window opened, coming back fine
    /// during it, says nothing about the vendor now: the window runs on, for
    /// every replica.
    #[tokio::test]
    async fn a_late_success_on_one_replica_leaves_the_window_open_for_both() {
        let Some((sa, sb)) = two_shared().await else { return };
        warm_up(&sa).await;
        warm_up(&sb).await;
        let a = breaker(sa.clone(), 2, Duration::from_secs(30));
        let b = breaker(sb, 2, Duration::from_secs(30));
        a.record_failure("v").await;
        a.record_failure("v").await;
        assert!(b.is_open("v").await);
        b.record_success("v").await; // a request B sent before the window
        assert!(a.is_open("v").await && b.is_open("v").await, "B's late success closed the shared window for both");
        flush_keys(&sa).await;
    }

    #[tokio::test]
    async fn a_success_on_one_replica_restarts_the_streak_for_both() {
        let Some((sa, sb)) = two_shared().await else { return };
        let a = breaker(sa.clone(), 2, Duration::from_secs(30));
        let b = breaker(sb, 2, Duration::from_secs(30));
        a.record_failure("v").await;
        b.record_success("v").await;
        a.record_failure("v").await;
        assert!(!a.is_open("v").await, "one failure since the success is under the threshold");
        b.record_failure("v").await;
        assert!(b.is_open("v").await);
        flush_keys(&sa).await;
    }

    #[tokio::test]
    async fn redis_breaker_reopens_after_a_failed_probe() {
        let Some((sa, sb)) = two_shared().await else { return };
        let a = breaker(sa.clone(), 2, Duration::from_millis(300));
        let b = breaker(sb, 2, Duration::from_millis(300));
        a.record_failure("p").await;
        a.record_failure("p").await;
        assert!(a.is_open("p").await);
        tokio::time::sleep(Duration::from_millis(360)).await;
        assert!(!a.is_open("p").await, "half-open");
        // The probe fails on the other replica: it opens the breaker for both.
        b.record_failure("p").await;
        assert!(b.is_open("p").await, "the failed probe opens it again");
        assert!(a.is_open("p").await);
        flush_keys(&sa).await;
    }

    #[tokio::test]
    async fn redis_failure_while_open_does_not_extend_the_window() {
        let Some((sa, sb)) = two_shared().await else { return };
        // Both clients connect first, so the second failure (sent 200 ms in)
        // lands inside the 600 ms window with 400 ms of margin. An extended
        // window would end at about 800 ms, so a check after 620 ms still
        // tells the two apart.
        warm_up(&sa).await;
        warm_up(&sb).await;
        let a = breaker(sa.clone(), 1, Duration::from_millis(600));
        let b = breaker(sb, 1, Duration::from_millis(600));
        a.record_failure("p").await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        b.record_failure("p").await;
        tokio::time::sleep(Duration::from_millis(420)).await;
        assert!(!a.is_open("p").await, "the first window ended on time");
        flush_keys(&sa).await;
    }

    /// The router's `Retry-After` (handlers review #5) is the shared window's
    /// time left, on Redis's clock: the same on every replica.
    #[tokio::test]
    async fn the_time_a_shared_breaker_has_left_is_the_same_on_both_replicas() {
        let Some((sa, sb)) = two_shared().await else { return };
        warm_up(&sa).await;
        warm_up(&sb).await;
        let a = breaker(sa.clone(), 1, Duration::from_secs(30));
        let b = breaker(sb, 1, Duration::from_secs(30));
        assert_eq!(b.open_remaining("v").await, None, "closed");
        a.record_failure("v").await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        let left = b.open_remaining("v").await.expect("A's failure opened it for B");
        assert!((Duration::from_secs(25)..=Duration::from_millis(29_750)).contains(&left), "{left:?} of a 30 s window opened 250 ms ago");
        flush_keys(&sa).await;
    }

    /// With Redis out, the time left is this replica's own breaker's.
    #[tokio::test]
    async fn a_dead_redis_says_this_replicas_own_time_left() {
        let cb = breaker(dead_redis(false).await, 1, Duration::from_secs(30));
        cb.record_failure("p").await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        let left = cb.open_remaining("p").await.expect("this replica's breaker opened");
        assert!((Duration::from_secs(25)..=Duration::from_millis(29_750)).contains(&left), "{left:?} of a 30 s window opened 250 ms ago");
    }

    #[tokio::test]
    async fn a_dead_redis_falls_back_to_the_local_answer() {
        // Fail-closed is for limits; the breaker falls back either way.
        for fail_closed in [false, true] {
            let cb = breaker(dead_redis(fail_closed).await, 2, Duration::from_millis(400));
            cb.record_failure("p").await;
            assert!(!cb.is_open("p").await, "one failure is under the threshold");
            cb.record_failure("p").await;
            assert!(cb.is_open("p").await, "this replica's own breaker opens");
            tokio::time::sleep(Duration::from_millis(460)).await; // past the window
            cb.record_success("p").await;
            assert!(!cb.is_open("p").await);
            // Without the clear, the old streak plus one failure would open it again.
            cb.record_failure("p").await;
            assert!(!cb.is_open("p").await, "the success restarted this replica's streak");
        }
    }

    async fn through_a_proxy(proxy: &CutProxy, prefix: &str) -> Arc<SharedRedis> {
        let mut settings = RedisSettings::from_url(&proxy.url());
        settings.breaker_recovery = Duration::ZERO; // 500 ms, the floor
        SharedRedis::connect(&settings, prefix).await.unwrap()
    }

    /// Delete the keys under `prefix` through a client that goes straight to
    /// Redis: the one a test cut off can't clean up after itself.
    async fn flush_directly(url: &str, prefix: &str) {
        let direct = SharedRedis::connect(&RedisSettings::from_url(url), prefix).await.unwrap();
        flush_keys(&direct).await;
    }

    #[tokio::test]
    async fn a_streak_seen_while_redis_was_up_carries_into_an_outage() {
        let Some(url) = test_url() else { return };
        let proxy = CutProxy::start(addr_of(&url)).await;
        let prefix = test_prefix();
        let cb = breaker(through_a_proxy(&proxy, &prefix).await, 3, Duration::from_secs(30));
        cb.record_failure("p").await;
        cb.record_failure("p").await;
        assert!(!cb.is_open("p").await);
        proxy.cut();
        cb.record_failure("p").await;
        assert!(cb.is_open("p").await, "the third failure in a row, two of them counted while Redis was up");
        flush_directly(&url, &prefix).await;
    }

    #[tokio::test]
    async fn a_success_clears_the_local_streak_too() {
        let Some(url) = test_url() else { return };
        let proxy = CutProxy::start(addr_of(&url)).await;
        let prefix = test_prefix();
        let cb = breaker(through_a_proxy(&proxy, &prefix).await, 3, Duration::from_millis(300));
        proxy.cut();
        for _ in 0..3 {
            cb.record_failure("p").await;
        }
        assert!(cb.is_open("p").await, "the first outage: this replica's own breaker");
        proxy.restore();
        tokio::time::sleep(Duration::from_millis(700)).await; // past the Redis breaker's recovery
        cb.record_success("p").await;
        assert!(!cb.is_open("p").await, "Redis answers: no breaker is open");
        proxy.cut();
        cb.record_failure("p").await;
        assert!(!cb.is_open("p").await, "the next outage starts from a streak of one, not the old one");
        flush_directly(&url, &prefix).await;
    }

    /// `ProxyRouter::with_shared` swaps in the Redis breaker, built from the
    /// configured settings.
    #[tokio::test]
    async fn routers_given_a_redis_share_one_breaker_with_the_configured_threshold() {
        let Some((sa, sb)) = two_shared().await else { return };
        let router = |shared: Arc<SharedRedis>| {
            let config = AppConfig {
                circuit_breaker: CircuitBreakerConfig { threshold: 2, open_for_seconds: 30 },
                ..AppConfig::default()
            };
            ProxyRouter::new(
                Arc::new(ProviderRegistry::new()),
                Arc::new(GenerationCache::new(&CacheGlobalConfig::default())),
                Arc::new(config),
                Arc::new(LocalStore),
            )
            .with_shared(shared)
        };
        // Connected first. A failure that never reaches Redis (a first call that
        // connects under load can run past its 500 ms) is counted on this
        // replica alone, and the two routers would no longer share a streak.
        warm_up(&sa).await;
        warm_up(&sb).await;
        let flusher = sa.clone();
        let (a, b) = (router(sa), router(sb));
        a.circuit_breaker.record_failure("v").await;
        assert!(!b.circuit_breaker.is_open("v").await, "one failure is under the configured threshold of 2");
        b.circuit_breaker.record_failure("v").await;
        assert!(a.circuit_breaker.is_open("v").await, "the second, on the other replica, opens it for both");
        flush_keys(&flusher).await;
    }
}
