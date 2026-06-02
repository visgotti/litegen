#[cfg(test)]
mod tests {
    use std::time::Duration;
    use super::super::CircuitBreaker;

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
}
