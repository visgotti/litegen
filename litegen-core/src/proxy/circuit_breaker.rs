use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::RwLock;

/// Where a [`CircuitBreaker`] keeps each provider's failure streak and the
/// time its breaker opened. [`LocalBreakerStore`] keeps them in this process.
/// The server can supply a store that several replicas share (`docs/scaling.md`);
/// the engine neither knows nor names it.
///
/// The rules every store follows:
///
/// - `record_failure` counts one more consecutive failure. When the streak
///   has reached `threshold` and no open window is running (none yet, or the
///   last one ended), the breaker opens for a window of `open_for`. So a
///   failed half-open probe opens it again, and a failure during a window
///   leaves the window's end where it was.
/// - `record_success` clears the streak and a window that has ended. While a
///   window of `open_for` is running it changes nothing: the success is from
///   a request sent before the breaker opened, and says nothing about the
///   provider now (a vendor that sheds new requests while it finishes the
///   ones it accepted would otherwise make the breaker flap). So an open
///   window always runs its full length.
/// - `is_open` is true while a window is running.
#[async_trait]
pub trait BreakerStore: Send + Sync {
    async fn is_open(&self, provider: &str, open_for: Duration) -> bool;
    async fn record_success(&self, provider: &str, open_for: Duration);
    async fn record_failure(&self, provider: &str, threshold: u32, open_for: Duration);
}

struct BreakerState {
    consecutive_failures: u32,
    opened_at: Option<Instant>,
}

impl BreakerState {
    fn new() -> Self {
        Self {
            consecutive_failures: 0,
            opened_at: None,
        }
    }

    fn is_open(&self, open_for: Duration) -> bool {
        if let Some(opened_at) = self.opened_at {
            if opened_at.elapsed() < open_for {
                return true; // Still open
            }
            // open_for has elapsed — breaker transitions to half-open (allow trial)
        }
        false
    }
}

/// The streaks of this process alone: one map, behind one lock.
#[derive(Default)]
pub struct LocalBreakerStore {
    state: RwLock<HashMap<String, BreakerState>>,
}

#[async_trait]
impl BreakerStore for LocalBreakerStore {
    async fn is_open(&self, provider: &str, open_for: Duration) -> bool {
        let state = self.state.read().await;
        state.get(provider).map(|s| s.is_open(open_for)).unwrap_or(false)
    }

    async fn record_success(&self, provider: &str, open_for: Duration) {
        let mut state = self.state.write().await;
        let entry = state.entry(provider.to_string()).or_insert_with(BreakerState::new);
        // A success during a window ends nothing (see the trait's rules).
        if entry.is_open(open_for) {
            return;
        }
        entry.consecutive_failures = 0;
        entry.opened_at = None;
    }

    async fn record_failure(&self, provider: &str, threshold: u32, open_for: Duration) {
        let mut state = self.state.write().await;
        let entry = state.entry(provider.to_string()).or_insert_with(BreakerState::new);
        entry.consecutive_failures += 1;
        // Opens when the streak is long enough and no window is running: none
        // yet, or the last one ended (a failed half-open probe opens it again).
        if entry.consecutive_failures >= threshold && entry.opened_at.is_none_or(|t| t.elapsed() >= open_for) {
            entry.opened_at = Some(Instant::now());
        }
    }
}

/// Tracks per-provider failure streaks and opens the circuit after `threshold`
/// consecutive failures, preventing calls for `open_for` duration.
pub struct CircuitBreaker {
    store: Arc<dyn BreakerStore>,
    /// Both live: gateway settings change them (`docs/gateway-settings.md`).
    threshold: AtomicU32,
    open_for_ms: AtomicU64,
}

impl CircuitBreaker {
    /// A breaker whose streaks live in this process.
    pub fn new(threshold: u32, open_for: Duration) -> Self {
        Self::with_store(threshold, open_for, Arc::new(LocalBreakerStore::default()))
    }

    /// A breaker whose streaks live in `store`.
    pub fn with_store(threshold: u32, open_for: Duration, store: Arc<dyn BreakerStore>) -> Self {
        Self {
            store,
            threshold: AtomicU32::new(threshold),
            open_for_ms: AtomicU64::new(open_for.as_millis() as u64),
        }
    }

    /// New settings: the threshold counts from the next failure, the open
    /// time from the next check, for providers already tracked too.
    pub fn reconfigure(&self, threshold: u32, open_for: Duration) {
        self.threshold.store(threshold, Ordering::SeqCst);
        self.open_for_ms.store(open_for.as_millis() as u64, Ordering::SeqCst);
    }

    fn open_for(&self) -> Duration {
        Duration::from_millis(self.open_for_ms.load(Ordering::SeqCst))
    }

    /// Returns `true` if the breaker for this provider is open (skip the call).
    pub async fn is_open(&self, provider: &str) -> bool {
        self.store.is_open(provider, self.open_for()).await
    }

    /// Call after a successful provider response. Resets the failure counter,
    /// unless a window is running: a request sent before the breaker opened
    /// that comes back fine during the window ends nothing.
    pub async fn record_success(&self, provider: &str) {
        self.store.record_success(provider, self.open_for()).await;
    }

    /// Call after a failed provider response. Opens the breaker once threshold
    /// is hit. A failure after an open window has ended (half-open) opens it
    /// again, since the streak is still over the threshold; a failure while the
    /// window is running does not extend it.
    pub async fn record_failure(&self, provider: &str) {
        self.store
            .record_failure(provider, self.threshold.load(Ordering::SeqCst), self.open_for())
            .await;
    }
}

#[cfg(test)]
#[path = "circuit_breaker_tests.rs"]
mod circuit_breaker_tests;
