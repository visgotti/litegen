use chrono::{DateTime, Duration, Utc};

pub const MAX_FAILS: usize = 5;
pub const WINDOW_MINUTES: i64 = 15;

pub fn is_locked_out(failures: &[DateTime<Utc>], now: DateTime<Utc>) -> bool {
    let window_start = now - Duration::minutes(WINDOW_MINUTES);
    let count = failures.iter().filter(|t| **t >= window_start).count();
    count >= MAX_FAILS
}

pub fn retry_after_seconds(failures: &[DateTime<Utc>], now: DateTime<Utc>) -> i64 {
    let window_start = now - Duration::minutes(WINDOW_MINUTES);
    let oldest_in_window = failures
        .iter()
        .filter(|t| **t >= window_start)
        .min()
        .copied();
    match oldest_in_window {
        Some(t) => {
            let unlock_at = t + Duration::minutes(WINDOW_MINUTES);
            (unlock_at - now).num_seconds().max(0)
        }
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    #[test]
    fn under_threshold_not_locked_out() {
        let attempts: Vec<chrono::DateTime<Utc>> =
            (0..4).map(|_| Utc::now() - Duration::seconds(1)).collect();
        assert!(!is_locked_out(&attempts, Utc::now()));
    }

    #[test]
    fn five_failures_within_window_locks_out() {
        let now = Utc::now();
        let attempts: Vec<_> = (0..5).map(|_| now - Duration::minutes(1)).collect();
        assert!(is_locked_out(&attempts, now));
    }

    #[test]
    fn old_failures_outside_window_dont_lock() {
        let now = Utc::now();
        let attempts: Vec<_> = (0..5).map(|_| now - Duration::minutes(30)).collect();
        assert!(!is_locked_out(&attempts, now));
    }

    #[test]
    fn retry_after_returns_seconds_until_window_clears() {
        let now = Utc::now();
        // Oldest failure 1 min ago, window is 15 min → 14 min left.
        let attempts: Vec<_> = (0..5).map(|_| now - Duration::minutes(1)).collect();
        let ra = retry_after_seconds(&attempts, now);
        assert!((60 * 14..=60 * 15).contains(&ra));
    }
}
