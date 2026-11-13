//! State that several litegen replicas share through Redis (`docs/scaling.md`).
//!
//! This module is the one gateway to Redis. Every shared limit, the shared
//! cache and config sync go through [`SharedRedis::run`], which keeps them all
//! on the same rules:
//!
//! - **Never slow, never down.** One call, connecting included, costs at most
//!   [`CALL_TIMEOUT`]. After enough failures in a row [`RedisBreaker`] opens
//!   and calls fail at once, without touching the network, until a probe
//!   shows Redis is back. A dead Redis therefore adds no latency to a request
//!   once the breaker has opened. An answer that takes the slow-call time
//!   (`LITEGEN_REDIS_SLOW_CALL_MS`, 250 ms by default) or longer counts as a
//!   failure too, so a Redis that answers slowly opens the breaker as a dead
//!   one does.
//! - **Starts without Redis.** [`SharedRedis::connect`] only checks the
//!   settings. The connection is made on the first call, and made again after
//!   it drops.
//! - **No secrets.** The URL may hold a password. It is never formatted into
//!   an error or a log line: an error carries what Redis or the network said,
//!   and the log names the target (`host:port`) only.
//!
//! The engine never names this module (`.claude/rules/engine-boundary.md`).

use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use redis::aio::{ConnectionManager, ConnectionManagerConfig, PubSub};
use redis::{
    Client, ConnectionAddr, ErrorKind, IntoConnectionInfo, RedisError, RedisResult, ServerErrorKind, TlsCertificates,
};

use crate::config::connections::RedisSettings;
use crate::config::AppConfig;

pub mod breaker;
pub mod cache_tier;
pub mod events;
pub mod provider_breaker;
pub mod rate_limit;
pub mod slots;
pub mod throttle;
// Helpers for the tests of this module and of the modules built on it, and for
// the integration tests. Always compiled so they can share it; production
// never calls it.
#[doc(hidden)]
pub mod testing;

pub use breaker::RedisBreaker;
pub use throttle::Throttle;

/// The most one Redis call may take, from asking for a connection to the
/// answer. Longer than that is a failure, as far as the breaker is concerned.
pub const CALL_TIMEOUT: Duration = Duration::from_millis(500);

/// A call that gets its turn to connect with less than this left does not
/// try: a connection it cannot finish is not worth starting, and the call has
/// spent its time waiting, not reaching Redis.
const MIN_CONNECT_BUDGET: Duration = Duration::from_millis(50);

/// Why a call did not return a value. Neither variant ever holds a password.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SharedError {
    /// The breaker is open: the call failed at once, without touching the
    /// network.
    #[error("Redis is unavailable (its circuit breaker is open)")]
    Unavailable,
    /// The call went out and failed: a timeout, a connection error, or an
    /// error reply. The text is Redis's or the network's, never the URL's.
    #[error("Redis error: {0}")]
    Redis(String),
}

/// A client for the Redis the replicas share. See the module docs.
pub struct SharedRedis {
    client: Client,
    /// The connection in use, and which one it is (see [`Self::discard`]).
    slot: Mutex<Slot>,
    /// Held while a connection is made, so a burst of calls on an empty slot
    /// makes one connection, not one each.
    connecting: tokio::sync::Mutex<()>,
    breaker: RedisBreaker,
    /// An answer this slow or slower counts as a breaker failure; zero, never
    /// (see [`Self::answered`]).
    slow_call: Duration,
    /// When the last slow answer that counted came back.
    slow_counted_at: Mutex<Option<Instant>>,
    /// Keeps the slow-answer warning to one line a minute.
    slow_warning: Throttle,
    key_prefix: String,
    fail_closed: bool,
    /// This client's id on the replicas' event channel (see [`Self::id`]).
    id: String,
}

#[derive(Default)]
struct Slot {
    /// Counts the connections made, from 1. A call that fails discards the
    /// connection it used, not a newer one that another call made meanwhile.
    generation: u64,
    manager: Option<ConnectionManager>,
}

/// Never the client: its connection info holds the password.
impl std::fmt::Debug for SharedRedis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedRedis")
            .field("key_prefix", &self.key_prefix)
            .field("fail_closed", &self.fail_closed)
            .finish_non_exhaustive()
    }
}

/// How a call failed, for the breaker.
enum CallError {
    /// The call spent its time queued behind another call's connection, and
    /// never reached Redis. The breaker hears nothing of it: the call that
    /// made the connection reports for all of them.
    Queued,
    /// No connection could be made (any error counts: Redis is not usable).
    Connect(RedisError),
    /// The command went out. An error reply means Redis is there, unless it
    /// says the node can't serve now ([`cannot_use`]).
    Command(RedisError),
}

impl SharedRedis {
    /// A client for `settings`, with every key under `key_prefix` (`litegen:`).
    /// Fails only on settings that cannot work: a URL that does not parse, or a
    /// CA file that cannot be read. It never fails because Redis is
    /// unreachable: the connection is made on the first call, and again after
    /// it drops.
    pub async fn connect(settings: &RedisSettings, key_prefix: &str) -> Result<Arc<SharedRedis>, SharedError> {
        let client = build_client(settings).await?;
        Ok(Arc::new(SharedRedis {
            client,
            slot: Mutex::new(Slot::default()),
            connecting: tokio::sync::Mutex::new(()),
            // Never under the time one call can take: a call that started before
            // the breaker opened could otherwise settle after it half-opens, and
            // be taken for the probe.
            breaker: RedisBreaker::new(settings.breaker_failure_threshold, settings.breaker_recovery.max(CALL_TIMEOUT)),
            slow_call: settings.slow_call,
            slow_counted_at: Mutex::new(None),
            slow_warning: Throttle::new(),
            key_prefix: key_prefix.to_string(),
            fail_closed: settings.fail_closed,
            // Random, never made from the URL (which may hold a password).
            id: uuid::Uuid::new_v4().to_string(),
        }))
    }

    /// This client's id: a UUID made when it was created, which its events
    /// carry as `from` (`events`), so a replica can tell its own events from
    /// another's. A litegen process has one client for the one replica it is,
    /// so in production this is the replica's id; a test that runs two
    /// replicas in one process gives each its own client, and so its own id.
    /// (The process-wide [`replica_id`] is another thing: the poller's
    /// claimant.)
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The prefix, then `parts` joined with `:`: `key(&["rl", "abc"])` is
    /// `litegen:rl:abc`.
    pub fn key(&self, parts: &[&str]) -> String {
        format!("{}{}", self.key_prefix, parts.join(":"))
    }

    /// Whether limits refuse requests, rather than fall back to this
    /// replica's own counters, while Redis cannot be reached.
    pub fn fail_closed(&self) -> bool {
        self.fail_closed
    }

