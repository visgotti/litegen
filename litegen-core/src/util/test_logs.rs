//! Test only: the library test binary's process-wide tracing subscriber
//! (FU-SPANFLAKE, follow-ups 2026-11-13).
//!
//! A test reads what its own thread logs through a subscriber of its own
//! (`tracing::subscriber::set_default`). Whether a span or an event is built
//! at all is decided once per callsite and kept (tracing-core 0.1.36,
//! `callsite.rs`). While at most one dispatcher is registered, a callsite
//! first reached on some thread takes that thread's own answer, and a thread
//! with no subscriber of its own answers "never". So a callsite that another
//! test's thread reached first, while a test held the only registered
//! subscriber, stayed off for that test as well, and the test failed only
//! when it ran beside the other one: `a_client_poll_runs_in_a_span_naming_its_job`
//! failed in two parallel runs of a few modules out of two, and never alone.
//!
//! [`hold_process_subscriber`] registers one subscriber for the whole process,
//! for good. A test holding a subscriber of its own then makes at least two
//! registered dispatchers, so every callsite's answer is taken from all of
//! them, the test's own included: a callsite the test wants is never off.
//! Every test that reads its own thread's log calls it before `set_default`.
//!
//! The process-wide subscriber passes only sqlx's statement events
//! (`sqlx::query`), which sqlx's SQLite driver logs from a worker thread that
//! a per-thread subscriber never reaches, to a writer that keeps them only
//! while a [`SqlxLogs`] is open, so the other tests' statements cost no memory.

use std::sync::Mutex;

/// What sqlx logged while a [`SqlxLogs`] is open; `None` keeps nothing.
static SQLX_TEXT: Mutex<Option<Vec<u8>>> = Mutex::new(None);

struct SqlxWriter;

impl std::io::Write for SqlxWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(text) = SQLX_TEXT.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            text.extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SqlxWriter {
    type Writer = SqlxWriter;
    fn make_writer(&'a self) -> SqlxWriter {
        SqlxWriter
    }
}

/// Install the process-wide subscriber, once (see the module's notes). Any
/// test that calls `tracing_subscriber::fmt().init()`, or any other global
/// init, after it would panic, because a global default is then set: use
/// `tracing::subscriber::set_default` in a test, after calling this.
pub(crate) fn hold_process_subscriber() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(SqlxWriter)
            .with_ansi(false)
            .with_env_filter(tracing_subscriber::EnvFilter::new("sqlx::query=trace"))
            .finish();
        // An error means a global subscriber is already there; then
        // `SqlxLogs`' own probe fails, and says so.
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

/// What sqlx logged about its statements while this lives.
pub(crate) struct SqlxLogs;

impl SqlxLogs {
    pub(crate) fn open() -> SqlxLogs {
        hold_process_subscriber();
        *SQLX_TEXT.lock().unwrap_or_else(|p| p.into_inner()) = Some(Vec::new());
        SqlxLogs
    }

    pub(crate) fn text(&self) -> String {
        let text = SQLX_TEXT.lock().unwrap_or_else(|p| p.into_inner());
        String::from_utf8_lossy(text.as_deref().unwrap_or_default()).into_owned()
    }
}

impl Drop for SqlxLogs {
    fn drop(&mut self) {
        *SQLX_TEXT.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}
