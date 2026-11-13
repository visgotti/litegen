//! Parallel-request slots that all replicas share (`docs/scaling.md`).
//!
//! A principal's cap lives in one sorted set under
//! `key(["slots", entity_type, entity_id])`. Each in-flight request is a
//! member, a fresh UUID, and its score is when its lease ends, in
//! milliseconds. The cap is the number of members whose lease has not ended.
//!
//! - **A lease, not a counter.** The holder renews its lease while the request
//!   runs ([`refresh`]) and gives the slot back when it ends ([`release`]). A
//!   replica that crashes renews nothing, so its slots free themselves when
//!   the lease runs out ([`SLOT_LEASE`]). A bare counter would stay high
//!   forever.
//! - **One step.** [`acquire`] sweeps the leases that ended, counts what is
//!   left and adds the member in one `EVAL`, which Redis runs without
//!   interleaving another command. Two replicas taking at the same instant
//!   cannot both see the last free slot.
//! - **One clock.** The time is `redis.call('TIME')`, never a timestamp a
//!   replica sends. Replicas' clocks differ, and a replica that is a few
//!   seconds behind would see another's live lease as ended.
//! - **A refresh never adds.** It is `ZADD XX`: a member that was released,
//!   or swept after its lease ended, stays gone. A refresh that lost a race
//!   with the release therefore cannot bring the slot back.
//! - **The set expires.** Every acquire and refresh sets the key to expire in
//!   twice the lease, so a principal nobody uses any more leaves nothing
//!   behind. A refresh sets it too: a request that runs for minutes, with no
//!   other request acquiring, would otherwise see the whole set expire under
//!   it, and the next acquire would count an empty set.
//!
//! Both scripts run through [`redis::Script`]: sent by hash, and loaded again
//! when Redis answers `NOSCRIPT` (after a restart, a failover or
//! `SCRIPT FLUSH`). Redis 5 and later (and Valkey) allow a script to read the
//! clock and then write, because they replicate its effects, not its text.
//!
//! The holder side (the refresher task, the release on drop, the fallback to
//! this process's own slots) is `management::slots`.

use std::sync::LazyLock;
use std::time::Duration;

use super::{SharedError, SharedRedis};

/// How long a slot's lease lasts without a refresh. A crashed replica's slots
/// are free this long after its last refresh.
pub const SLOT_LEASE: Duration = Duration::from_secs(30);

/// How often a held slot's lease is renewed: three tries per lease.
pub const SLOT_REFRESH_EVERY: Duration = Duration::from_secs(10);

/// Takes a slot in the set `KEYS[1]` if fewer than the cap are leased.
///
/// - `ARGV[1]`: the member, unique to this request.
/// - `ARGV[2]`: the cap.
/// - `ARGV[3]`: the lease, in milliseconds.
///
/// Returns 1 if the member was added, 0 if the cap is reached.
pub const SCRIPT_SOURCE: &str = r#"
local t = redis.call('TIME')
local now = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
local lease = tonumber(ARGV[3])
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', string.format('%.0f', now))
if redis.call('ZCARD', KEYS[1]) >= tonumber(ARGV[2]) then
  return 0
end
redis.call('ZADD', KEYS[1], string.format('%.0f', now + lease), ARGV[1])
redis.call('PEXPIRE', KEYS[1], string.format('%.0f', lease * 2))
return 1
"#;

/// Renews the lease of a member of the set `KEYS[1]`, if it is still in it.
///
/// - `ARGV[1]`: the member.
/// - `ARGV[2]`: the lease, in milliseconds.
///
/// Returns 1 if the member was renewed, 0 if it is gone (released, or swept
/// after its lease ended). A member that is gone is not added back.
pub const REFRESH_SOURCE: &str = r#"
local t = redis.call('TIME')
local now = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
local lease = tonumber(ARGV[2])
if not redis.call('ZSCORE', KEYS[1], ARGV[1]) then
  return 0
end
redis.call('ZADD', KEYS[1], 'XX', string.format('%.0f', now + lease), ARGV[1])
redis.call('PEXPIRE', KEYS[1], string.format('%.0f', lease * 2))
return 1
"#;

/// Built once, so their SHA-1s are computed once.
static ACQUIRE: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(SCRIPT_SOURCE));
static REFRESH: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(REFRESH_SOURCE));

fn millis(lease: Duration) -> u64 {
    u64::try_from(lease.as_millis()).unwrap_or(u64::MAX)
}

