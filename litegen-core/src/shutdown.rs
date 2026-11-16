//! What a graceful stop waits for before the runtime goes away (`src/main.rs`).
//!
//! When `main` returns, the runtime drops every task still running, and a task
//! spawned while it shuts down never runs. A billed request gives back what it
//! reserved from its guards' drops (`QuotaHold`, `LimitGuard`), in tasks those
//! drops spawn, and its settlement and rows are written in tasks of their own:
//! dropped with the runtime, none of that happens. So that work, and the
//! billed requests themselves, are tracked here:
//!
//! - [`spawn_tracked`] spawns work a stop waits for: the release of a
//!   reservation, a submit's settlement and rows, a request-log write, and a
//!   failed settlement's retries, which go on until it lands. A stop waits no
//!   longer than its grace's deadline plus a few seconds ([`finish`]'s
//!   `then`); the runtime then drops what still runs, and the sweep gives
//!   back the reservations it held.
//! - [`cut_at_stop`] runs a billed request (the idempotency layer wraps every
//!   billed POST in it). At a stop it gets until the grace's deadline. One
//!   still running then is cut: dropped while the runtime still runs, so the
//!   releases its guards spawn as they drop are tracked and run.
//! - [`finish`] is the stop's wait: until the deadline for both, then the cut,
//!   then a bounded wait for the work the cut started.
//!
//! What a crash, a kill or a lost database reply leaves is recorded in the
//! database instead, and given back by a sweep (`management::reservations`).

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// One process's stop: the tracked work and the billed requests running, and
/// whether the cut has come.
pub struct Stop {
    /// Tracked work running now.
    work: watch::Sender<usize>,
    /// Billed requests running now.
    billed: watch::Sender<usize>,
    /// True from the cut on.
    cut: watch::Sender<bool>,
}

/// One of a stop's counts, held while a task runs and given back when it
/// ends, however it ends: finished, panicked or dropped.
struct Running {
    stop: Arc<Stop>,
    billed: bool,
}

impl Running {
    fn start(stop: &Arc<Stop>, billed: bool) -> Self {
        stop.count(billed).send_modify(|n| *n += 1);
        Self { stop: stop.clone(), billed }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.count(self.billed).send_modify(|n| *n = n.saturating_sub(1));
    }
}

/// What [`Stop::finish`] found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Finished {
    /// The billed requests still running at the deadline, and cut.
    pub cut: usize,
    /// The tracked work and billed requests still running when the wait after
    /// the cut ended. What they hold is given back by a sweep.
    pub left: usize,
}

impl Stop {
    /// A stop with nothing running.
    pub fn new() -> Arc<Self> {
        Arc::new(Self { work: watch::channel(0).0, billed: watch::channel(0).0, cut: watch::channel(false).0 })
    }

    fn count(&self, billed: bool) -> &watch::Sender<usize> {
        if billed {
            &self.billed
        } else {
            &self.work
        }
    }

    /// Spawn `work`, which a stop lets finish. Like `tokio::spawn`, it panics
    /// outside a runtime.
    pub fn spawn_tracked<F>(self: &Arc<Self>, work: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let running = Running::start(self, false);
        tokio::spawn(async move {
            let _running = running;
            work.await
        })
    }

    /// [`spawn_tracked`](Self::spawn_tracked), or `None` outside a runtime
    /// (`work` is then dropped unrun): for a `Drop`, which may run anywhere.
    pub fn try_spawn_tracked<F>(self: &Arc<Self>, work: F) -> Option<JoinHandle<F::Output>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let running = Running::start(self, false);
        Some(runtime.spawn(async move {
            let _running = running;
            work.await
        }))
    }

    /// Run a billed request to its end, or until the stop cuts it: `None`
    /// then, once it has been dropped. Its drops run before it stops being
    /// counted, so the work they spawn is counted first.
    pub async fn cut_at_stop<F: Future>(self: &Arc<Self>, request: F) -> Option<F::Output> {
        let _running = Running::start(self, true);
        let mut cut = self.cut.subscribe();
        tokio::select! {
            out = request => Some(out),
            Ok(_) = cut.wait_for(|cut_came| *cut_came) => None,
        }
    }

    /// Until nothing tracked and no billed request runs.
    async fn until_idle(&self) {
        let (mut work, mut billed) = (self.work.subscribe(), self.billed.subscribe());
        loop {
            // Both marked seen before they are read, so a change after the
            // read wakes the wait.
            if *work.borrow_and_update() == 0 && *billed.borrow_and_update() == 0 {
                return;
            }
            tokio::select! {
                _ = work.changed() => {}
                _ = billed.changed() => {}
            }
        }
    }

    /// The stop's wait: the tracked work and the billed requests get until
    /// `deadline`; then the billed requests still running are cut, and the
    /// work running then (the releases their drops spawned among it) gets
    /// `then` more.
    pub async fn finish(&self, deadline: Instant, then: Duration) -> Finished {
        if tokio::time::timeout_at(deadline, self.until_idle()).await.is_ok() {
            return Finished::default();
        }
        let cut = *self.billed.borrow();
        self.cut.send_replace(true);
        match tokio::time::timeout(then, self.until_idle()).await {
            Ok(()) => Finished { cut, left: 0 },
            Err(_) => Finished { cut, left: *self.work.borrow() + *self.billed.borrow() },
        }
    }
}

