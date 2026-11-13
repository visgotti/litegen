//! Parallel-request slots held for the length of a request.
//!
//! [`acquire_slot`] gives a principal one of its `max` slots, or refuses. The
//! slot is a [`SlotGuard`], freed on drop. It is one of two kinds:
//!
//! - **Shared** ([`SharedSlot`]) when the limiter has Redis: a lease in the
//!   set all replicas count against (`shared::slots`), renewed while the
//!   request runs.
//! - **Local** ([`ParallelSlot`], in `enforce`) without Redis, or while Redis
//!   cannot be reached and fail-closed is off. The cap is then per replica.
//!
//! `enforce::begin` takes one per principal with a cap. `enforce` re-exports
//! this module's public items, so the paths `enforce::SlotGuard` and
//! `enforce::acquire_slot` work.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::enforce::ParallelSlot;
use super::types::EntityType;
use crate::api::middleware::rate_limit::{LimitRefusal, RateLimiter};
use crate::shared::slots::{self as redis_slots, SLOT_LEASE, SLOT_REFRESH_EVERY};
use crate::shared::{SharedError, SharedRedis, Throttle};

/// A parallel-request slot held for one request: this process's own
/// ([`ParallelSlot`]) or one all replicas count ([`SharedSlot`]). Freed on drop.
#[derive(Debug)]
pub enum SlotGuard {
    Local(ParallelSlot),
    Shared(SharedSlot),
}

/// A slot leased in the set all replicas share (`shared::slots`).
///
/// While it lives, a task renews the lease every [`SLOT_REFRESH_EVERY`], so a
/// request that runs longer than the lease keeps its slot. Dropping it stops
/// the task and gives the slot back. A replica that dies does neither, and its
/// slots free themselves when the lease runs out.
#[derive(Debug)]
pub struct SharedSlot {
    shared: Arc<SharedRedis>,
    set_key: String,
    member: String,
    refresher: tokio::task::JoinHandle<()>,
}

impl SharedSlot {
    /// Hold `member` of `set_key`, which `redis_slots::acquire` just added, and
    /// keep its `lease` renewed every `refresh_every`. Needs a tokio runtime.
    ///
    /// The refresher ends when the member is gone from the set (released,
    /// swept, or lost with Redis's data): there is nothing left to renew, and
    /// the cap it counted against belongs to someone else. The request goes on
    /// uncounted until it ends.
    fn start(shared: Arc<SharedRedis>, set_key: String, member: String, lease: Duration, refresh_every: Duration) -> Self {
        let refresher = tokio::spawn({
            let (shared, set_key, member) = (shared.clone(), set_key.clone(), member.clone());
            async move {
                // The first renewal is one period away: the lease is fresh.
                let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + refresh_every, refresh_every);
                // After a stall, renew once and carry on, not once per missed tick.
                ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticks.tick().await;
                    match redis_slots::refresh_member(&shared, &set_key, &member, lease).await {
                        Ok(true) => {}
                        Ok(false) => {
                            tracing::debug!(%member, "a parallel slot's lease is gone from Redis; it is no longer renewed");
                            break;
                        }
                        Err(error) => {
                            warn_upkeep(&error, "a slot's lease may run out under its request, and the cap then undercounts")
                        }
                    }
                }
            }
        });
        Self { shared, set_key, member, refresher }
    }
}

impl Drop for SharedSlot {
    fn drop(&mut self) {
        self.refresher.abort();
        // Without a runtime (a sync caller, a thread outside tokio) there is
        // nothing to run the release on, and `tokio::spawn` would panic. The
        // lease runs out by itself within `SLOT_LEASE`.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let (shared, set_key, member) = (self.shared.clone(), self.set_key.clone(), self.member.clone());
        runtime.spawn(async move {
            // A refresh that was already on its way cannot undo this: it is
            // `ZADD XX`, and the member is gone.
            if let Err(error) = redis_slots::release(&shared, &set_key, &member).await {
                warn_upkeep(&error, "the slot frees itself when its lease runs out");
            }
        });
    }
}