    /// Run `op` against the shared connection. With the breaker open this is
    /// `Err(Unavailable)` at once, off the network. Otherwise `op` and the
    /// connection it needs share [`CALL_TIMEOUT`], and the result goes to the
    /// breaker: a timeout, a connection-level error, or a reply saying the
    /// node can't serve (READONLY, LOADING, MASTERDOWN: see [`cannot_use`])
    /// counts as a failure and drops the connection, so the next call
    /// connects afresh. The calls in flight on a connection that is lost count
    /// as one failure, not one each. Any other error reply (WRONGTYPE, a
    /// script error) is returned but counts as a success, because Redis
    /// answered, unless Redis took the slow-call time or longer to answer
    /// ([`Self::answered`]).
    pub async fn run<T, F, Fut>(&self, op: F) -> Result<T, SharedError>
    where
        F: FnOnce(ConnectionManager) -> Fut,
        Fut: Future<Output = RedisResult<T>>,
    {
        let started = Instant::now();
        if !self.breaker.allow(started) {
            return Err(SharedError::Unavailable);
        }
        // How far the call got, for the breaker if it times out: which
        // connection it used (to discard it), and whether it reached Redis.
        let used = AtomicU64::new(0);
        let reached = AtomicBool::new(false);
        // When the command went out, with the connection in hand: what Redis
        // took to answer. Connecting, and waiting behind another call's
        // connect, are not Redis being slow to answer.
        let sent = OnceLock::new();
        let outcome = tokio::time::timeout(CALL_TIMEOUT, async {
            let (generation, connection) = self.connection(started + CALL_TIMEOUT, &reached).await?;
            used.store(generation, Ordering::Relaxed);
            let _ = sent.set(Instant::now());
            op(connection).await.map_err(CallError::Command)
        })
        .await;
        self.settle(outcome, used.load(Ordering::Relaxed), reached.load(Ordering::Relaxed), sent.get().copied())
    }

