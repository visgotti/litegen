//! Keeps a log line about a Redis outage from flooding the log.
//!
//! Every limit that falls back (or refuses) while Redis is out would say so on
//! every request. A [`Throttle`] lets one line through per
//! [`OUTAGE_WARNING_EVERY`] and drops the rest.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How often the log may say that Redis is failing.
pub const OUTAGE_WARNING_EVERY: Duration = Duration::from_secs(60);

/// Lets a log line through at most once every [`OUTAGE_WARNING_EVERY`].
///
/// Keep one per line that could repeat: a flood of one kind must not hide
/// another. It lives in a field or, being `const`, in a `static`.
#[derive(Debug, Default)]
pub struct Throttle(Mutex<Option<Instant>>);

impl Throttle {
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Whether the log may speak at `now`. If so, `now` becomes the last time,
    /// and the next answer is yes only [`OUTAGE_WARNING_EVERY`] later.
    pub fn due(&self, now: Instant) -> bool {
        // A panic while holding the lock leaves a time that is still right.
        let mut last = self.0.lock().unwrap_or_else(|p| p.into_inner());
        match *last {
            Some(then) if now.saturating_duration_since(then) < OUTAGE_WARNING_EVERY => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_warning_comes_once_a_minute_at_most() {
        let throttle = Throttle::new();
        let t0 = Instant::now();
        assert!(throttle.due(t0), "the first one");
        assert!(!throttle.due(t0 + Duration::from_secs(1)));
        assert!(!throttle.due(t0 + Duration::from_secs(59)));
        assert!(throttle.due(t0 + Duration::from_secs(60)), "a minute on");
        assert!(!throttle.due(t0 + Duration::from_secs(61)), "counted from the last one");
    }
}
