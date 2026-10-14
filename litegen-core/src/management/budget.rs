//! Budget windows: parsing `budget_duration` and rolling a window forward.
//!
//! Durations follow LiteLLM's format: `<n>s`, `<n>m`, `<n>h`, `<n>d`, `<n>w`
//! or `<n>mo` (calendar months). Windows are anchored at the moment the budget
//! was set and roll forward in whole steps, so a `30d` budget set on the 3rd
//! always resets on the 3rd + k·30 days, no matter when the next request lands.

use chrono::{DateTime, Duration, Months, Utc};

/// A parsed budget window length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetWindow {
    Fixed(Duration),
    Months(u32),
}

/// Parse a `budget_duration` string. Returns a human-readable error for the
/// API to surface on a bad value.
pub fn parse_duration(s: &str) -> Result<BudgetWindow, String> {
    let s = s.trim();
    let err = || format!("invalid budget_duration '{s}': use <n>s, <n>m, <n>h, <n>d, <n>w or <n>mo, e.g. 30d");
    let (num, unit) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(0) | None => return Err(err()),
        Some(i) => s.split_at(i),
    };
    let n: i64 = num.parse().map_err(|_| err())?;
    if n <= 0 || n > 100_000 {
        return Err(err());
    }
    Ok(match unit {
        "s" => BudgetWindow::Fixed(Duration::seconds(n)),
        "m" => BudgetWindow::Fixed(Duration::minutes(n)),
        "h" => BudgetWindow::Fixed(Duration::hours(n)),
        "d" => BudgetWindow::Fixed(Duration::days(n)),
        "w" => BudgetWindow::Fixed(Duration::weeks(n)),
        "mo" => BudgetWindow::Months(u32::try_from(n).map_err(|_| err())?),
        _ => return Err(err()),
    })
}

impl BudgetWindow {
    /// `from` advanced by one window.
    pub fn after(&self, from: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            BudgetWindow::Fixed(d) => from + *d,
            BudgetWindow::Months(n) => from
                .checked_add_months(Months::new(*n))
                .unwrap_or(from + Duration::days(30 * i64::from(*n))),
        }
    }

    /// The first window end strictly after `now`, stepping from `reset_at`.
    /// Used when a window has lapsed — possibly several windows ago.
    pub fn next_after(&self, mut reset_at: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
        // Skip whole windows arithmetically for fixed lengths so a `1s` budget
        // idle for a year does not loop 31M times.
        if let BudgetWindow::Fixed(d) = self {
            let step = d.num_milliseconds().max(1);
            let behind = (now - reset_at).num_milliseconds();
            if behind >= 0 {
                let steps = behind / step + 1;
                return reset_at + Duration::milliseconds(step * steps);
            }
            return reset_at;
        }
        while reset_at <= now {
            reset_at = self.after(reset_at);
        }
        reset_at
    }
}

/// When a window that starts now should first reset, or None for a lifetime
/// budget. Errors on an unparseable duration.
pub fn initial_reset_at(duration: Option<&str>, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, String> {
    match duration {
        None => Ok(None),
        Some(d) => Ok(Some(parse_duration(d)?.after(now))),
    }
}

/// If the window ending at `reset_at` has lapsed by `now`, the end of the
/// window that contains `now`. None while the current window is still open,
/// or for a lifetime budget.
pub fn lapsed_window_next_reset(
    duration: Option<&str>,
    reset_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let (Some(d), Some(at)) = (duration, reset_at) else { return None };
    if at > now {
        return None;
    }
    let window = parse_duration(d).ok()?;
    Some(window.next_after(at, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, 0, 0).unwrap()
    }

    #[test]
    fn parses_every_unit() {
        assert_eq!(parse_duration("30s").unwrap(), BudgetWindow::Fixed(Duration::seconds(30)));
        assert_eq!(parse_duration("15m").unwrap(), BudgetWindow::Fixed(Duration::minutes(15)));
        assert_eq!(parse_duration("12h").unwrap(), BudgetWindow::Fixed(Duration::hours(12)));
        assert_eq!(parse_duration("30d").unwrap(), BudgetWindow::Fixed(Duration::days(30)));
        assert_eq!(parse_duration("2w").unwrap(), BudgetWindow::Fixed(Duration::weeks(2)));
        assert_eq!(parse_duration("1mo").unwrap(), BudgetWindow::Months(1));
        assert_eq!(parse_duration(" 7d ").unwrap(), BudgetWindow::Fixed(Duration::days(7)));
    }

    #[test]
    fn rejects_malformed_durations() {
        for bad in ["", "d", "30", "0d", "-1d", "30x", "1.5d", "30 d", "mo", "999999d"] {
            let e = parse_duration(bad).unwrap_err();
            assert!(e.contains("budget_duration"), "{bad}: {e}");
        }
    }

    #[test]
    fn month_windows_follow_the_calendar() {
        let w = parse_duration("1mo").unwrap();
        assert_eq!(w.after(t(2026, 1, 31, 0)), t(2026, 2, 28, 0));
        assert_eq!(w.after(t(2026, 3, 15, 9)), t(2026, 4, 15, 9));
    }

    #[test]
    fn initial_reset_is_one_window_out() {
        let now = t(2026, 10, 1, 0);
        assert_eq!(initial_reset_at(Some("1d"), now).unwrap(), Some(t(2026, 10, 2, 0)));
        assert_eq!(initial_reset_at(None, now).unwrap(), None);
        assert!(initial_reset_at(Some("nope"), now).is_err());
    }

    #[test]
    fn open_window_does_not_lapse() {
        let now = t(2026, 10, 1, 12);
        assert_eq!(lapsed_window_next_reset(Some("1d"), Some(t(2026, 10, 2, 0)), now), None);
        assert_eq!(lapsed_window_next_reset(None, Some(t(2026, 9, 1, 0)), now), None);
        assert_eq!(lapsed_window_next_reset(Some("1d"), None, now), None);
    }

    #[test]
    fn lapsed_window_rolls_to_the_window_containing_now() {
        // Window ended 2026-10-02 00:00; now is 3.5 days later → next end is 10-06.
        let next = lapsed_window_next_reset(Some("1d"), Some(t(2026, 10, 2, 0)), t(2026, 10, 5, 12));
        assert_eq!(next, Some(t(2026, 10, 6, 0)));
        // Landing exactly on a boundary starts the following window.
        let next = lapsed_window_next_reset(Some("1d"), Some(t(2026, 10, 2, 0)), t(2026, 10, 2, 0));
        assert_eq!(next, Some(t(2026, 10, 3, 0)));
        // Months step through the calendar.
        let next = lapsed_window_next_reset(Some("1mo"), Some(t(2026, 1, 15, 0)), t(2026, 4, 20, 0));
        assert_eq!(next, Some(t(2026, 5, 15, 0)));
    }

    #[test]
    fn tiny_windows_idle_for_long_do_not_loop() {
        let start = t(2020, 1, 1, 0);
        let now = t(2026, 1, 1, 0);
        let next = parse_duration("1s").unwrap().next_after(start, now);
        assert!(next > now && next <= now + Duration::seconds(1));
    }
}
