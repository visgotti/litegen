//! HTTP clients shared by the whole process (P8, code review 2026-11-11).
//!
//! A `reqwest::Client` holds a connection pool, and its clones share it. An
//! adapter built for one call (an org's own key, a poller row, a price quote)
//! used to build its own clients and drop them with the call, so each such
//! call paid a new TCP and TLS handshake to the vendor. [`shared`] builds each
//! client once and hands out clones.
//!
//! - **The key names every setting** of the builder the client comes from
//!   ([`ClientKey`]), so two callers share a client only when their builders
//!   are the same: sharing never drops a redirect policy, a proxy rule, the
//!   public-only resolver or a timeout.
//! - **One set per runtime.** A pooled connection is driven by a task on the
//!   runtime that opened it, and a request another runtime sends over it
//!   fails once that runtime stops (hyper's "dispatch gone", which isn't
//!   retried). So each tokio runtime has its own clients, dropped when it
//!   stops. Production runs one runtime; tests run one per test.
//! - **Bounded:** at most [`MAX_PER_RUNTIME`] clients per runtime. Past that,
//!   the client handed out longest ago is dropped for each new one, and the
//!   log says so the first time (new keys come from operators editing
//!   settings, not from requests).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

/// What a shared client is: the helper that builds it and every setting
/// that helper takes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ClientKey {
    /// `providers::api_client`: a vendor API's client, with the operator's
    /// extra headers. `headers` is a digest of their names and values, so
    /// the key holds no copy of a header secret, but the client built for it
    /// keeps the headers as its default headers: after an operator changes a
    /// header, the client built for the old value stays until it is the one
    /// handed out longest ago at the bound, or its runtime stops.
    Api { timeout: Duration, headers: [u8; 32] },
    /// `ssrf::public_only_client_with_timeout`.
    PublicOnly { timeout: Duration },
    /// `ssrf::public_only_download_client`.
    PublicOnlyDownload { timeout: Duration, idle: Duration },
    /// `ssrf::no_redirect_download_client`.
    NoRedirectDownload { timeout: Duration, idle: Duration },
    /// `ssrf::public_only_upload_client`.
    PublicOnlyUpload { timeout: Duration },
    /// `ssrf::no_redirect_upload_client`.
    NoRedirectUpload { timeout: Duration },
}

/// The most clients one runtime keeps.
pub(crate) const MAX_PER_RUNTIME: usize = 64;

/// The runtime a client belongs to; `None` for one built outside any.
type RuntimeId = Option<tokio::runtime::Id>;

/// One runtime's clients, each with when it was last handed out.
#[derive(Default)]
struct RuntimeClients {
    clients: HashMap<ClientKey, (reqwest::Client, u64)>,
    /// Counts the hand-outs: a client's stamp is the count at its last one.
    handed: u64,
    /// Whether the log has said this runtime's set is full.
    reported_full: bool,
}

impl RuntimeClients {
    /// The client kept for `key`, stamped as just handed out.
    fn hand_out(&mut self, key: &ClientKey) -> Option<reqwest::Client> {
        self.handed += 1;
        let now = self.handed;
        self.clients.get_mut(key).map(|(client, at)| {
            *at = now;
            client.clone()
        })
    }

    /// Keep `built` for `key` (or the client another caller kept first) and
    /// hand it out. At the bound, the client handed out longest ago makes
    /// room; the second value is true the first time that happens.
    fn keep(&mut self, key: ClientKey, built: reqwest::Client) -> (reqwest::Client, bool) {
        let mut first_full = false;
        if self.clients.len() >= MAX_PER_RUNTIME && !self.clients.contains_key(&key) {
            let oldest = self.clients.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                self.clients.remove(&oldest);
            }
            first_full = !std::mem::replace(&mut self.reported_full, true);
        }
        self.handed += 1;
        let now = self.handed;
        let (client, at) = self.clients.entry(key).or_insert((built, now));
        *at = now;
        (client.clone(), first_full)
    }
}

type Clients = HashMap<RuntimeId, RuntimeClients>;

static CLIENTS: OnceLock<Mutex<Clients>> = OnceLock::new();

fn clients() -> MutexGuard<'static, Clients> {
    CLIENTS.get_or_init(Default::default).lock().unwrap_or_else(|p| p.into_inner())
}

