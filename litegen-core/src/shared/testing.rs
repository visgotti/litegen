//! Helpers for tests that need Redis: the lib tests and the integration tests
//! share them. Production never calls this module.
//!
//! The Redis tests run against a throwaway container named by
//! `LITEGEN_REDIS_TEST_URL`, and skip themselves (with a line saying so)
//! when it is unset. Each test uses a prefix of its own, [`test_prefix`], so
//! tests do not see each other's keys.

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::SharedRedis;
use crate::api::middleware::rate_limit::RateLimiter;
use crate::config::connections::RedisSettings;

/// `LITEGEN_REDIS_TEST_URL`, or `None` (after printing a skip line) when it is
/// unset or empty.
pub fn test_url() -> Option<String> {
    match std::env::var("LITEGEN_REDIS_TEST_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!("skipping: LITEGEN_REDIS_TEST_URL is not set (a throwaway Redis, for example redis://127.0.0.1:56379)");
            None
        }
    }
}

/// A key prefix no other test uses: `litegen-test:<uuid>:`.
pub fn test_prefix() -> String {
    format!("litegen-test:{}:", uuid::Uuid::new_v4())
}

/// The socket address in a Redis URL, for [`CutProxy::start`].
pub fn addr_of(url: &str) -> SocketAddr {
    let parsed = redis::parse_redis_url(url).expect("a Redis URL");
    let host = parsed.host_str().expect("a host in the Redis URL").to_string();
    let port = parsed.port().unwrap_or(6379);
    (host.as_str(), port)
        .to_socket_addrs()
        .expect("the host resolves")
        .next()
        .expect("at least one address")
}

/// Two clients on one fresh prefix: two replicas sharing a Redis. `None`
/// (after the skip line) without `LITEGEN_REDIS_TEST_URL`.
pub async fn two_shared() -> Option<(Arc<SharedRedis>, Arc<SharedRedis>)> {
    let url = test_url()?;
    let prefix = test_prefix();
    let settings = RedisSettings::from_url(&url);
    let a = SharedRedis::connect(&settings, &prefix).await.expect("the test Redis URL is usable");
    let b = SharedRedis::connect(&settings, &prefix).await.expect("the test Redis URL is usable");
    Some((a, b))
}

/// Two replicas' limiters on one fresh prefix: each has its own
/// [`RateLimiter`], and the shared buckets and slots are the ones they have in
/// common. `None` (after the skip line) without `LITEGEN_REDIS_TEST_URL`.
pub async fn two_shared_limiters() -> Option<(Arc<RateLimiter>, Arc<RateLimiter>)> {
    let (a, b) = two_shared().await?;
    Some((Arc::new(RateLimiter::with_shared(a)), Arc::new(RateLimiter::with_shared(b))))
}

/// A client whose Redis is not there (`127.0.0.1:1` refuses at once), so it
/// needs no container. `fail_closed` is its setting of that name.
pub async fn dead_redis(fail_closed: bool) -> Arc<SharedRedis> {
    let mut settings = RedisSettings::from_url("redis://127.0.0.1:1");
    settings.fail_closed = fail_closed;
    SharedRedis::connect(&settings, &test_prefix()).await.expect("a usable URL")
}

/// `SCRIPT FLUSH`: forget every loaded script, as a Redis restart or a
/// failover does.
pub async fn flush_scripts(shared: &SharedRedis) {
    shared
        .run(|mut c| async move { redis::cmd("SCRIPT").arg("FLUSH").query_async::<()>(&mut c).await })
        .await
        .expect("SCRIPT FLUSH");
}

/// Delete every key under this client's prefix.
pub async fn flush_keys(shared: &SharedRedis) {
    let pattern = format!("{}*", shared.key(&[]));
    shared
        .run(|mut c| async move {
            let mut cursor = 0u64;
            loop {
                let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(200)
                    .query_async(&mut c)
                    .await?;
                if !keys.is_empty() {
                    redis::cmd("DEL").arg(keys).query_async::<()>(&mut c).await?;
                }
                if next == 0 {
                    return Ok(());
                }
                cursor = next;
            }
        })
        .await
        .expect("flush this prefix's keys");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Forward,
    Cut,
    Hang,
    Deaf,
}

