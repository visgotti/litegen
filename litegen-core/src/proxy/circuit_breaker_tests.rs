#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use async_trait::async_trait;
    use super::super::{BreakerStore, CircuitBreaker};

    #[tokio::test]
    async fn threshold_reached_opens_breaker() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(30));
        cb.record_failure("openai").await;
        cb.record_failure("openai").await;
        assert!(!cb.is_open("openai").await, "Should not be open yet (2 failures < threshold 3)");
        cb.record_failure("openai").await;
        assert!(cb.is_open("openai").await, "Should be open after 3 consecutive failures");
    }

    #[tokio::test]
    async fn record_success_resets_counter() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(30));
        cb.record_failure("openai").await;
        cb.record_failure("openai").await;
        cb.record_success("openai").await;
        cb.record_failure("openai").await;
        cb.record_failure("openai").await;
        // Only 2 failures since last success — breaker should remain closed
        assert!(!cb.is_open("openai").await, "Success should have reset counter; 2 failures not enough");
    }

    #[tokio::test]
    async fn breaker_auto_recovers_after_open_for_elapses() {
        // Use a very short open_for so we can test expiry without sleeping long
        let cb = CircuitBreaker::new(2, Duration::from_millis(50));
        cb.record_failure("openai").await;
        cb.record_failure("openai").await;
        assert!(cb.is_open("openai").await, "Should be open immediately after threshold");

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!cb.is_open("openai").await, "Should auto-recover after open_for elapses");
    }

    #[tokio::test]
    async fn independent_providers_tracked_separately() {
        let cb = CircuitBreaker::new(2, Duration::from_secs(30));
        cb.record_failure("openai").await;
        cb.record_failure("openai").await;
        assert!(cb.is_open("openai").await, "openai should be open");
        assert!(!cb.is_open("stability").await, "stability should be unaffected");
    }

    #[tokio::test]
    async fn fresh_provider_is_closed() {
        let cb = CircuitBreaker::new(5, Duration::from_secs(30));
        assert!(!cb.is_open("brand-new-provider").await, "Unknown provider should start closed");
    }

    /// Gateway settings change the breaker live (`docs/gateway-settings.md`):
    /// a new threshold counts at the next failure, a new open time at the
    /// next check, for providers already tracked too.
    #[tokio::test]
    async fn a_new_threshold_and_open_time_apply_to_the_next_check() {
        let cb = CircuitBreaker::new(5, Duration::from_secs(30));
        cb.record_failure("acme").await;
        assert!(!cb.is_open("acme").await);
        cb.reconfigure(2, Duration::from_secs(30));
        cb.record_failure("acme").await;
        assert!(cb.is_open("acme").await, "two failures meet the new threshold");
        cb.reconfigure(2, Duration::from_millis(50));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(!cb.is_open("acme").await, "past the new, shorter open time: half-open");
    }

    /// A provider that keeps failing is cut off again once a window ends: the
    /// call that lets one request through (half-open) fails, and the breaker
    /// opens for another window.
    #[tokio::test]
    async fn a_failed_half_open_probe_opens_the_breaker_again() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(300));
        cb.record_failure("p").await;
        cb.record_failure("p").await;
        assert!(cb.is_open("p").await);
        tokio::time::sleep(Duration::from_millis(360)).await;
        assert!(!cb.is_open("p").await, "half-open");
        cb.record_failure("p").await;
        assert!(cb.is_open("p").await, "the failed probe opens it again");
    }

    /// Only a failure after the window opens it again: one during the window
    /// leaves the end where it was.
    #[tokio::test]
    async fn a_failure_while_open_does_not_extend_the_window() {
        // The second failure must land inside the window, so it has 400 ms of
        // margin against a stalled test thread. An extended window would end
        // at about 800 ms, so a check after 620 ms still tells them apart.
        let cb = CircuitBreaker::new(1, Duration::from_millis(600));
        cb.record_failure("p").await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        cb.record_failure("p").await;
        tokio::time::sleep(Duration::from_millis(420)).await;
        assert!(!cb.is_open("p").await, "the first window ended on time");
    }

    /// A success after the window that a failed probe reopened clears the
    /// breaker for good: the streak starts again from nothing.
    #[tokio::test]
    async fn a_success_after_a_reopen_closes_it() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(300));
        cb.record_failure("p").await;
        cb.record_failure("p").await;
        tokio::time::sleep(Duration::from_millis(360)).await;
        cb.record_failure("p").await;
        assert!(cb.is_open("p").await, "the failed probe reopened it");
        tokio::time::sleep(Duration::from_millis(360)).await;
        cb.record_success("p").await;
        assert!(!cb.is_open("p").await, "success clears it");
        // Without the clear, the old streak plus this one failure would open
        // it again at once.
        cb.record_failure("p").await;
        assert!(!cb.is_open("p").await, "the count starts again from one, under the threshold of 2");
        cb.record_failure("p").await;
        assert!(cb.is_open("p").await, "and the second failure opens it");
    }

    /// A request sent before the breaker opened that comes back fine during
    /// the window says nothing about the provider now: the window runs on.
    #[tokio::test]
    async fn a_success_from_before_the_window_does_not_end_it() {
        let cb = CircuitBreaker::new(2, Duration::from_secs(30));
        cb.record_failure("p").await;
        cb.record_failure("p").await;
        assert!(cb.is_open("p").await);
        cb.record_success("p").await;
        assert!(cb.is_open("p").await, "a late success ended the open window");
    }

    /// The router's `Retry-After` for a route whose every breaker is open
    /// (handlers review #5) is how long the first breaker has left, not the
    /// whole window; a window that has ended has nothing left.
    #[tokio::test]
    async fn an_open_breaker_says_how_long_it_has_left() {
        let cb = CircuitBreaker::new(1, Duration::from_secs(30));
        assert_eq!(cb.open_remaining("p").await, None, "closed");
        cb.record_failure("p").await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        let left = cb.open_remaining("p").await.expect("open");
        assert!((Duration::from_secs(25)..=Duration::from_millis(29_750)).contains(&left), "{left:?} of a 30 s window opened 250 ms ago");
        let short = CircuitBreaker::new(1, Duration::from_millis(200));
        short.record_failure("p").await;
        tokio::time::sleep(Duration::from_millis(260)).await;
        assert_eq!(short.open_remaining("p").await, None, "the window has ended");
    }

    /// What a store was asked, in order.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    #[async_trait]
    impl BreakerStore for Recorder {
        async fn is_open(&self, provider: &str, open_for: Duration) -> bool {
            self.0.lock().unwrap().push(format!("is_open {provider} {}ms", open_for.as_millis()));
            false
        }
        async fn record_success(&self, provider: &str, open_for: Duration) {
            self.0.lock().unwrap().push(format!("success {provider} {}ms", open_for.as_millis()));
        }
        async fn record_failure(&self, provider: &str, threshold: u32, open_for: Duration) {
            self.0.lock().unwrap().push(format!("failure {provider} {threshold} {}ms", open_for.as_millis()));
        }
    }

    /// The breaker keeps nothing itself: every call goes to its store, with
    /// the settings in force at that moment.
    #[tokio::test]
    async fn a_breaker_hands_its_store_the_current_settings() {
        let store = Arc::new(Recorder::default());
        let cb = CircuitBreaker::with_store(3, Duration::from_secs(30), store.clone());
        cb.record_failure("p").await;
        cb.reconfigure(7, Duration::from_secs(5));
        cb.record_failure("p").await;
        assert!(!cb.is_open("p").await);
        cb.record_success("p").await;
        assert_eq!(
            *store.0.lock().unwrap(),
            ["failure p 3 30000ms", "failure p 7 5000ms", "is_open p 5000ms", "success p 5000ms"]
        );
    }
}