    /// Check that Redis answers, through the breaker like any other call.
    pub async fn ping(&self) -> Result<(), SharedError> {
        self.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await })
            .await
            .map(|_| ())
    }

    /// Publish `message` on `channel`. The channel is used as written: the
    /// caller builds it with [`Self::key`].
    pub async fn publish(&self, channel: &str, message: &str) -> Result<(), SharedError> {
        self.run(|mut c| async move {
            redis::cmd("PUBLISH").arg(channel).arg(message).query_async::<i64>(&mut c).await
        })
        .await
        .map(|_| ())
    }

    /// A connection of its own, subscribed to `channel`: messages arrive on
    /// `on_message()` until it drops, and the caller subscribes again then.
    /// Opening it goes through the breaker and [`CALL_TIMEOUT`] like a call.
    pub async fn subscribe(&self, channel: &str) -> Result<PubSub, SharedError> {
        if !self.breaker.allow(Instant::now()) {
            return Err(SharedError::Unavailable);
        }
        let outcome = tokio::time::timeout(CALL_TIMEOUT, async {
            let mut pubsub = self.client.get_async_pubsub().await.map_err(CallError::Connect)?;
            pubsub.subscribe(channel).await.map_err(CallError::Command)?;
            Ok(pubsub)
        })
        .await;
        // The connection is the subscriber's own, not the shared one, and the
        // call is its own attempt at Redis. Its time includes connecting, so
        // it is not checked for a slow answer.
        self.settle(outcome, 0, true, None)
    }

    /// The shared connection, made now if there is none. `reached` is set
    /// once the call has used or tried to make a connection, as opposed to
    /// only waiting its turn behind another call that was making one: only a
    /// call that reached Redis may tell the breaker how it went.
    async fn connection(&self, deadline: Instant, reached: &AtomicBool) -> Result<(u64, ConnectionManager), CallError> {
        if let Some(found) = self.current() {
            reached.store(true, Ordering::Relaxed);
            return Ok(found);
        }
        let _connecting = self.connecting.lock().await;
        // Another call may have connected while this one waited its turn.
        if let Some(found) = self.current() {
            reached.store(true, Ordering::Relaxed);
            return Ok(found);
        }
        if deadline.saturating_duration_since(Instant::now()) < MIN_CONNECT_BUDGET {
            return Err(CallError::Queued);
        }
        reached.store(true, Ordering::Relaxed);
        // No retries of its own: a failed connection is the breaker's to
        // count, and the next call's to try again. When a command later fails
        // with a connection error, the manager starts one background attempt
        // to reconnect before `settle` discards it; that is bounded, since
        // once the breaker opens no call reaches a manager to start another.
        let config = ConnectionManagerConfig::new()
            .set_number_of_retries(0)
            .set_connection_timeout(Some(CALL_TIMEOUT))
            .set_response_timeout(Some(CALL_TIMEOUT));
        let manager = ConnectionManager::new_with_config(self.client.clone(), config).await.map_err(CallError::Connect)?;
        let mut slot = self.slot();
        slot.generation += 1;
        slot.manager = Some(manager.clone());
        Ok((slot.generation, manager))
    }

    fn current(&self) -> Option<(u64, ConnectionManager)> {
        let slot = self.slot();
        slot.manager.as_ref().map(|m| (slot.generation, m.clone()))
    }

    /// Forget connection number `generation`, which a call found no good, and
    /// say whether that call is the one to count the loss on the breaker.
    ///
    /// Every call in flight on a connection fails when it goes. They are one
    /// loss, not one failure each: counted one by one, a busy replica's burst
    /// would open the breaker at once (the reason queued callers record
    /// nothing either). So only the call that takes the connection out of the
    /// slot counts; the others find it gone, or replaced by a newer one, and
    /// count nothing. Generation 0 is a connection of the call's own (the one
    /// it was making, or a subscriber's), which no other call shares: always
    /// counted.
    fn lose(&self, generation: u64) -> bool {
        if generation == 0 {
            return true;
        }
        let mut slot = self.slot();
        if slot.generation == generation && slot.manager.is_some() {
            slot.manager = None;
            true
        } else {
            false
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Tell the breaker how the call ended, drop the connection if it is no
    /// good, and turn the outcome into the caller's result. A call that never
    /// `reached` Redis, only waited behind another one, tells the breaker
    /// nothing. `sent` is when the command went out, if it did.
    fn settle<T>(
        &self,
        outcome: Result<Result<T, CallError>, tokio::time::error::Elapsed>,
        generation: u64,
        reached: bool,
        sent: Option<Instant>,
    ) -> Result<T, SharedError> {
        let now = Instant::now();
        match outcome {
            Ok(Ok(value)) => {
                self.answered(sent, now);
                Ok(value)
            }
            Ok(Err(CallError::Queued)) => Err(queued_out()),
            Err(_) if !reached => Err(queued_out()),
            Ok(Err(CallError::Connect(e))) => {
                self.breaker.record(false, now);
                Err(describe(&e))
            }
            Ok(Err(CallError::Command(e))) => {
                if !cannot_use(&e) {
                    self.answered(sent, now);
                } else if self.lose(generation) {
                    self.breaker.record(false, now);
                }
                Err(describe(&e))
            }
            Err(_) => {
                if self.lose(generation) {
                    self.breaker.record(false, now);
                }
                Err(SharedError::Redis(format!("timed out after {} ms", CALL_TIMEOUT.as_millis())))
            }
        }
    }

    /// Tell the breaker about an answer Redis gave (a value or an error
    /// reply) to the command that went out at `sent`: a success, unless Redis
    /// was slow to give it.
    ///
    /// An answer that took [`Self::slow_call`] or longer counts as a failure:
    /// it cost the request nearly what a timeout does, and a Redis that
    /// answers every call just inside the timeout would otherwise add its
    /// latency to every call without ever opening the breaker. It counts once
    /// per stall, though: only if its command went out after the last slow
    /// answer that counted came back. One stall (a fork, a network hiccup)
    /// delays every call in flight, and counted one by one those would open
    /// the breaker on every busy replica at once (as with a lost connection,
    /// [`Self::lose`]). An overlapping slow answer records nothing, neither a
    /// failure nor a success. A slow answer is still the caller's, and the
    /// connection stays.
    fn answered(&self, sent: Option<Instant>, now: Instant) {
        let slow = sent.filter(|sent| !self.slow_call.is_zero() && now.saturating_duration_since(*sent) >= self.slow_call);
        let Some(sent) = slow else {
            self.breaker.record(true, now);
            return;
        };
        {
            let mut last = self.slow_counted_at.lock().unwrap_or_else(|p| p.into_inner());
            if last.is_some_and(|counted| sent <= counted) {
                return;
            }
            *last = Some(now);
        }
        self.breaker.record(false, now);
        if self.slow_warning.due(now) {
            tracing::warn!(
                took_ms = u64::try_from(now.saturating_duration_since(sent).as_millis()).unwrap_or(u64::MAX),
                slow_call_ms = u64::try_from(self.slow_call.as_millis()).unwrap_or(u64::MAX),
                "Redis answered slowly: an answer that takes LITEGEN_REDIS_SLOW_CALL_MS or longer counts as a \
                 failure, and enough in a row open the Redis breaker (at most one such warning a minute)"
            );
        }
    }
}

/// What a call that spent its time queued behind another call's connection
/// is told.
fn queued_out() -> SharedError {
    SharedError::Redis(format!(
        "timed out after {} ms waiting for a connection that another call was making",
        CALL_TIMEOUT.as_millis()
    ))
}

/// Whether `e` says this connection can't be used: either the connection is no
/// good, or the node behind it can't serve now. Either way the call counts as
/// a failure and the connection goes, so the next call connects afresh. As
/// opposed to Redis having answered with an error (WRONGTYPE, a script error),
/// which says nothing about Redis being there: a bug in one script must not
/// switch every replica to its own counters.
///
/// The node can't serve when it answers READONLY (a primary demoted by a
/// failover, which keeps its clients' connections: a new connection to a DNS
/// name reaches the new primary), LOADING (a restarted Redis reading its data)
/// or MASTERDOWN (a replica cut off from its primary, which answers that to
/// every command, PING included). No script bug makes those three replies.
/// OOM and MISCONF stay answers: a new connection can't help a full or
/// misconfigured node.
///
/// A write inside a script gets READONLY too, but Redis 5 and 6 wrap it in
/// `ERR`: `ERR Error running script (call to f_<sha>): @user_script:1:
/// @user_script: 1: -READONLY You can't write against a read only replica.`
/// (5.0 adds three spaces at the end). Redis 7 and Valkey send the READONLY
/// itself. litegen's frequent writes are scripts (a rate-limit take, a slot's
/// acquire and refresh, a cache put, the provider breaker's writes), so that
/// `ERR` counts too. Only Redis's own text is matched: an `ERR` that says it
/// ran a script and carries the replica error.
fn cannot_use(e: &RedisError) -> bool {
    e.is_io_error()
        || e.is_connection_dropped()
        || e.is_connection_refusal()
        || e.is_timeout()
        // A reply that cannot be parsed or a refused login also leave the
        // connection unusable.
        || e.is_unrecoverable_error()
        || matches!(
            e.kind(),
            ErrorKind::Server(ServerErrorKind::ReadOnly | ServerErrorKind::BusyLoading | ServerErrorKind::MasterDown)
        )
        || (matches!(e.kind(), ErrorKind::Server(ServerErrorKind::ResponseError))
            && e.detail().is_some_and(|d| d.starts_with("Error running script") && d.contains("-READONLY ")))
}

/// An error for the caller. Redis's own text (a kind and a description from
/// the server or the network) never holds the URL, so never a password.
fn describe(e: &RedisError) -> SharedError {
    SharedError::Redis(e.to_string())
}

/// The id of this process among the replicas: made once, so every poll claim
/// it writes can be told from another replica's. Events carry the publishing
/// client's own id instead ([`SharedRedis::id`]).
pub fn replica_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

/// The client for `settings`: the TLS rules applied, nothing connected.
async fn build_client(settings: &RedisSettings) -> Result<Client, SharedError> {
    let mut info = settings.url().into_connection_info().map_err(|e| describe(&e))?;
    let tls = matches!(info.addr(), ConnectionAddr::TcpTls { .. });
    if tls {
        ensure_crypto_provider();
    }
    // `LITEGEN_REDIS_SSL_CERT_REQS=none`: the same as `#insecure` on the URL.
    if !settings.verify_certificates {
        if let ConnectionAddr::TcpTls { host, port, tls_params, .. } = info.addr().clone() {
            info = info.set_addr(ConnectionAddr::TcpTls { host, port, insecure: true, tls_params });
        }
    }
    match (&settings.ca_certs, tls) {
        (Some(path), true) => {
            let path = path.clone();
            let bundle = tokio::task::spawn_blocking(move || ca_bundle(&path))
                .await
                .map_err(|e| SharedError::Redis(format!("loading the CA certificates failed: {e}")))??;
            Client::build_with_tls(info, TlsCertificates { client_tls: None, root_cert: Some(bundle) })
                .map_err(|e| describe(&e))
        }
        // The CA file means something for TLS only.
        _ => Client::open(info).map_err(|e| describe(&e)),
    }
}

/// rustls needs a process-wide crypto provider before it builds a client
/// config, and says so by panicking when it has two to choose from. litegen's
/// dependencies enable both `ring` (sqlx) and `aws-lc-rs`, so install the
/// second, which litegen uses elsewhere. Another provider installed first stays.
fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // Err means a provider was installed in the meantime: that one stays.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}

const PEM_BEGIN: &str = "-----BEGIN CERTIFICATE-----";

/// The PEM bundle `redis` takes as its root certificates: the system's roots,
/// then the certificates in the file at `path`. `redis` trusts only what it is
/// given once it is given any, so the system's roots go in too. That matches
/// the documented `LITEGEN_REDIS_SSL_CA_CERTS`, "the system's roots plus" the
/// file (LiteLLM's behavior).
fn ca_bundle(path: &Path) -> Result<Vec<u8>, SharedError> {
    let file = std::fs::read_to_string(path)
        .map_err(|e| SharedError::Redis(format!("cannot read the CA file {}: {e}", path.display())))?;
    if !file.contains(PEM_BEGIN) {
        return Err(SharedError::Redis(format!(
            "the CA file {} holds no PEM certificate (-----BEGIN CERTIFICATE-----)",
            path.display()
        )));
    }
    let system = rustls_native_certs::load_native_certs();
    if !system.errors.is_empty() {
        // The file is what the operator asked for; a system root that did not
        // load should not stop it. This tolerance covers the bundle only: redis
        // loads the native roots itself on every TLS connect and fails on the
        // first one that does not load (`create_rustls_config`).
        tracing::warn!(
            skipped = system.errors.len(),
            loaded = system.certs.len(),
            "some of the system's root certificates could not be loaded for Redis TLS"
        );
    }
    Ok(pem_bundle(system.certs.iter().map(|c| c.as_ref()), &file).into_bytes())
}

/// `roots` (DER) as PEM, followed by `extra_pem` as it is.
fn pem_bundle<'a>(roots: impl IntoIterator<Item = &'a [u8]>, extra_pem: &str) -> String {
    use base64::Engine as _;
    let mut bundle = String::new();
    for der in roots {
        let encoded = base64::engine::general_purpose::STANDARD.encode(der);
        bundle.push_str(PEM_BEGIN);
        bundle.push('\n');
        // PEM lines are 64 characters; base64 text is ASCII, so cut by bytes.
        for line in encoded.as_bytes().chunks(64) {
            bundle.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
            bundle.push('\n');
        }
        bundle.push_str("-----END CERTIFICATE-----\n");
    }
    bundle.push_str(extra_pem);
    if !bundle.ends_with('\n') {
        bundle.push('\n');
    }
    bundle
}

