//! CPU work proportional to a request's size, kept off the async workers
//! (P15): hashing and decoding a body of up to 25 MiB holds a worker for tens
//! of milliseconds in release, over a second in debug, and every task queued
//! behind it waits.

/// From this size up, work is handed off; below it, the hand-off costs more
/// than the work.
const HAND_OFF_FROM: usize = 1024 * 1024;

/// Run `f`, CPU work proportional to `bytes`, without stalling the async
/// runtime: on a multi-thread runtime and from 1 MiB up, through
/// [`tokio::task::block_in_place`], so the worker's other tasks move to
/// another thread while `f` runs. Otherwise inline: on a current-thread
/// runtime (every `#[tokio::test]`, where `block_in_place` panics), outside
/// a runtime, or for a small input. Synchronous, so callers and their
/// borrows don't change, and nothing is copied.
///
/// Not for a `LocalSet` on a multi-thread runtime, where `block_in_place`
/// panics too; litegen has none.
pub fn cpu_heavy<T>(bytes: usize, f: impl FnOnce() -> T) -> T {
    let multi_thread = tokio::runtime::Handle::try_current()
        .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread);
    if multi_thread && bytes >= HAND_OFF_FROM {
        tokio::task::block_in_place(f)
    } else {
        f()
    }
}

/// Test support for P15's timing tests: the longest a task on the runtime's
/// one worker waited to run while `work` ran in a task of its own on that
/// worker, and `work`'s value. Call it from
/// `#[tokio::test(flavor = "multi_thread", worker_threads = 1)]`: the test's
/// own body runs on its main thread, every spawned task on the one worker.
/// The ticker runs before the work starts, and wakes once after it ends, so a
/// stall the work caused is always recorded.
#[cfg(test)]
pub(crate) async fn longest_stall_while<T: Send + 'static>(work: impl std::future::Future<Output = T> + Send + 'static) -> (T, std::time::Duration) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    let (gap_us, ticks) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let ticker = tokio::spawn({
        let (gap_us, ticks) = (gap_us.clone(), ticks.clone());
        async move {
            let mut last = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(1)).await;
                let now = Instant::now();
                gap_us.fetch_max((now - last).as_micros() as u64, Ordering::Relaxed);
                ticks.fetch_add(1, Ordering::Relaxed);
                last = now;
            }
        }
    });
    while ticks.load(Ordering::Relaxed) < 3 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    gap_us.store(0, Ordering::Relaxed);
    let value = tokio::spawn(work).await.expect("the work ran");
    tokio::time::sleep(Duration::from_millis(50)).await;
    ticker.abort();
    (value, Duration::from_micros(gap_us.load(Ordering::Relaxed)))
}

#[cfg(test)]
mod tests {
    use super::cpu_heavy;

    /// A current-thread runtime (every `#[tokio::test]`) has no other thread
    /// to hand the work to, and a small input isn't worth one: the work runs
    /// where it is, and its value comes back.
    #[tokio::test]
    async fn inline_on_a_current_thread_runtime_and_for_small_inputs() {
        assert_eq!(cpu_heavy(64 * 1024 * 1024, || 7), 7);
        assert_eq!(cpu_heavy(16, || "small"), "small");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn small_inputs_run_inline_on_a_multi_thread_runtime() {
        let here = std::thread::current().id();
        let ran_on = tokio::spawn(async { cpu_heavy(16, || std::thread::current().id()) }).await.unwrap();
        assert_ne!(ran_on, here, "spawned work runs on the worker");
        assert_eq!(cpu_heavy(16, || 1 + 1), 2);
    }

    /// Outside any runtime it just runs.
    #[test]
    fn outside_a_runtime_it_runs_inline() {
        assert_eq!(cpu_heavy(64 * 1024 * 1024, || "outside"), "outside");
    }
}
