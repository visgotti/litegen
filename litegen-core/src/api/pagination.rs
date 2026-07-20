//! Pagination helpers shared by the list endpoints.
//!
//! Client-supplied `page`/`per_page` reach these list handlers straight from the
//! query string. Two hazards must be neutralised before those values touch a SQL
//! `LIMIT`/`OFFSET` or an in-memory `.take()`:
//!
//! 1. An unbounded `per_page` (e.g. `per_page=4000000000`) asks the process to
//!    materialise an arbitrarily large result set into memory — an OOM DoS. We
//!    clamp it to [`MAX_PER_PAGE`].
//! 2. `(page - 1) * per_page` computed in `u32` wraps silently in release builds
//!    (`saturating_sub` guards only the subtraction, not the product), so a deep
//!    page can alias back to page 1 and return the wrong rows. We compute the
//!    offset in `u64` and saturate.

/// Largest page size any list endpoint will honour. Requests above this are
/// clamped down rather than rejected, so existing callers keep working.
pub const MAX_PER_PAGE: u32 = 200;

/// Default page size when the caller does not specify one.
pub const DEFAULT_PER_PAGE: u32 = 50;

/// Clamp a caller-supplied `per_page` into `1..=MAX_PER_PAGE`.
///
/// `0` (which would make `total_pages` divide-by-zero and return an empty page)
/// is treated as "unspecified" and mapped to [`DEFAULT_PER_PAGE`].
pub fn clamp_per_page(requested: u32) -> u32 {
    match requested {
        0 => DEFAULT_PER_PAGE,
        n => n.min(MAX_PER_PAGE),
    }
}

/// Normalise a `page` to be at least 1.
pub fn clamp_page(requested: u32) -> u32 {
    requested.max(1)
}

/// Overflow-safe zero-based offset for `page`/`per_page`.
///
/// Computed in `u64` so the product cannot wrap the way a `u32` multiply would;
/// the result is saturated into `usize`.
pub fn offset(page: u32, per_page: u32) -> usize {
    let page = clamp_page(page) as u64;
    let per_page = per_page as u64;
    let raw = (page - 1).saturating_mul(per_page);
    usize::try_from(raw).unwrap_or(usize::MAX)
}

/// Total number of pages for `total` items at `per_page` per page.
///
/// `per_page` is clamped first so a `0` can never divide-by-zero.
pub fn total_pages(total: u64, per_page: u32) -> u32 {
    let per_page = clamp_per_page(per_page) as u64;
    total.div_ceil(per_page) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_per_page_caps_large_values() {
        assert_eq!(clamp_per_page(4_000_000_000), MAX_PER_PAGE);
        assert_eq!(clamp_per_page(MAX_PER_PAGE + 1), MAX_PER_PAGE);
    }

    #[test]
    fn clamp_per_page_passes_through_reasonable_values() {
        assert_eq!(clamp_per_page(50), 50);
        assert_eq!(clamp_per_page(MAX_PER_PAGE), MAX_PER_PAGE);
    }

    #[test]
    fn clamp_per_page_zero_becomes_default() {
        assert_eq!(clamp_per_page(0), DEFAULT_PER_PAGE);
    }

    #[test]
    fn offset_does_not_overflow_on_deep_pages() {
        // page=70000, per_page=70000: (page-1)*per_page = 4_899_930_000, which
        // exceeds u32::MAX and would wrap to 604_962_704 under a u32 multiply —
        // silently returning the wrong slice. u64 math must preserve the value.
        let off = offset(70_000, 70_000);
        assert_eq!(off, 69_999_usize * 70_000);
        assert_eq!(off, 4_899_930_000, "deep-page offset must not wrap in u32");
    }

    #[test]
    fn offset_first_page_is_zero() {
        assert_eq!(offset(1, 50), 0);
        assert_eq!(offset(0, 50), 0); // page 0 normalises to page 1
    }

    #[test]
    fn total_pages_never_divides_by_zero() {
        assert_eq!(total_pages(100, 0), 2); // per_page 0 -> DEFAULT_PER_PAGE (50)
        assert_eq!(total_pages(0, 50), 0);
        assert_eq!(total_pages(101, 50), 3);
    }
}