/// Take a parallel-request slot for a principal that may have `max` requests
/// in flight.
///
/// With Redis (`RateLimiter::with_shared`) the slot is leased there and every
/// replica counts against the same cap. Without it, or while Redis cannot be
/// reached and fail-closed is off, it is this process's own ([`ParallelSlot`]),
/// and the cap is per replica until Redis works again. Fail-closed refuses
/// with [`LimitRefusal::Unverifiable`] instead, and takes no local slot.
///
/// A full cap is [`LimitRefusal::Exceeded`] with `retry_after_secs` 0: a slot
/// frees when a request ends, not on a clock, and the 429 it becomes carries no
/// `Retry-After`.
pub async fn acquire_slot(
    limiter: &RateLimiter,
    entity_type: EntityType,
    entity_id: &str,
    max: u32,
) -> Result<SlotGuard, LimitRefusal> {
    acquire_slot_with(limiter, entity_type, entity_id, max, SLOT_LEASE, SLOT_REFRESH_EVERY).await
}

/// [`acquire_slot`] with the lease and its renewal period given, so the tests
/// can use short leases.
async fn acquire_slot_with(
    limiter: &RateLimiter,
    entity_type: EntityType,
    entity_id: &str,
    max: u32,
    lease: Duration,
    refresh_every: Duration,
) -> Result<SlotGuard, LimitRefusal> {
    let full = LimitRefusal::Exceeded { retry_after_secs: 0 };
    if let Some(shared) = limiter.shared() {
        let set_key = shared.key(&["slots", entity_type.as_str(), entity_id]);
        let member = uuid::Uuid::new_v4().to_string();
        // Armed before the call, so a caller dropped mid-acquire (a client
        // that hung up) gives the member back too.
        let mut in_doubt = InDoubt { shared: shared.clone(), set_key: set_key.clone(), member: member.clone(), armed: true };
        let acquired = redis_slots::acquire(shared, &set_key, &member, max, lease).await;
        // A known outcome: added (the slot below holds it), not added, or kept
        // off the network by the open breaker. Any other error leaves it in
        // doubt, and the guard's drop gives it back.
        if matches!(acquired, Ok(_) | Err(SharedError::Unavailable)) {
            in_doubt.armed = false;
        }
        match acquired {
            Ok(true) => return Ok(SlotGuard::Shared(SharedSlot::start(shared.clone(), set_key, member, lease, refresh_every))),
            Ok(false) => return Err(full),
            Err(error) if shared.fail_closed() => {
                warn_fallback(&error, "parallel-capped requests are refused with 503 until it works again");
                return Err(LimitRefusal::Unverifiable);
            }
            Err(error) => warn_fallback(&error, "caps are per replica until it works again"),
        }
    }
    ParallelSlot::try_acquire(entity_type, entity_id, max).map(SlotGuard::Local).ok_or(full)
}

/// A member that [`redis_slots::acquire`] may have added without this
/// process knowing: given back on drop unless disarmed.
///
/// An acquire whose answer is lost (a timeout after Redis ran the script, a
/// connection dropped after the command was written, a caller that hung up
/// mid-call) may still have taken its slot. Nothing would hold, renew or
/// release that member, so it would count against the cap on every replica
/// until its lease ran out (30 s). The `ZREM` is harmless when the script
/// never ran: the member is a fresh UUID.
///
/// - During an outage the `ZREM` fails too, as a second call the breaker
///   counts: it opens about twice as fast, which is harmless.
/// - If the lost command reaches Redis only after its own `ZREM` (the `ZREM`
///   goes out on a new connection once the old one was dropped), the member
///   lives out its lease, as every lost member did before. Redis serves the
///   command it received first, so in practice the `ZREM` comes second.
struct InDoubt {
    shared: Arc<SharedRedis>,
    set_key: String,
    member: String,
    armed: bool,
}

impl Drop for InDoubt {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // As `SharedSlot::drop`: without a runtime, the lease runs out by itself.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let (shared, set_key, member) = (self.shared.clone(), self.set_key.clone(), self.member.clone());
        runtime.spawn(async move {
            if let Err(error) = redis_slots::release(&shared, &set_key, &member).await {
                warn_upkeep(&error, "the slot frees itself when its lease runs out");
            }
        });
    }
}