/// One line for the startup log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupNote {
    /// A warning (something to change) rather than information.
    pub warn: bool,
    pub text: String,
}

/// What `config` says about running several replicas: information when Redis
/// is off, and a warning for each setting that would defeat Redis when it is
/// on. litegen can't count its replicas, so a single replica gets the same
/// notes as a fleet.
pub fn startup_notes(config: &AppConfig) -> Vec<StartupNote> {
    if config.redis.is_none() {
        return vec![StartupNote {
            warn: false,
            text: "Redis is not set: rate limits, parallel slots, the circuit breaker and the cache are per \
                   process. LITEGEN_REDIS_URL shares them across replicas."
                .to_string(),
        }];
    }
    let mut notes = Vec::new();
    // The same test as `db::connect`: everything but Postgres is SQLite.
    let postgres = config.database_url.starts_with("postgres://") || config.database_url.starts_with("postgresql://");
    if !postgres {
        notes.push(StartupNote {
            warn: true,
            text: "Redis is set, but the database is SQLite, which only one replica can use: set \
                   LITEGEN_DATABASE_URL (or the LITEGEN_DATABASE_* parts) to a Postgres database every replica \
                   shares."
                .to_string(),
        });
    }
    if config.image_storage.backend != "s3" {
        notes.push(StartupNote {
            warn: true,
            text: "Redis is set, but stored files are on local disk, where the other replicas can't read \
                   them: set image_storage.backend to s3 (LITEGEN__IMAGE_STORAGE__BACKEND=s3, with the \
                   LITEGEN_S3_* settings) so every replica serves the same files."
                .to_string(),
        });
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::testing::{addr_of, dead_redis, test_prefix, test_url, two_shared, CutProxy};
    use super::*;

    /// A self-signed CA certificate, for the PEM files in these tests.
    const TEST_CA: &str = "-----BEGIN CERTIFICATE-----
MIIBijCCATGgAwIBAgIUUspePeu/bkZlrWjGe9EVCzLxvz4wCgYIKoZIzj0EAwIw
GjEYMBYGA1UEAwwPbGl0ZWdlbiB0ZXN0IGNhMCAXDTI2MTEwOTA4NDg0MVoYDzIx
MjYxMDE2MDg0ODQxWjAaMRgwFgYDVQQDDA9saXRlZ2VuIHRlc3QgY2EwWTATBgcq
hkjOPQIBBggqhkjOPQMBBwNCAARZOJA9E7e226+3pPngTaObj13xgMCLaWNlrU6z
9kmQQUwQYgQxhgOW/K+UNpW6wt32P461McMAKbn1iFVb8+a6o1MwUTAdBgNVHQ4E
FgQUS9+bni0I5RjOhmrhGYLS9WmazGcwHwYDVR0jBBgwFoAUS9+bni0I5RjOhmrh
GYLS9WmazGcwDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNHADBEAiBxMLgq
iqpV3oUwNG5PuZeKHyYAzCICPaAVvzqLlCTVOAIgOpf/HuTPb1shoX8I5vrfsnij
0nxS40dzIt+vtwMpUzA=
-----END CERTIFICATE-----
";

    async fn ping(s: &SharedRedis) -> Result<String, SharedError> {
        s.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await }).await
    }

    async fn client_id(s: &SharedRedis) -> Result<i64, SharedError> {
        s.run(|mut c| async move { redis::cmd("CLIENT").arg("ID").query_async::<i64>(&mut c).await }).await
    }

    #[test]
    fn notes_warn_on_sqlite_or_local_storage_with_redis_and_inform_without() {
        let mut c = AppConfig::default();
        assert_eq!(startup_notes(&c).iter().filter(|n| !n.warn).count(), 1, "the per-process info line");
        c.redis = Some(RedisSettings::from_url("redis://h:6379"));
        let notes = startup_notes(&c);
        assert!(notes.iter().any(|n| n.warn && n.text.contains("SQLite")));
        assert!(notes.iter().any(|n| n.warn && n.text.contains("local")));
    }

    #[test]
    fn the_notes_name_what_to_change_and_what_redis_shares() {
        let mut c = AppConfig::default();
        let [info] = startup_notes(&c).try_into().expect("one note without Redis");
        assert!(!info.warn);
        for shared in ["rate limits", "parallel slots", "circuit breaker", "cache", "per process", "LITEGEN_REDIS_URL"] {
            assert!(info.text.contains(shared), "{shared}: {}", info.text);
        }
        c.redis = Some(RedisSettings::from_url("redis://h:6379"));
        let notes = startup_notes(&c);
        assert!(notes.iter().all(|n| n.warn), "with Redis there is no info line");
        assert!(notes.iter().any(|n| n.text.contains("LITEGEN_DATABASE_URL")));
        assert!(notes.iter().any(|n| n.text.contains("image_storage.backend")));
    }

    #[test]
    fn redis_on_postgres_and_s3_has_nothing_to_warn_about() {
        let mut c = AppConfig {
            redis: Some(RedisSettings::from_url("redis://h:6379")),
            database_url: "postgres://u@db/litegen".to_string(),
            image_storage: crate::config::ImageStorageConfig { backend: "s3".to_string(), ..Default::default() },
            ..Default::default()
        };
        assert!(startup_notes(&c).is_empty());
        c.database_url = "postgresql://u@db/litegen".to_string();
        assert!(startup_notes(&c).is_empty(), "both Postgres schemes");
        c.database_url = "sqlite://litegen.db".to_string();
        let notes = startup_notes(&c);
        assert_eq!(notes.len(), 1, "only the database: {notes:?}");
        assert!(notes[0].warn && notes[0].text.contains("SQLite"));
    }

    #[tokio::test]
    async fn key_is_the_prefix_then_the_parts_joined_with_colons() {
        let s = SharedRedis::connect(&RedisSettings::from_url("redis://127.0.0.1:1"), "litegen:").await.unwrap();
        assert_eq!(s.key(&["rl", "abc"]), "litegen:rl:abc");
        assert_eq!(s.key(&["events"]), "litegen:events");
        assert_eq!(s.key(&[]), "litegen:");
    }

    #[tokio::test]
    async fn each_client_has_an_id_of_its_own() {
        let settings = RedisSettings::from_url("redis://user:secret@127.0.0.1:1");
        let a = SharedRedis::connect(&settings, "litegen:").await.unwrap();
        let b = SharedRedis::connect(&settings, "litegen:").await.unwrap();
        assert!(uuid::Uuid::parse_str(a.id()).is_ok(), "{}", a.id());
        assert!(uuid::Uuid::parse_str(b.id()).is_ok(), "{}", b.id());
        assert_ne!(a.id(), b.id(), "two clients in one process are two replicas to the event channel");
        assert_ne!(a.id(), replica_id(), "it is not the process's id");
    }

    #[test]
    fn replica_id_is_one_uuid_per_process() {
        let id = replica_id();
        assert!(uuid::Uuid::parse_str(id).is_ok(), "{id}");
        assert_eq!(replica_id(), id);
    }

    #[tokio::test]
    async fn starts_with_redis_down_and_recovers() {
        let Some(url) = test_url() else { return };
        let proxy = CutProxy::start(addr_of(&url)).await;
        proxy.cut();
        let mut settings = RedisSettings::from_url(&proxy.url());
        settings.breaker_recovery = Duration::from_secs(1);
        let s = SharedRedis::connect(&settings, &test_prefix()).await.expect("connects lazily");
        for _ in 0..5 {
            assert!(s.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await }).await.is_err());
        }
        proxy.restore();
        tokio::time::sleep(Duration::from_millis(1100)).await; // past the breaker's recovery
        assert_eq!(s.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await }).await.unwrap(), "PONG");
    }

    #[tokio::test]
    async fn a_hanging_redis_costs_at_most_the_timeout() {
        let Some(url) = test_url() else { return };
        let proxy = CutProxy::start(addr_of(&url)).await;
        proxy.hang();
        let s = SharedRedis::connect(&RedisSettings::from_url(&proxy.url()), &test_prefix()).await.unwrap();
        let t = Instant::now();
        let _ = s.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await }).await;
        assert!(t.elapsed() < Duration::from_millis(1000));
        for _ in 0..5 {
            let _ = s.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await }).await;
        }
        let t = Instant::now();
        assert!(matches!(
            s.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await }).await,
            Err(SharedError::Unavailable)
        ));
        assert!(t.elapsed() < Duration::from_millis(50), "open: no network");
    }

    #[tokio::test]
    async fn a_connection_that_hangs_mid_call_costs_at_most_the_timeout_too() {
        let Some(url) = test_url() else { return };
        let proxy = CutProxy::start(addr_of(&url)).await;
        let s = SharedRedis::connect(&RedisSettings::from_url(&proxy.url()), &test_prefix()).await.unwrap();
        assert_eq!(ping(&s).await.unwrap(), "PONG");
        proxy.hang();
        let t = Instant::now();
        assert!(ping(&s).await.is_err());
        assert!(t.elapsed() < Duration::from_millis(1000), "{:?}", t.elapsed());
        proxy.restore();
        assert_eq!(ping(&s).await.unwrap(), "PONG", "a new connection, after the stalled one was dropped");
    }

    #[tokio::test]
    async fn callers_queued_behind_a_slow_connect_do_not_trip_the_breaker() {
        // The upstream is never reached: a hung proxy forwards nothing.
        let proxy = CutProxy::start("127.0.0.1:1".parse().unwrap()).await;
        proxy.hang();
        let s = SharedRedis::connect(&RedisSettings::from_url(&proxy.url()), &test_prefix()).await.unwrap();
        // One call makes the connection and times out; the other seven wait their
        // turn for it and run out of time in the queue. Only the first reached
        // the network, so only it counts against the default threshold of 5.
        let results = futures::future::join_all((0..8).map(|_| ping(&s))).await;
        assert!(results.iter().all(|r| matches!(r, Err(SharedError::Redis(_)))), "{results:?}");
        proxy.restore();
        let next = ping(&s).await;
        assert!(!matches!(next, Err(SharedError::Unavailable)), "the breaker is still closed: {next:?}");
    }

    #[tokio::test]
    async fn callers_that_each_reach_a_dead_redis_still_open_the_breaker() {
        // Refused at once, so every caller makes an attempt of its own. Callers
        // take the connecting lock one at a time and each has 500 ms in all, so
        // the threshold is kept low: the breaker needs only two serial refused
        // connects to open, which a loaded machine still finishes in time. One
        // caller recording for all of them would not be enough.
        let mut settings = RedisSettings::from_url("redis://127.0.0.1:1");
        settings.breaker_failure_threshold = 2;
        let s = SharedRedis::connect(&settings, &test_prefix()).await.unwrap();
        let results = futures::future::join_all((0..6).map(|_| ping(&s))).await;
        assert!(results.iter().all(|r| r.is_err()), "{results:?}");
        assert!(matches!(ping(&s).await, Err(SharedError::Unavailable)));
    }

    #[tokio::test]
    async fn a_recovery_under_the_call_timeout_counts_as_the_call_timeout() {
        // A call that started before the breaker opened can settle after a
        // zero recovery has passed, and would act as the probe. The floor keeps
        // the breaker open for at least as long as one call can take.
        let mut settings = RedisSettings::from_url("redis://127.0.0.1:1");
        settings.breaker_failure_threshold = 1;
        settings.breaker_recovery = Duration::ZERO;
        let s = SharedRedis::connect(&settings, &test_prefix()).await.unwrap();
        assert!(matches!(ping(&s).await, Err(SharedError::Redis(_))), "refused, and that opens it");
        assert!(matches!(ping(&s).await, Err(SharedError::Unavailable)), "an immediate call is not a probe");
    }

    #[tokio::test]
    async fn concurrent_calls_on_an_empty_slot_share_one_connection() {
        let Some((s, _)) = two_shared().await else { return };
        let ids = tokio::join!(
            client_id(&s),
            client_id(&s),
            client_id(&s),
            client_id(&s),
            client_id(&s),
            client_id(&s),
            client_id(&s),
            client_id(&s),
        );
        let ids = [ids.0, ids.1, ids.2, ids.3, ids.4, ids.5, ids.6, ids.7].map(|id| id.expect("a CLIENT ID"));
        assert!(ids.iter().all(|id| *id == ids[0]), "one connection served all eight: {ids:?}");
    }

    #[tokio::test]
    async fn a_dropped_connection_is_replaced_by_a_new_one() {
        let Some(url) = test_url() else { return };
        let proxy = CutProxy::start(addr_of(&url)).await;
        let s = SharedRedis::connect(&RedisSettings::from_url(&proxy.url()), &test_prefix()).await.unwrap();
        let before = client_id(&s).await.unwrap();
        assert_eq!(client_id(&s).await.unwrap(), before, "one connection serves the calls");
        proxy.cut();
        assert!(ping(&s).await.is_err());
        proxy.restore();
        assert_ne!(client_id(&s).await.unwrap(), before, "the next call connected from scratch");
    }

    #[tokio::test]
    async fn an_error_reply_comes_back_but_leaves_the_breaker_closed_and_the_connection_in_place() {
        let Some((s, _)) = two_shared().await else { return };
        let key = s.key(&["a-string"]);
        let k = key.clone();
        s.run(|mut c| async move { redis::cmd("SET").arg(k).arg("x").query_async::<()>(&mut c).await }).await.unwrap();
        let before = client_id(&s).await.unwrap();
        // Well over the default threshold of 5.
        for _ in 0..8 {
            let k = key.clone();
            let wrong = s.run(|mut c| async move { redis::cmd("LPUSH").arg(k).arg("y").query_async::<i64>(&mut c).await }).await;
            match wrong {
                Err(SharedError::Redis(text)) => assert!(text.contains("WRONGTYPE"), "{text}"),
                other => panic!("expected an error reply, got {other:?}"),
            }
        }
        assert_eq!(client_id(&s).await.unwrap(), before, "an error reply doesn't drop the connection");
        testing::flush_keys(&s).await;
    }

    /// Redis's answer when the node can't serve: made by a script, so the test
    /// needs no failover. The reply is byte for byte the one a replica sends.
    async fn answer_with(s: &SharedRedis, error: &str) -> Result<(), SharedError> {
        let error = error.to_string();
        s.run(|mut c| async move {
            redis::cmd("EVAL").arg("return redis.error_reply(ARGV[1])").arg(0).arg(error).query_async::<()>(&mut c).await
        })
        .await
    }

    /// What a node answers when it can't serve, as real servers send it: a
    /// primary demoted by a failover (a plain write, or Redis 7 and Valkey),
    /// then a write inside a script on that primary as Redis 6.2.24 and 5.0.14
    /// answer it (wrapped in `ERR`; 5.0 ends it with three spaces), a Redis
    /// loading its data, a replica cut off from its primary.
    const CANNOT_SERVE: [&str; 5] = [
        "READONLY You can't write against a read only replica.",
        "ERR Error running script (call to f_756928dd1246249109cfe8475bfa267e967ea948): @user_script:1: @user_script: 1: -READONLY You can't write against a read only replica.",
        "ERR Error running script (call to f_756928dd1246249109cfe8475bfa267e967ea948): @user_script:1: @user_script: 1: -READONLY You can't write against a read only replica.   ",
        "LOADING Redis is loading the dataset in memory",
        "MASTERDOWN Link with MASTER is down and replica-serve-stale-data is set to 'no'.",
    ];

    #[tokio::test]
    async fn a_node_that_cannot_serve_has_its_connection_replaced() {
        let Some((s, _)) = two_shared().await else { return };
        for error in CANNOT_SERVE {
            let before = client_id(&s).await.unwrap();
            let answered = answer_with(&s, error).await;
            let (_, said) = error.split_once(' ').unwrap();
            assert!(matches!(&answered, Err(SharedError::Redis(text)) if text.contains(said)), "{answered:?}");
            assert_ne!(
                client_id(&s).await.unwrap(),
                before,
                "after `{error}` the next call must connect afresh: a primary demoted by a failover keeps its old connections"
            );
        }
    }

    #[tokio::test]
    async fn answers_from_a_node_that_cannot_serve_open_the_breaker() {
        for error in CANNOT_SERVE {
            let Some((s, _)) = two_shared().await else { return };
            for _ in 0..5 {
                let _ = answer_with(&s, error).await;
            }
            let next = ping(&s).await;
            assert!(matches!(next, Err(SharedError::Unavailable)), "five `{error}` answers in a row must open the breaker, got {next:?}");
        }
    }

    /// Any other error inside a script, as Redis 6.2.24 wraps it: a
    /// WRONGTYPE from `redis.call`, a Lua error, an unknown command.
    const SCRIPT_ERRORS: [&str; 3] = [
        "ERR Error running script (call to f_55adaad9aeacea76b502ddc5b6f00f789cec478c): @user_script:1: WRONGTYPE Operation against a key holding the wrong kind of value",
        "ERR Error running script (call to f_d7b467a515fcbd21bd1b452549c352cfb23b2458): @user_script:1: user_script:1: Script attempted to access nonexistent global variable 'nosuch'",
        "ERR Error running script (call to f_4bea801b1c3d4dabc2dcad5bce19abeff09aea95): @user_script:1: @user_script: 1: Unknown Redis command called from Lua script",
    ];

    /// Only a wrapped READONLY says the node can't serve. Every other script
    /// error stays an answer: a bug in one script must not switch every
    /// replica to its own counters.
    #[tokio::test]
    async fn a_script_error_other_than_readonly_stays_an_answer() {
        let Some((s, _)) = two_shared().await else { return };
        let before = client_id(&s).await.unwrap();
        for error in SCRIPT_ERRORS {
            // Over the default threshold of 5, for each.
            for _ in 0..6 {
                let answered = answer_with(&s, error).await;
                assert!(matches!(&answered, Err(SharedError::Redis(text)) if text.contains("Error running script")), "{answered:?}");
            }
        }
        assert_eq!(client_id(&s).await.unwrap(), before, "a script error leaves the connection in place");
        assert_eq!(ping(&s).await, Ok("PONG".to_string()), "and the breaker closed");
    }

    /// A PING that `late` holds back once the connection is in hand: to `run`,
    /// a Redis that answered `late` after the command went out.
    async fn slow_ping(s: &SharedRedis, late: Duration) -> Result<String, SharedError> {
        s.run(|mut c| async move {
            tokio::time::sleep(late).await;
            redis::cmd("PING").query_async::<String>(&mut c).await
        })
        .await
    }

    /// A client with the shipped settings, the 250 ms slow-call rule
    /// included, which the `testing` helpers' clients leave off.
    async fn shipped() -> Option<Arc<SharedRedis>> {
        let url = test_url()?;
        Some(SharedRedis::connect(&RedisSettings::from_url(&url), &test_prefix()).await.unwrap())
    }

    /// A Redis that answers, but slowly, costs a request as much as one that
    /// doesn't: five slow calls in a row open the breaker like five failures.
    #[tokio::test]
    async fn calls_that_redis_answers_slowly_open_the_breaker() {
        let Some(s) = shipped().await else { return };
        assert_eq!(ping(&s).await.unwrap(), "PONG"); // connected
        for i in 0..5 {
            // Late, but inside the 500 ms a call may take.
            let answered = slow_ping(&s, Duration::from_millis(300)).await;
            assert_eq!(answered.unwrap(), "PONG", "slow call {i} still returns what Redis said");
        }
        let t = Instant::now();
        let next = ping(&s).await;
        assert!(matches!(next, Err(SharedError::Unavailable)), "five slow calls in a row must open the breaker, got {next:?}");
        assert!(t.elapsed() < Duration::from_millis(50), "open: no network");
    }

    /// One stall delays every call in flight. That is one slow Redis, not a
    /// failure per call: counted per call, one 300 ms hiccup on a busy replica
    /// would open the breaker for the whole recovery period.
    #[tokio::test]
    async fn slow_answers_to_calls_sent_together_count_once() {
        let Some(s) = shipped().await else { return };
        assert_eq!(ping(&s).await.unwrap(), "PONG");
        let answers = futures::future::join_all((0..8).map(|_| slow_ping(&s, Duration::from_millis(300)))).await;
        assert!(answers.iter().all(|a| a.as_deref() == Ok("PONG")), "{answers:?}");
        let next = ping(&s).await;
        assert_eq!(next, Ok("PONG".to_string()), "eight slow answers to calls sent together opened the breaker");
    }

    #[tokio::test]
    async fn the_slow_call_setting_decides_what_counts_as_slow() {
        let Some(url) = test_url() else { return };
        let mut settings = RedisSettings::from_url(&url);
        settings.slow_call = Duration::from_millis(100);
        let strict = SharedRedis::connect(&settings, &test_prefix()).await.unwrap();
        settings.slow_call = Duration::ZERO;
        let off = SharedRedis::connect(&settings, &test_prefix()).await.unwrap();
        for _ in 0..5 {
            assert_eq!(slow_ping(&strict, Duration::from_millis(150)).await.unwrap(), "PONG");
            assert_eq!(slow_ping(&off, Duration::from_millis(300)).await.unwrap(), "PONG");
        }
        let next = ping(&strict).await;
        assert!(matches!(next, Err(SharedError::Unavailable)), "150 ms answers are slow under a 100 ms setting, got {next:?}");
        assert_eq!(ping(&off).await, Ok("PONG".to_string()), "a setting of 0 counts no answer as slow");
    }

    /// The clients the `testing` helpers build count no answer as slow: on a
    /// busy test runner, five answers that happen to take 250 ms must not open
    /// the breaker in a test that is about something else.
    #[tokio::test]
    async fn the_test_helpers_clients_count_no_answer_as_slow() {
        let Some((s, _)) = two_shared().await else { return };
        assert_eq!(ping(&s).await.unwrap(), "PONG"); // connected
        for _ in 0..5 {
            assert_eq!(slow_ping(&s, Duration::from_millis(260)).await.unwrap(), "PONG");
        }
        assert_eq!(ping(&s).await, Ok("PONG".to_string()), "five slow answers opened the breaker of a test helper's client");
    }

    #[tokio::test]
    async fn calls_in_flight_on_one_lost_connection_count_as_one_failure() {
        let Some(url) = test_url() else { return };
        let proxy = CutProxy::start(addr_of(&url)).await;
        let s = SharedRedis::connect(&RedisSettings::from_url(&proxy.url()), &test_prefix()).await.unwrap();
        assert_eq!(ping(&s).await.unwrap(), "PONG");
        // Eight calls go out on the one connection, which stops answering: each
        // times out. It is one connection lost, not eight failures of Redis,
        // so the default threshold of 5 is not reached.
        proxy.hang();
        let results = futures::future::join_all((0..8).map(|_| ping(&s))).await;
        assert!(results.iter().all(|r| matches!(r, Err(SharedError::Redis(_)))), "{results:?}");
        proxy.restore();
        let next = ping(&s).await;
        assert_eq!(
            next,
            Ok("PONG".to_string()),
            "the calls in flight on one lost connection opened the breaker as if each were a failure of its own"
        );
    }

    #[tokio::test]
    async fn errors_never_carry_the_password() {
        let url = "redis://:hunter2@127.0.0.1:1";
        let settings = RedisSettings::from_url(url);
        let s = SharedRedis::connect(&settings, "p:").await.expect("connects lazily");
        let err = ping(&s).await.expect_err("nothing listens on port 1");
        assert!(matches!(err, SharedError::Redis(_)), "{err:?}");
        for text in [format!("{err}"), format!("{err:?}"), format!("{s:?}"), format!("{settings:?}")] {
            assert!(!text.contains("hunter2"), "leaked: {text}");
        }
        // A URL that can't be used: the error says so without quoting it.
        for bad in ["redis://:hunter2@127.0.0.1:notaport", "rediss://:hunter2@127.0.0.1:6379#nonsense"] {
            let err = SharedRedis::connect(&RedisSettings::from_url(bad), "p:").await.expect_err(bad);
            for text in [format!("{err}"), format!("{err:?}")] {
                assert!(!text.contains("hunter2"), "{bad} leaked: {text}");
            }
        }
        // And a TLS one whose CA file is missing.
        let mut tls = RedisSettings::from_url("rediss://:hunter2@127.0.0.1:1");
        tls.ca_certs = Some("/nonexistent/ca.pem".into());
        let err = SharedRedis::connect(&tls, "p:").await.expect_err("no such file");
        assert!(!format!("{err:?}").contains("hunter2"));
    }

    #[tokio::test]
    async fn a_tls_url_is_an_error_not_a_panic_whichever_crypto_provider_the_process_has() {
        // rustls panics building a client config when it has no default
        // provider and two to choose from (litegen's dependencies enable both).
        let s = SharedRedis::connect(&RedisSettings::from_url("rediss://127.0.0.1:1"), "p:").await.unwrap();
        assert!(matches!(ping(&s).await, Err(SharedError::Redis(_))));
    }

    #[tokio::test]
    async fn turning_certificate_checks_off_is_the_url_s_insecure_form() {
        let verified = build_client(&RedisSettings::from_url("rediss://127.0.0.1:6380")).await.unwrap();
        assert!(matches!(verified.get_connection_info().addr(), ConnectionAddr::TcpTls { insecure: false, .. }));
        let mut off = RedisSettings::from_url("rediss://127.0.0.1:6380");
        off.verify_certificates = false;
        let unverified = build_client(&off).await.unwrap();
        assert!(matches!(unverified.get_connection_info().addr(), ConnectionAddr::TcpTls { insecure: true, .. }));
        // A plain redis:// URL has nothing to turn off.
        let mut plain = RedisSettings::from_url("redis://127.0.0.1:6379");
        plain.verify_certificates = false;
        assert!(matches!(build_client(&plain).await.unwrap().get_connection_info().addr(), ConnectionAddr::Tcp(..)));
    }

    #[test]
    fn the_ca_bundle_is_the_system_roots_then_the_file() {
        use base64::Engine as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca.pem");
        std::fs::write(&path, TEST_CA).unwrap();
        let bundle = String::from_utf8(ca_bundle(&path).unwrap()).unwrap();
        assert!(bundle.ends_with(TEST_CA), "the file's certificates come last, as written");

        // Every block before the file's is one of the system's roots, and every
        // root is there (the system lists them in no fixed order).
        let blocks: Vec<&str> = bundle.split("-----END CERTIFICATE-----").collect();
        let mut in_bundle: Vec<Vec<u8>> = blocks[..blocks.len() - 2]
            .iter()
            .map(|block| {
                let b64: String = block.lines().filter(|l| !l.starts_with("-----")).collect();
                base64::engine::general_purpose::STANDARD.decode(b64).expect("base64 between the armor lines")
            })
            .collect();
        let mut system: Vec<Vec<u8>> = rustls_native_certs::load_native_certs().certs.iter().map(|c| c.as_ref().to_vec()).collect();
        in_bundle.sort();
        system.sort();
        assert_eq!(in_bundle, system);
        assert_eq!(bundle.matches(PEM_BEGIN).count(), system.len() + 1, "plus the file's one");
    }

    #[test]
    fn pem_armor_wraps_at_64_characters() {
        let der: Vec<u8> = (0..100u8).collect();
        let bundle = pem_bundle([der.as_slice()], "-----BEGIN CERTIFICATE-----\nQQ==\n-----END CERTIFICATE-----");
        let lines: Vec<&str> = bundle.lines().collect();
        assert_eq!(lines[0], "-----BEGIN CERTIFICATE-----");
        assert_eq!((lines[1].len(), lines[2].len(), lines[3].len()), (64, 64, 8), "100 bytes are 136 base64 characters");
        assert_eq!(lines[4], "-----END CERTIFICATE-----");
        assert_eq!(&lines[5..], ["-----BEGIN CERTIFICATE-----", "QQ==", "-----END CERTIFICATE-----"]);
        assert!(bundle.ends_with('\n'), "a file with no final newline still ends the bundle cleanly");
    }

    #[tokio::test]
    async fn the_ca_file_is_checked_when_connecting_with_tls_and_only_then() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("ca.pem");
        std::fs::write(&good, TEST_CA).unwrap();
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "not a certificate\n").unwrap();

        let mut s = RedisSettings::from_url("rediss://127.0.0.1:6380");
        s.ca_certs = Some(good.clone());
        SharedRedis::connect(&s, "p:").await.expect("redis accepts the bundle: system roots and the file");

        s.ca_certs = Some(dir.path().join("missing.pem"));
        let err = SharedRedis::connect(&s, "p:").await.expect_err("unreadable");
        assert!(err.to_string().contains("missing.pem"), "{err}");

        s.ca_certs = Some(empty);
        let err = SharedRedis::connect(&s, "p:").await.expect_err("no certificate in it");
        assert!(err.to_string().contains("no PEM certificate"), "{err}");

        // With a plain redis:// URL the CA file means nothing, so nothing reads it.
        let mut plain = RedisSettings::from_url("redis://127.0.0.1:6379");
        plain.ca_certs = Some(dir.path().join("missing.pem"));
        SharedRedis::connect(&plain, "p:").await.expect("ignored without TLS");
    }

    #[tokio::test]
    async fn publish_reaches_a_subscriber_on_another_client() {
        use futures::StreamExt;
        let Some((a, b)) = two_shared().await else { return };
        let channel = a.key(&["events"]);
        let mut subscription = a.subscribe(&channel).await.expect("subscribes");
        let mut messages = subscription.on_message();
        b.publish(&channel, "hello").await.unwrap();
        let msg = tokio::time::timeout(Duration::from_secs(2), messages.next()).await.expect("a message in time").expect("open");
        assert_eq!(msg.get_channel_name(), channel);
        assert_eq!(msg.get_payload::<String>().unwrap(), "hello");
    }

    #[tokio::test]
    async fn a_dead_redis_fails_calls_publish_and_subscribe_without_hanging() {
        let s = dead_redis(false).await;
        assert!(!s.fail_closed());
        assert!(dead_redis(true).await.fail_closed());
        let t = Instant::now();
        assert!(matches!(s.publish("c", "m").await, Err(SharedError::Redis(_))));
        assert!(matches!(s.subscribe("c").await, Err(SharedError::Redis(_))));
        assert!(s.ping().await.is_err());
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[tokio::test]
    async fn two_clients_on_one_prefix_see_each_others_keys() {
        let Some((a, b)) = two_shared().await else { return };
        assert_eq!(a.key(&["k"]), b.key(&["k"]));
        let k = a.key(&["k"]);
        a.run(|mut c| async move { redis::cmd("SET").arg(k).arg("v").query_async::<()>(&mut c).await }).await.unwrap();
        let k = b.key(&["k"]);
        let got = b.run(|mut c| async move { redis::cmd("GET").arg(k).query_async::<String>(&mut c).await }).await.unwrap();
        assert_eq!(got, "v");
        testing::flush_keys(&a).await;
        let k = b.key(&["k"]);
        let gone = b.run(|mut c| async move { redis::cmd("EXISTS").arg(k).query_async::<i64>(&mut c).await }).await.unwrap();
        assert_eq!(gone, 0, "flush_keys deletes this prefix's keys");
    }
}
