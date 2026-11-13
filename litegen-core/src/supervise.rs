//! Background loops that outlive a panic in one round of their work.
//!
//! A tokio task that panics ends, and a loop running in it stops for good:
//! nothing restarts it, and nothing looks at its handle before shutdown. So
//! each round of litegen's background loops (a poller tick and each row in
//! it, a retention sweep, a config sync, an OAuth purge) runs under
//! [`guarded`]: a panic is logged with its text, and the loop goes on to its
//! next round.

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt as _;

/// The text a panic carried, for the log.
pub fn panic_text(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic with no text".to_string()
    }
}

/// Run one round of a loop's work. A panic in it is logged as `what`
/// panicking, with its text, and gives `None`: the caller's loop goes on.
///
/// Whatever the round held (a permit, a claim) is dropped as the panic
/// unwinds, as it would be when a task ends.
pub async fn guarded<T>(what: &'static str, round: impl Future<Output = T>) -> Option<T> {
    match AssertUnwindSafe(round).catch_unwind().await {
        Ok(value) => Some(value),
        Err(payload) => {
            tracing::error!(panic = %panic_text(&*payload), "{what} panicked; the next round runs at its time");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_round_that_panics_gives_none_and_one_that_ends_gives_its_value() {
        assert_eq!(guarded("a round", async { 7 }).await, Some(7));
        let panicked: Option<u8> = guarded("a round", async { panic!("a round that panics") }).await;
        assert_eq!(panicked, None);
    }

    #[test]
    fn a_panics_text_is_read_whether_it_was_formatted_or_not() {
        let fixed = std::panic::catch_unwind(|| panic!("fixed text")).unwrap_err();
        assert_eq!(panic_text(&*fixed), "fixed text");
        let formatted = std::panic::catch_unwind(|| panic!("formatted {}", 7)).unwrap_err();
        assert_eq!(panic_text(&*formatted), "formatted 7");
        let other = std::panic::catch_unwind(|| std::panic::panic_any(7_u8)).unwrap_err();
        assert_eq!(panic_text(&*other), "a panic with no text");
    }
}
