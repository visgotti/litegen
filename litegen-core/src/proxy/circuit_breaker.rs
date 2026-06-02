use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

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

    fn is_open(&self, _threshold: u32, open_for: Duration) -> bool {
        if let Some(opened_at) = self.opened_at {
            if opened_at.elapsed() < open_for {
                return true; // Still open
            }
            // open_for has elapsed — breaker transitions to half-open (allow trial)
        }
        false
    }
}

/// Tracks per-provider failure streaks and opens the circuit after `threshold`
/// consecutive failures, preventing calls for `open_for` duration.
pub struct CircuitBreaker {
    state: RwLock<HashMap<String, BreakerState>>,
    threshold: u32,
    open_for: Duration,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, open_for: Duration) -> Self {
        Self {
            state: RwLock::new(HashMap::new()),
            threshold,
            open_for,
        }
    }

    /// Returns `true` if the breaker for this provider is open (skip the call).
    pub async fn is_open(&self, provider: &str) -> bool {
        let state = self.state.read().await;
        state.get(provider)
            .map(|s| s.is_open(self.threshold, self.open_for))
            .unwrap_or(false)
    }

    /// Call after a successful provider response. Resets the failure counter.
    pub async fn record_success(&self, provider: &str) {
        let mut state = self.state.write().await;
        let entry = state.entry(provider.to_string()).or_insert_with(BreakerState::new);
        entry.consecutive_failures = 0;
        entry.opened_at = None;
    }

    /// Call after a failed provider response. Opens the breaker once threshold is hit.
    pub async fn record_failure(&self, provider: &str) {
        let mut state = self.state.write().await;
        let threshold = self.threshold;
        let entry = state.entry(provider.to_string()).or_insert_with(BreakerState::new);
        entry.consecutive_failures += 1;
        if entry.consecutive_failures >= threshold && entry.opened_at.is_none() {
            entry.opened_at = Some(Instant::now());
        }
    }
}

#[cfg(test)]
#[path = "circuit_breaker_tests.rs"]
mod circuit_breaker_tests;
