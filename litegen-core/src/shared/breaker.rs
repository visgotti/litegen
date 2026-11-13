//! The circuit breaker that guards every Redis call, after LiteLLM's: it opens
//! after `threshold` failures in a row, lets one probe through once `recovery`
//! has passed, closes on a probe that succeeds and opens again on one that
//! fails.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
enum Phase {
    /// Calls go through; `failures` in the state counts the streak.
    Closed,
    /// Calls fail at once until `recovery` has passed since `since`.
    Open { since: Instant },
    /// A probe went out at `probe_at`. No other call goes through until it
    /// reports. If it never does (its caller was dropped mid-call, as axum
    /// drops a handler whose client hung up), another probe goes out once
    /// `recovery` has passed since this one, so the breaker can't wedge.
    HalfOpen { probe_at: Instant },
}

#[derive(Debug)]
struct State {
    phase: Phase,
    /// Failures in a row while closed.
    failures: u32,
}

/// See the module docs. Time is passed in, so the tests don't sleep.
#[derive(Debug)]
pub struct RedisBreaker {
    threshold: u32,
    recovery: Duration,
    state: Mutex<State>,
}

impl RedisBreaker {
    /// A breaker that opens after `threshold` consecutive failures (at least 1)
    /// and probes again `recovery` after it opened.
    pub fn new(threshold: u32, recovery: Duration) -> Self {
        Self {
            threshold: threshold.max(1),
            recovery,
            state: Mutex::new(State { phase: Phase::Closed, failures: 0 }),
        }
    }

    /// Whether a call may go to Redis at `now`. When the breaker is open and
    /// `recovery` has passed, this returns true once: that call is the probe.
    pub fn allow(&self, now: Instant) -> bool {
        let mut state = self.lock();
        match state.phase {
            Phase::Closed => true,
            Phase::Open { since } => self.take_probe(&mut state, since, now),
            Phase::HalfOpen { probe_at } => self.take_probe(&mut state, probe_at, now),
        }
    }

    /// Record how a call that `allow` let through ended: `ok` is false for a
    /// timeout, a connection-level error, or a reply saying the node can't
    /// serve (`SharedRedis::run` decides, and counts the calls in flight on
    /// one lost connection once).
    pub fn record(&self, ok: bool, now: Instant) {
        let mut state = self.lock();
        match state.phase {
            Phase::Closed if ok => state.failures = 0,
            Phase::Closed => {
                state.failures += 1;
                if state.failures >= self.threshold {
                    state.phase = Phase::Open { since: now };
                }
            }
            // A call that started before the breaker opened and ends now says
            // nothing new: the probe decides.
            Phase::Open { .. } => {}
            Phase::HalfOpen { .. } if ok => {
                state.phase = Phase::Closed;
                state.failures = 0;
            }
            Phase::HalfOpen { .. } => state.phase = Phase::Open { since: now },
        }
    }

    /// The probe goes out when `recovery` has passed since `last` (when the
    /// breaker opened, or when the previous probe went out).
    fn take_probe(&self, state: &mut State, last: Instant, now: Instant) -> bool {
        if now.saturating_duration_since(last) >= self.recovery {
            state.phase = Phase::HalfOpen { probe_at: now };
            true
        } else {
            false
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_opens_after_threshold_and_probes_after_recovery() {
        let b = RedisBreaker::new(3, Duration::from_secs(60));
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(b.allow(t0));
            b.record(false, t0);
        }
        assert!(!b.allow(t0 + Duration::from_secs(59)));
        assert!(b.allow(t0 + Duration::from_secs(60)), "one probe");
        assert!(!b.allow(t0 + Duration::from_secs(60)), "only one");
        b.record(false, t0 + Duration::from_secs(60));
        assert!(!b.allow(t0 + Duration::from_secs(119)), "a failed probe opens it again");
        assert!(b.allow(t0 + Duration::from_secs(120)));
        b.record(true, t0 + Duration::from_secs(120));
        assert!(b.allow(t0 + Duration::from_secs(120)), "closed");
    }

    #[test]
    fn a_probe_that_never_reports_does_not_wedge_the_breaker() {
        let recovery = Duration::from_secs(60);
        let b = RedisBreaker::new(1, recovery);
        let t0 = Instant::now();
        assert!(b.allow(t0));
        b.record(false, t0);
        // The probe goes out, and its caller is dropped before it records.
        assert!(b.allow(t0 + recovery));
        assert!(!b.allow(t0 + recovery + Duration::from_secs(1)), "still waiting for it");
        assert!(!b.allow(t0 + 2 * recovery - Duration::from_millis(1)));
        assert!(b.allow(t0 + 2 * recovery), "a new probe goes out");
        assert!(!b.allow(t0 + 2 * recovery), "and only one");
        b.record(true, t0 + 2 * recovery);
        assert!(b.allow(t0 + 2 * recovery), "that probe closed it");
    }

    #[test]
    fn a_success_resets_the_failure_streak() {
        let b = RedisBreaker::new(3, Duration::from_secs(60));
        let t0 = Instant::now();
        for _ in 0..2 {
            b.record(false, t0);
        }
        b.record(true, t0);
        for _ in 0..2 {
            b.record(false, t0);
        }
        assert!(b.allow(t0), "two failures after a success is still under the threshold");
        b.record(false, t0);
        assert!(!b.allow(t0), "three in a row opens it");
    }

    #[test]
    fn a_late_report_from_before_it_opened_changes_nothing() {
        let b = RedisBreaker::new(1, Duration::from_secs(60));
        let t0 = Instant::now();
        b.record(false, t0);
        b.record(true, t0 + Duration::from_secs(1));
        assert!(!b.allow(t0 + Duration::from_secs(2)), "only a probe closes it");
        b.record(false, t0 + Duration::from_secs(30));
        assert!(b.allow(t0 + Duration::from_secs(60)), "a late failure doesn't push the probe back");
    }

    #[test]
    fn a_threshold_of_zero_still_needs_one_failure() {
        let b = RedisBreaker::new(0, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(b.allow(t0));
        b.record(false, t0);
        assert!(!b.allow(t0));
    }
}
