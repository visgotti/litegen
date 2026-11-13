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
//!   the runtime's clients are dropped and built again as they are asked for
//!   (new keys come from operators editing settings, not from requests).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

/// What a shared client is: the helper that builds it and every setting
/// that helper takes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ClientKey {
    /// `providers::api_client`: a vendor API's client, with the operator's
    /// extra headers. `headers` is a digest of their names and values, so the
    /// map holds no copy of a header secret.
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

type Clients = HashMap<RuntimeId, HashMap<ClientKey, reqwest::Client>>;

static CLIENTS: OnceLock<Mutex<Clients>> = OnceLock::new();

fn clients() -> MutexGuard<'static, Clients> {
    CLIENTS.get_or_init(Default::default).lock().unwrap_or_else(|p| p.into_inner())
}

/// The client for `key` on the current runtime: the one built for it before,
/// or `build()`'s, kept for the next caller. `build` runs outside the lock.
pub(crate) fn shared(key: ClientKey, build: impl FnOnce() -> reqwest::Client) -> reqwest::Client {
    let handle = tokio::runtime::Handle::try_current().ok();
    let runtime = handle.as_ref().map(|h| h.id());
    if let Some(client) = clients().get(&runtime).and_then(|mine| mine.get(&key)) {
        return client.clone();
    }
    let built = build();
    #[cfg(test)]
    count_build(runtime, &key);
    let (client, first) = {
        let mut all = clients();
        let first = !all.contains_key(&runtime);
        let mine = all.entry(runtime).or_default();
        if mine.len() >= MAX_PER_RUNTIME && !mine.contains_key(&key) {
            mine.clear();
        }
        (mine.entry(key).or_insert(built).clone(), first)
    };
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
        assert!(clients().get(&Some(id)).is_some_and(|m| m.contains_key(&key)), "kept while the runtime runs");
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
        let n = clients().get(&Some(id)).map_or(0, HashMap::len);
        assert!(n <= MAX_PER_RUNTIME, "{n} clients kept for one runtime");
    }
}