/// The client for `key` on the current runtime: the one built for it before,
/// or `build()`'s, kept for the next caller. `build` runs outside the lock.
pub(crate) fn shared(key: ClientKey, build: impl FnOnce() -> reqwest::Client) -> reqwest::Client {
    let handle = tokio::runtime::Handle::try_current().ok();
    let runtime = handle.as_ref().map(|h| h.id());
    if let Some(client) = clients().get_mut(&runtime).and_then(|mine| mine.hand_out(&key)) {
        return client;
    }
    let built = build();
    #[cfg(test)]
    count_build(runtime, &key);
    let (client, first, full) = {
        let mut all = clients();
        let first = !all.contains_key(&runtime);
        let (client, full) = all.entry(runtime).or_default().keep(key, built);
        (client, first, full)
    };
    if full {
        // Registry review #7: past the bound each new client drops one, so a
        // deployment using more clients than this builds some again (and
        // opens new connections) as their turns come round.
        tracing::warn!(
            bound = MAX_PER_RUNTIME,
            "this runtime's shared HTTP clients reached their bound: for each new one, the one handed out longest ago is dropped"
        );
    }
    // The runtime's clients go when it stops: tokio drops this task then,
    // polled or not, and `forget` with it.
    if let (true, Some(handle)) = (first, handle) {
        let forget = Forget(runtime);
        handle.spawn(async move {
            let _forget = forget;
            std::future::pending::<()>().await;
        });
    }
    client
}

/// Drops its runtime's clients when dropped.
struct Forget(RuntimeId);

impl Drop for Forget {
    fn drop(&mut self) {
        clients().remove(&self.0);
    }
}

/// Test only: how many times each client was built, per runtime.
#[cfg(test)]
static BUILDS: OnceLock<Mutex<HashMap<(RuntimeId, ClientKey), usize>>> = OnceLock::new();

#[cfg(test)]
fn count_build(runtime: RuntimeId, key: &ClientKey) {
    *BUILDS.get_or_init(Default::default).lock().unwrap().entry((runtime, key.clone())).or_default() += 1;
}

/// Test only: how many times the client for `key` was built on the current
/// runtime.
#[cfg(test)]
pub(crate) fn builds(key: &ClientKey) -> usize {
    let runtime = tokio::runtime::Handle::try_current().ok().map(|h| h.id());
    BUILDS.get_or_init(Default::default).lock().unwrap().get(&(runtime, key.clone())).copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A runtime's clients are dropped when it stops, so the map holds only
    /// live runtimes' clients (tests start one runtime per test).
    #[test]
    fn a_stopped_runtimes_clients_are_dropped() {
        let key = ClientKey::PublicOnly { timeout: Duration::from_millis(4_321) };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let id = rt.handle().id();
        rt.block_on(async {
            shared(key.clone(), reqwest::Client::new);
        });
        assert!(clients().get(&Some(id)).is_some_and(|m| m.clients.contains_key(&key)), "kept while the runtime runs");
        drop(rt);
        assert!(!clients().contains_key(&Some(id)), "dropped with the runtime");
    }

    /// Past the per-runtime bound, the runtime's clients start again.
    #[tokio::test]
    async fn a_runtime_keeps_at_most_its_bound() {
        for i in 0..=(MAX_PER_RUNTIME as u64) {
            shared(ClientKey::PublicOnly { timeout: Duration::from_millis(10_000 + i) }, reqwest::Client::new);
        }
        let id = tokio::runtime::Handle::current().id();
        let n = clients().get(&Some(id)).map_or(0, |m| m.clients.len());
        assert!(n <= MAX_PER_RUNTIME, "{n} clients kept for one runtime");
    }

    /// What the log lines written while the guard lives said.
    #[derive(Clone, Default)]
    struct Logs(std::sync::Arc<Mutex<Vec<u8>>>);

    impl Logs {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl std::io::Write for Logs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
        type Writer = Logs;
        fn make_writer(&'a self) -> Logs {
            self.clone()
        }
    }

    /// Registry review #7: at the bound, the client used longest ago goes,
    /// not the runtime's whole set, so a deployment with more clients in use
    /// than the bound no longer goes back to a new client (and a new
    /// connection) on every call. The first time a runtime's set is full, the
    /// log says so, once.
    #[tokio::test]
    async fn the_bound_drops_only_the_client_used_longest_ago_and_says_so_once() {
        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt().with_writer(logs.clone()).with_ansi(false).with_max_level(tracing::Level::TRACE).finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let key = |i: u64| ClientKey::PublicOnly { timeout: Duration::from_millis(20_000 + i) };
        let bound = MAX_PER_RUNTIME as u64;
        for i in 0..bound {
            shared(key(i), reqwest::Client::new);
        }
        shared(key(0), reqwest::Client::new); // in use again: now the latest
        shared(key(bound), reqwest::Client::new); // one more than the bound
        shared(key(0), reqwest::Client::new);
        assert_eq!(builds(&key(0)), 1, "a client in use was dropped at the bound");
        shared(key(1), reqwest::Client::new);
        assert_eq!(builds(&key(1)), 2, "the client used longest ago was the one dropped");
        shared(key(bound + 1), reqwest::Client::new);
        let id = tokio::runtime::Handle::current().id();
        let n = clients().get(&Some(id)).map_or(0, |m| m.clients.len());
        assert!(n <= MAX_PER_RUNTIME, "{n} clients kept for one runtime");
        let text = logs.text();
        assert_eq!(text.matches("shared HTTP clients").count(), 1, "the full set is reported once: {text:?}");
    }
}