/// Take a slot for `member` in the set `set_key`, leased for `lease`. Returns
/// whether there was one: `false` means `max` slots are leased already.
pub async fn acquire(
    shared: &SharedRedis,
    set_key: &str,
    member: &str,
    max: u32,
    lease: Duration,
) -> Result<bool, SharedError> {
    let (set_key, member, lease_ms) = (set_key.to_string(), member.to_string(), millis(lease));
    let granted: i64 = shared
        .run(|mut connection| async move {
            let mut invocation = ACQUIRE.prepare_invoke();
            invocation.key(set_key.as_str()).arg(member.as_str()).arg(max).arg(lease_ms);
            invocation.invoke_async(&mut connection).await
        })
        .await?;
    Ok(granted == 1)
}

/// Renew `member`'s lease to `lease` from now, and the set's expiry to twice
/// that. A member that is gone is left gone: there is nothing to renew, and
/// the cap it counted against has been given to someone else.
pub async fn refresh(shared: &SharedRedis, set_key: &str, member: &str, lease: Duration) -> Result<(), SharedError> {
    refresh_member(shared, set_key, member, lease).await.map(|_| ())
}

/// [`refresh`], saying whether `member` was still in the set: `false` means it
/// is gone (released, swept after its lease ended, or lost with Redis's data),
/// and nothing was written. The holder's refresher stops on it.
pub(crate) async fn refresh_member(
    shared: &SharedRedis,
    set_key: &str,
    member: &str,
    lease: Duration,
) -> Result<bool, SharedError> {
    let (set_key, member, lease_ms) = (set_key.to_string(), member.to_string(), millis(lease));
    let renewed: i64 = shared
        .run(|mut connection| async move {
            let mut invocation = REFRESH.prepare_invoke();
            invocation.key(set_key.as_str()).arg(member.as_str()).arg(lease_ms);
            invocation.invoke_async(&mut connection).await
        })
        .await?;
    Ok(renewed == 1)
}