/// An acquire that could not use Redis.
static FALLBACK_WARNING: Throttle = Throttle::new();
/// A renewal or a release that failed: a different line, so a flood of one
/// cannot hide the other.
static UPKEEP_WARNING: Throttle = Throttle::new();

/// Say that taking a slot in Redis failed, at most once a minute. `error` is
/// Redis's or the network's own text, never the URL.
fn warn_fallback(error: &SharedError, consequence: &str) {
    if FALLBACK_WARNING.due(Instant::now()) {
        tracing::warn!(%error, "Redis parallel-slot limiting failed; {consequence}");
    }
}

/// Say that renewing or releasing a slot in Redis failed, at most once a
/// minute.
fn warn_upkeep(error: &SharedError, consequence: &str) {
    if UPKEEP_WARNING.due(Instant::now()) {
        tracing::warn!(%error, "Redis parallel-slot upkeep failed; {consequence}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::management::enforce::in_flight;
    use crate::shared::testing::{dead_redis, flush_keys, two_shared_limiters};

    // The Redis ones need `LITEGEN_REDIS_TEST_URL` and skip themselves without
    // it; the ones on `dead_redis` and `RateLimiter::new` need no container.

    fn unique_id() -> String {
        format!("slot-test-{}", uuid::Uuid::new_v4())
    }

    /// Take a slot, trying again until the cap has room: a release is a task
    /// spawned by the drop, so it lands a moment later. Panics after 2 s.
    async fn slot_within_2s(
        limiter: &RateLimiter,
        entity_type: EntityType,
        id: &str,
        max: u32,
        lease: Duration,
        refresh_every: Duration,
    ) -> SlotGuard {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match acquire_slot_with(limiter, entity_type, id, max, lease, refresh_every).await {
                Ok(slot) => return slot,
                Err(refusal) => assert!(Instant::now() < deadline, "the slot was not given back within 2 s ({refusal:?})"),
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_cap_holds_across_two_limiters() {
        let Some((a, b)) = two_shared_limiters().await else { return };
        let id = uuid::Uuid::new_v4().to_string();
        let _s1 = acquire_slot(&a, EntityType::Key, &id, 2).await.unwrap();
        let _s2 = acquire_slot(&b, EntityType::Key, &id, 2).await.unwrap();
        assert!(acquire_slot(&a, EntityType::Key, &id, 2).await.is_err());
        drop(_s1);
        // Released: the slot comes back to the other replica.
        let _s3 = slot_within_2s(&b, EntityType::Key, &id, 2, SLOT_LEASE, SLOT_REFRESH_EVERY).await;
    }

    #[tokio::test]
    async fn shared_slots_are_per_principal() {
        let Some((a, b)) = two_shared_limiters().await else { return };
        let id = unique_id();
        let _held = acquire_slot(&a, EntityType::Key, &id, 1).await.unwrap();
        assert!(acquire_slot(&b, EntityType::Org, &id, 1).await.is_ok(), "another kind of principal, same id");
        assert!(acquire_slot(&b, EntityType::Key, &unique_id(), 1).await.is_ok(), "another id");
        assert!(matches!(acquire_slot(&b, EntityType::Key, &id, 1).await, Err(LimitRefusal::Exceeded { .. })));
    }

    #[tokio::test]
    async fn a_running_request_keeps_its_slot_past_the_lease() {
        let Some((a, b)) = two_shared_limiters().await else { return };
        let id = unique_id();
        let lease = Duration::from_millis(1_000);
        let every = Duration::from_millis(100);
        let held = acquire_slot_with(&a, EntityType::Key, &id, 1, lease, every).await.unwrap();
        // More than two leases on: the refresher has renewed it again and again.
        tokio::time::sleep(Duration::from_millis(2_200)).await;
        assert!(
            matches!(acquire_slot_with(&b, EntityType::Key, &id, 1, lease, every).await, Err(LimitRefusal::Exceeded { .. })),
            "the lease lapsed under a running request"
        );
        drop(held);
        let _next = slot_within_2s(&b, EntityType::Key, &id, 1, lease, every).await;
    }

    #[tokio::test]
    async fn a_killed_replicas_slot_frees_itself() {
        let Some((a, b)) = two_shared_limiters().await else { return };
        let id = unique_id();
        let lease = Duration::from_millis(1_000);
        let every = Duration::from_millis(100);
        let SlotGuard::Shared(slot) = acquire_slot_with(&a, EntityType::Key, &id, 1, lease, every).await.unwrap() else {
            panic!("with Redis the slot is a shared one")
        };
        // A crash stops the refresher and runs no destructor, so nothing is released.
        slot.refresher.abort();
        std::mem::forget(slot);
        assert!(acquire_slot_with(&b, EntityType::Key, &id, 1, lease, every).await.is_err(), "still leased");
        tokio::time::sleep(Duration::from_millis(1_300)).await;
        assert!(acquire_slot_with(&b, EntityType::Key, &id, 1, lease, every).await.is_ok(), "the lease ran out");
    }

    #[tokio::test]
    async fn a_refresher_stops_when_its_member_is_gone_from_redis() {
        let Some((a, _)) = two_shared_limiters().await else { return };
        let id = unique_id();
        let every = Duration::from_millis(100);
        let SlotGuard::Shared(slot) = acquire_slot_with(&a, EntityType::Key, &id, 1, SLOT_LEASE, every).await.unwrap() else {
            panic!("with Redis the slot is a shared one")
        };
        // It renews while the member is there ...
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(!slot.refresher.is_finished());
        // ... and ends at its next tick once Redis has lost the set, as after a restart.
        flush_keys(a.shared().unwrap()).await;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !slot.refresher.is_finished() {
            assert!(Instant::now() < deadline, "the refresher kept running after its member was gone");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// An acquire whose answer never comes back may still have taken its slot
    /// in Redis. Nothing holds it, so it must be given back at once, not
    /// counted against the cap for the whole lease.
    #[tokio::test]
    async fn an_acquire_whose_answer_is_lost_gives_its_slot_back() {
        lose_an_acquires_answer(false).await;
    }

    /// The same when fail-closed refuses the request with a 503.
    #[tokio::test]
    async fn a_refused_acquire_whose_answer_is_lost_gives_its_slot_back_too() {
        lose_an_acquires_answer(true).await;
    }

    /// One acquire whose reply Redis sends but the client never gets: the
    /// request is served on a local slot (fail-open) or refused (fail-closed),
    /// and the member the script added must be gone from Redis within 2 s.
    async fn lose_an_acquires_answer(fail_closed: bool) {
        use crate::config::connections::RedisSettings;
        use crate::shared::testing::{addr_of, test_prefix, test_url, CutProxy};
        let Some(url) = test_url() else { return };
        let proxy = CutProxy::start(addr_of(&url)).await;
        let prefix = test_prefix();
        let mut settings = RedisSettings::from_url(&proxy.url());
        settings.fail_closed = fail_closed;
        let behind = Arc::new(RateLimiter::with_shared(SharedRedis::connect(&settings, &prefix).await.unwrap()));
        let direct = SharedRedis::connect(&RedisSettings::from_url(&url), &prefix).await.unwrap();
        let held = |id: &str| {
            let (direct, set) = (direct.clone(), direct.key(&["slots", EntityType::Key.as_str(), id]));
            async move {
                direct.run(|mut c| async move { redis::cmd("ZCARD").arg(set).query_async::<i64>(&mut c).await }).await.unwrap()
            }
        };
        // The script is loaded and the connection made while answers come
        // back, and that slot's release has landed before the replies stop.
        let warm_up = unique_id();
        drop(acquire_slot(&behind, EntityType::Key, &warm_up, 1).await.unwrap());
        let deadline = Instant::now() + Duration::from_secs(2);
        while held(&warm_up).await != 0 {
            assert!(Instant::now() < deadline, "the warm-up slot was not given back");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        proxy.deafen();
        let id = unique_id();
        let acquiring = tokio::spawn({
            let (behind, id) = (behind.clone(), id.clone());
            async move { acquire_slot(&behind, EntityType::Key, &id, 1).await }
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while held(&id).await == 0 {
            assert!(Instant::now() < deadline, "the acquire never reached Redis");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        proxy.restore(); // new connections work again; the deaf one stays dead
        let slot = acquiring.await.unwrap();
        if fail_closed {
            assert!(matches!(slot, Err(LimitRefusal::Unverifiable)), "fail-closed refuses: {slot:?}");
        } else {
            assert!(matches!(slot, Ok(SlotGuard::Local(_))), "fail-open takes a local slot: {slot:?}");
        }
        drop(slot);

        let deadline = Instant::now() + Duration::from_secs(2);
        while held(&id).await != 0 {
            assert!(
                Instant::now() < deadline,
                "the member of an acquire whose answer was lost still counts against the cap (it would until \
                 its 30 s lease ends; fail_closed {fail_closed})"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        flush_keys(&direct).await;
    }

    #[tokio::test]
    async fn without_redis_a_slot_is_local() {
        let limiter = RateLimiter::new();
        let id = unique_id();
        let a = acquire_slot(&limiter, EntityType::Key, &id, 1).await.unwrap();
        assert!(matches!(a, SlotGuard::Local(_)));
        assert_eq!(in_flight(EntityType::Key, &id), 1);
        assert!(matches!(acquire_slot(&limiter, EntityType::Key, &id, 1).await, Err(LimitRefusal::Exceeded { .. })));
        drop(a);
        assert_eq!(in_flight(EntityType::Key, &id), 0);
    }

    #[tokio::test]
    async fn redis_down_falls_back_to_local_slots() {
        let limiter = RateLimiter::with_shared(dead_redis(false).await);
        let id = unique_id();
        let a = acquire_slot(&limiter, EntityType::Key, &id, 2).await.unwrap();
        let _b = acquire_slot(&limiter, EntityType::Key, &id, 2).await.unwrap();
        assert!(matches!(a, SlotGuard::Local(_)), "counted on this replica");
        assert!(matches!(acquire_slot(&limiter, EntityType::Key, &id, 2).await, Err(LimitRefusal::Exceeded { .. })));
        assert_eq!(in_flight(EntityType::Key, &id), 2);
        drop(a);
        assert_eq!(in_flight(EntityType::Key, &id), 1);
    }

    #[tokio::test]
    async fn fail_closed_refuses_a_slot_when_redis_is_down_and_counts_none_locally() {
        let limiter = RateLimiter::with_shared(dead_redis(true).await);
        let id = unique_id();
        for _ in 0..3 {
            assert!(matches!(acquire_slot(&limiter, EntityType::Key, &id, 1).await, Err(LimitRefusal::Unverifiable)));
        }
        assert_eq!(in_flight(EntityType::Key, &id), 0, "no local slot was taken");
    }

    #[test]
    fn dropping_a_shared_slot_outside_a_runtime_does_not_panic() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let slot = runtime.block_on(async {
            let shared = dead_redis(false).await;
            SharedSlot::start(shared, "k".to_string(), "m".to_string(), SLOT_LEASE, SLOT_REFRESH_EVERY)
        });
        // This thread has no runtime: the release is skipped and the lease
        // runs out by itself.
        assert!(tokio::runtime::Handle::try_current().is_err());
        drop(slot);
    }

    #[tokio::test]
    async fn dropping_a_shared_slot_ends_its_refresher() {
        // No Redis needed: the refresher sleeps a lease-fraction before its
        // first call, and the release fails against a dead address.
        let shared = dead_redis(false).await;
        let baseline = Arc::strong_count(&shared);
        let slot = SharedSlot::start(shared.clone(), "k".to_string(), "m".to_string(), SLOT_LEASE, SLOT_REFRESH_EVERY);
        assert!(Arc::strong_count(&shared) > baseline, "the slot and its refresher hold the client");
        drop(slot);
        // A refresher that kept running would hold its clone of the client
        // for good; the release task holds one only until it has failed.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Arc::strong_count(&shared) > baseline {
            assert!(
                Instant::now() < deadline,
                "{} holders of the client are left; the refresher was not stopped",
                Arc::strong_count(&shared) - baseline
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
