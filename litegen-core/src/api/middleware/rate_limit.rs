use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use uuid::Uuid;

struct Bucket {
    last: Instant,
    tokens: f64,
}

/// Snapshot of the current rate-limit bucket state for a key.
pub struct RateLimitSnapshot {
    /// Remaining tokens (floored to 0).
    pub remaining: u32,
    /// Approximate duration until the bucket is full again.
    pub reset_in: Duration,
}

/// In-memory token-bucket rate limiter, keyed by API key UUID.
/// Thread-safe via tokio RwLock.
pub struct RateLimiter {
    buckets: RwLock<HashMap<Uuid, Bucket>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: RwLock::new(HashMap::new()),
        }
    }

    /// Attempt to consume one token for `key_id` at `rpm` rate.
    ///
    /// Returns `Ok(())` if allowed, or `Err(retry_after_secs)` if rate limited.
    /// `retry_after_secs` is the number of seconds until one token is available.
    pub async fn try_take(&self, key_id: Uuid, rpm: u32) -> Result<(), u64> {
        let capacity = rpm as f64;
        let refill_rate = capacity / 60.0; // tokens per second
        let now = Instant::now();

        let mut buckets = self.buckets.write().await;
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
            Ok(())
        } else {
            // Time until 1 token is available
            let secs_to_refill = (1.0 - bucket.tokens) / refill_rate;
            Err(secs_to_refill.ceil() as u64)
        }
    }

    /// Return a point-in-time snapshot of the bucket for `key_id` at `rpm`.
    /// If no bucket exists yet (key hasn't made a request), returns a full bucket.
    pub async fn snapshot(&self, key_id: Uuid, rpm: u32) -> RateLimitSnapshot {
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
        let secs = err.unwrap_err();
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
}