/// Give `member`'s slot back. Quiet when it is gone already (an expired
/// lease, a flushed Redis).
pub async fn release(shared: &SharedRedis, set_key: &str, member: &str) -> Result<(), SharedError> {
    let (set_key, member) = (set_key.to_string(), member.to_string());
    shared
        .run(|mut connection| async move {
            redis::cmd("ZREM").arg(set_key.as_str()).arg(member.as_str()).query_async::<i64>(&mut connection).await
        })
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::testing::{flush_keys, flush_scripts, two_shared};

    /// The set's members and their scores, lowest first.
    async fn members(s: &SharedRedis, set: &str) -> Vec<(String, f64)> {
        let set = set.to_string();
        s.run(|mut c| async move {
            redis::cmd("ZRANGE").arg(&set).arg(0).arg(-1).arg("WITHSCORES").query_async(&mut c).await
        })
        .await
        .unwrap()
    }

    async fn pttl(s: &SharedRedis, set: &str) -> i64 {
        let set = set.to_string();
        s.run(|mut c| async move { redis::cmd("PTTL").arg(&set).query_async(&mut c).await }).await.unwrap()
    }

    #[tokio::test]
    async fn a_lost_holder_frees_its_slot_after_the_lease() {
        let Some((s, _)) = two_shared().await else { return };
        let set = s.key(&["slots", "key", "t"]);
        let lease = Duration::from_millis(1_000);
        assert!(acquire(&s, &set, "gone", 1, lease).await.unwrap()); // never released or refreshed
        assert!(!acquire(&s, &set, "next", 1, lease).await.unwrap());
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        assert!(acquire(&s, &set, "next", 1, lease).await.unwrap());
    }

    #[tokio::test]
    async fn releasing_after_a_flush_is_quiet() {
        let Some((s, _)) = two_shared().await else { return };
        let set = s.key(&["slots", "key", "f"]);
        acquire(&s, &set, "m", 1, SLOT_LEASE).await.unwrap();
        flush_keys(&s).await;
        assert!(release(&s, &set, "m").await.is_ok());
    }

    #[test]
    fn scripts_use_the_redis_clock() {
        assert!(crate::shared::slots::SCRIPT_SOURCE.contains("redis.call('TIME')"));
        assert!(REFRESH_SOURCE.contains("redis.call('TIME')"));
        // The arguments are the member, the cap and the lease: never a time.
        assert!(!SCRIPT_SOURCE.contains("ARGV[4]"), "the time is never an argument: replicas' clocks differ");
        assert!(!REFRESH_SOURCE.contains("ARGV[3]"), "the time is never an argument: replicas' clocks differ");
    }

    #[tokio::test]
    async fn a_refresh_renews_the_sets_expiry() {
        let Some((s, _)) = two_shared().await else { return };
        let set = s.key(&["slots", "key", "ttl"]);
        assert!(acquire(&s, &set, "m", 1, SLOT_LEASE).await.unwrap());
        let ttl = pttl(&s, &set).await;
        assert!((50_000..=60_000).contains(&ttl), "twice the lease, got {ttl}");

        // The set's expiry runs down; a holder that only refreshes (no new
        // acquires) must keep it alive past twice the lease.
        s.run(|mut c| {
            let set = set.clone();
            async move { redis::cmd("PEXPIRE").arg(&set).arg(5_000).query_async::<i64>(&mut c).await }
        })
        .await
        .unwrap();
        assert!(pttl(&s, &set).await <= 5_000);
        refresh(&s, &set, "m", SLOT_LEASE).await.unwrap();
        let ttl = pttl(&s, &set).await;
        assert!((50_000..=60_000).contains(&ttl), "the refresh renewed it to twice the lease, got {ttl}");
    }

    #[tokio::test]
    async fn a_refresh_moves_the_lease_end_and_adds_no_member() {
        let Some((s, _)) = two_shared().await else { return };
        let set = s.key(&["slots", "key", "r"]);
        assert!(acquire(&s, &set, "m", 1, SLOT_LEASE).await.unwrap());
        let before = members(&s, &set).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        refresh(&s, &set, "m", SLOT_LEASE).await.unwrap();
        let after = members(&s, &set).await;
        assert_eq!((before.len(), after.len()), (1, 1));
        assert_eq!(after[0].0, "m");
        let moved = after[0].1 - before[0].1;
        assert!((50.0..=2_000.0).contains(&moved), "the lease end moved by {moved} ms");
    }

    #[tokio::test]
    async fn a_refresh_never_brings_back_a_released_member() {
        let Some((s, _)) = two_shared().await else { return };
        let set = s.key(&["slots", "key", "x"]);
        assert!(acquire(&s, &set, "m", 1, SLOT_LEASE).await.unwrap());
        release(&s, &set, "m").await.unwrap();
        refresh(&s, &set, "m", SLOT_LEASE).await.unwrap();
        assert!(members(&s, &set).await.is_empty(), "ZADD XX adds nothing");
        assert_eq!(pttl(&s, &set).await, -2, "and the set was not re-created");
        // The slot is free for someone else.
        assert!(acquire(&s, &set, "n", 1, SLOT_LEASE).await.unwrap());
    }

    #[tokio::test]
    async fn a_refresh_never_takes_back_a_slot_that_was_given_to_someone_else() {
        let Some((s, _)) = two_shared().await else { return };
        let set = s.key(&["slots", "key", "y"]);
        assert!(acquire(&s, &set, "late", 1, Duration::from_millis(100)).await.unwrap());
        tokio::time::sleep(Duration::from_millis(200)).await;
        // The next acquire sweeps the lease that ended and takes the slot.
        assert!(acquire(&s, &set, "next", 1, SLOT_LEASE).await.unwrap());
        // The late holder comes back: its member is gone, and stays gone.
        refresh(&s, &set, "late", SLOT_LEASE).await.unwrap();
        let held: Vec<String> = members(&s, &set).await.into_iter().map(|(member, _)| member).collect();
        assert_eq!(held, ["next"], "the cap would be passed");
        assert!(!acquire(&s, &set, "third", 1, SLOT_LEASE).await.unwrap());
    }

    #[tokio::test]
    async fn two_clients_filling_one_set_at_once_never_pass_the_cap() {
        let Some((a, b)) = two_shared().await else { return };
        let set = a.key(&["slots", "key", "burst"]);
        let tries = (0..50).map(|i| {
            let (s, set) = (if i % 2 == 0 { a.clone() } else { b.clone() }, set.clone());
            async move { acquire(&s, &set, &format!("m{i}"), 10, SLOT_LEASE).await.unwrap() }
        });
        assert_eq!(futures::future::join_all(tries).await.into_iter().filter(|ok| *ok).count(), 10);
        assert_eq!(members(&a, &set).await.len(), 10);
    }

    #[tokio::test]
    async fn the_scripts_load_again_after_a_flush() {
        let Some((s, _)) = two_shared().await else { return };
        let set = s.key(&["slots", "key", "sf"]);
        assert!(acquire(&s, &set, "m", 2, SLOT_LEASE).await.unwrap());
        refresh(&s, &set, "m", SLOT_LEASE).await.unwrap();

        flush_scripts(&s).await;
        let before = members(&s, &set).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        refresh(&s, &set, "m", SLOT_LEASE).await.unwrap();
        let after = members(&s, &set).await;
        assert!(after[0].1 > before[0].1, "the refresh script ran again after the flush");

        flush_scripts(&s).await;
        assert!(acquire(&s, &set, "n", 2, SLOT_LEASE).await.unwrap(), "the acquire script ran again after the flush");
        assert_eq!(members(&s, &set).await.len(), 2);
    }
}