/// This process's stop.
fn process() -> &'static Arc<Stop> {
    static STOP: std::sync::OnceLock<Arc<Stop>> = std::sync::OnceLock::new();
    STOP.get_or_init(Stop::new)
}

/// Spawn `work`, which this process's stop lets finish ([`Stop::spawn_tracked`]).
pub fn spawn_tracked<F>(work: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    process().spawn_tracked(work)
}

/// [`spawn_tracked`], or `None` outside a runtime ([`Stop::try_spawn_tracked`]).
pub fn try_spawn_tracked<F>(work: F) -> Option<JoinHandle<F::Output>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    process().try_spawn_tracked(work)
}

/// Run a billed request, which this process's stop cuts at its grace's end
/// ([`Stop::cut_at_stop`]).
pub async fn cut_at_stop<F: Future>(request: F) -> Option<F::Output> {
    process().cut_at_stop(request).await
}

/// This process's stop ([`Stop::finish`]). `main` calls it once, at the stop.
pub async fn finish(deadline: Instant, then: Duration) -> Finished {
    process().finish(deadline, then).await
}

#[cfg(test)]
mod tests {
    //! On paused time: each test's waits end at their deadlines, never on a
    //! loaded machine's clock.
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Sets its flag when dropped: a guard whose drop gives something back.
    struct GivesBack(Arc<Stop>, Arc<AtomicBool>);

    impl Drop for GivesBack {
        fn drop(&mut self) {
            let given = self.1.clone();
            // The release a guard's drop spawns: it takes a while.
            self.0.try_spawn_tracked(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                given.store(true, Ordering::SeqCst);
            });
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_stop_waits_for_the_tracked_work_to_end() {
        let stop = Stop::new();
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        stop.spawn_tracked(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let finished = stop.finish(started + Duration::from_secs(10), Duration::from_secs(5)).await;
        assert!(done.load(Ordering::SeqCst), "the stop returned before the work it tracks ended");
        assert_eq!(finished, Finished::default());
        assert!(started.elapsed() < Duration::from_secs(10), "it waited for the deadline, not for the work: {:?}", started.elapsed());
    }

    #[tokio::test(start_paused = true)]
    async fn a_billed_request_still_running_at_the_deadline_is_cut_and_what_its_drop_gives_back_runs() {
        let stop = Stop::new();
        let given = Arc::new(AtomicBool::new(false));
        let guard = GivesBack(stop.clone(), given.clone());
        let request = {
            let stop = stop.clone();
            tokio::spawn(async move {
                stop.cut_at_stop(async move {
                    let _held = guard;
                    std::future::pending::<u8>().await
                })
                .await
            })
        };
        tokio::task::yield_now().await;
        let started = Instant::now();
        let finished = stop.finish(started + Duration::from_secs(10), Duration::from_secs(5)).await;
        assert_eq!(finished, Finished { cut: 1, left: 0 });
        assert!(given.load(Ordering::SeqCst), "the release the cut request's drop spawned never ran");
        assert_eq!(request.await.unwrap(), None, "a cut request answers None");
    }

    #[tokio::test(start_paused = true)]
    async fn a_billed_request_that_ends_in_time_is_not_cut() {
        let stop = Stop::new();
        let request = {
            let stop = stop.clone();
            tokio::spawn(async move {
                stop.cut_at_stop(async {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    7
                })
                .await
            })
        };
        tokio::task::yield_now().await;
        let started = Instant::now();
        let finished = stop.finish(started + Duration::from_secs(10), Duration::from_secs(5)).await;
        assert_eq!(finished, Finished::default());
        assert_eq!(request.await.unwrap(), Some(7));
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    }

    #[tokio::test(start_paused = true)]
    async fn work_still_running_after_the_wait_is_counted_as_left() {
        let stop = Stop::new();
        stop.spawn_tracked(std::future::pending::<()>());
        let started = Instant::now();
        let finished = stop.finish(started + Duration::from_secs(10), Duration::from_secs(5)).await;
        assert_eq!(finished, Finished { cut: 0, left: 1 });
        assert!(started.elapsed() >= Duration::from_secs(15), "the deadline, then the wait after it: {:?}", started.elapsed());
    }
}