/// A TCP forwarder in front of a Redis, for making it unreachable on demand:
/// point a client at [`CutProxy::url`] instead of the real one.
///
/// - **`cut`:** drops the connections and refuses new ones (a new connection
///   is accepted and closed at once).
/// - **`hang`:** a connection stays open and carries nothing, either way: a
///   Redis that has stopped answering.
/// - **`deafen`:** forwards what clients send and throws away what Redis
///   answers: Redis runs the commands, and the replies are lost on the way
///   back. Open connections go deaf too.
/// - **`restore`:** forwards again. Connections cut, hung or deaf stay dead, so
///   a client has to connect afresh.
pub struct CutProxy {
    addr: SocketAddr,
    mode: watch::Sender<Mode>,
    accepting: JoinHandle<()>,
}

impl CutProxy {
    /// Listen on a free local port and forward to `upstream`.
    pub async fn start(upstream: SocketAddr) -> CutProxy {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a local port");
        let addr = listener.local_addr().expect("the bound address");
        let (mode, mut rx) = watch::channel(Mode::Forward);
        let accepting = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                // Mark the mode in force as seen first: a clone keeps this
                // receiver's version, and one that had not seen the latest
                // change would wake at once and close the connection.
                drop(rx.borrow_and_update());
                tokio::spawn(serve(client, upstream, rx.clone()));
            }
        });
        CutProxy { addr, mode, accepting }
    }

    /// Where clients connect.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `redis://` plus [`Self::addr`].
    pub fn url(&self) -> String {
        format!("redis://{}", self.addr)
    }

    /// Drop the connections and refuse new ones.
    pub fn cut(&self) {
        self.set(Mode::Cut);
    }

    /// Accept connections and forward nothing.
    pub fn hang(&self) {
        self.set(Mode::Hang);
    }

    /// Forward what clients send, and drop every reply.
    pub fn deafen(&self) {
        self.set(Mode::Deaf);
    }

    /// Forward again.
    pub fn restore(&self) {
        self.set(Mode::Forward);
    }

    fn set(&self, to: Mode) {
        // Not a send when nothing changes: every connection is watching.
        self.mode.send_if_modified(|mode| {
            let changed = *mode != to;
            *mode = to;
            changed
        });
    }
}

impl Drop for CutProxy {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

/// One client connection, until the mode changes from the one it started in.
async fn serve(mut client: TcpStream, upstream: SocketAddr, mut mode: watch::Receiver<Mode>) {
    let started = *mode.borrow();
    match started {
        Mode::Cut => return,
        Mode::Hang => return hold(mode).await,
        Mode::Forward | Mode::Deaf => {}
    }
    let Ok(mut upstream) = TcpStream::connect(upstream).await else { return };
    if started == Mode::Deaf {
        return deaf(client, upstream, mode).await;
    }
    let next = tokio::select! {
        _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => return,
        changed = mode.changed() => match changed {
            Ok(()) => *mode.borrow(),
            Err(_) => return,
        },
    };
    match next {
        // A hang keeps the connection open, silent; deaf keeps forwarding
        // what the client sends and drops the replies.
        Mode::Hang => hold(mode).await,
        Mode::Deaf => deaf(client, upstream, mode).await,
        // A cut ends the connection.
        Mode::Cut | Mode::Forward => {}
    }
}

/// Forward what the client sends to Redis and throw away what Redis answers,
/// until the mode changes; then the connection ends.
async fn deaf(client: TcpStream, upstream: TcpStream, mut mode: watch::Receiver<Mode>) {
    // The client's write half is kept, unused, so its side stays open.
    let (mut from_client, _to_client) = client.into_split();
    let (mut from_redis, mut to_redis) = upstream.into_split();
    let mut nowhere = tokio::io::sink();
    tokio::select! {
        _ = tokio::io::copy(&mut from_client, &mut to_redis) => {}
        _ = tokio::io::copy(&mut from_redis, &mut nowhere) => {}
        _ = mode.changed() => {}
    }
}

/// Keep a connection open, carrying nothing, until the mode changes.
async fn hold(mut mode: watch::Receiver<Mode>) {
    let _ = mode.changed().await;
}
