//! The messages replicas send each other over Redis pub/sub, so an admin
//! change reaches every replica within about a second instead of at its next
//! periodic refresh (`docs/scaling.md`).
//!
//! A message only says that something changed. The receiver reads the
//! database, or drops its own cache, for the rest. So a message lost to a
//! dropped connection costs nothing but the wait for the periodic refresh.
//!
//! On the wire an event is `{"event":"reload","from":"<id>"}` on the channel
//! `<prefix>events`. `from` is the publishing client's id ([`SharedRedis::id`],
//! one per replica, since a replica has one client), so a replica can skip what
//! it sent itself: it applied that change when it made it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde::{Deserialize, Serialize};

use super::{SharedRedis, Throttle};

/// The channel's name after the key prefix.
const CHANNEL: &str = "events";

/// The wait before the first resubscribe, doubled after each failure.
const MIN_BACKOFF: Duration = Duration::from_secs(1);
/// The longest wait between resubscribes. A subscription that stayed up this
/// long starts the backoff over when it ends.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// How a subscribed connection is checked to still be alive.
///
/// A NAT gateway, an AWS NLB (which forgets an idle connection after 350 s) or
/// a managed Redis's proxy can drop a connection without telling either end.
/// The listener then waits on a socket that will never speak again, and every
/// later event is missed without a word. So it pings: Redis answers a `PING`
/// on a subscribed connection, and a ping that fails or goes unanswered means
/// the connection is gone.
#[derive(Debug, Clone, Copy)]
struct Liveness {
    /// The time between pings. The traffic also keeps an idle connection from
    /// being forgotten.
    every: Duration,
    /// How long a ping may take before the connection counts as dead.
    within: Duration,
}

impl Liveness {
    /// A dead connection is noticed within 35 s. Until then, and after a drop
    /// that is not noticed, changes arrive at each periodic refresh.
    const PRODUCTION: Liveness = Liveness { every: Duration::from_secs(30), within: Duration::from_secs(5) };
}

/// What one replica tells the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SharedEvent {
    /// Runtime configuration was saved: read it from the database again.
    Reload,
    /// The response cache was emptied: empty yours.
    CacheClear,
}

/// The JSON on the channel.
#[derive(Deserialize)]
struct Wire {
    event: SharedEvent,
    from: String,
}

/// The JSON for `event` sent by the client whose id is `from`.
fn encode(event: SharedEvent, from: &str) -> String {
    serde_json::json!({ "event": event, "from": from }).to_string()
}

/// What a message means to the replica whose client has the id `me`.
#[derive(Debug, PartialEq, Eq)]
enum Incoming {
    /// Another replica's event.
    Event(SharedEvent),
    /// This replica's own, already applied.
    Mine,
    /// Not an event this version knows. The text is why.
    Unreadable(String),
}

fn decode(payload: &str, me: &str) -> Incoming {
    match serde_json::from_str::<Wire>(payload) {
        Ok(wire) if wire.from == me => Incoming::Mine,
        Ok(wire) => Incoming::Event(wire.event),
        Err(e) => Incoming::Unreadable(e.to_string().chars().take(200).collect()),
    }
}

/// Tell the other replicas about `event`. Best effort: a failure is logged
/// (at most once a minute) and never returned, since the replicas catch up at
/// their next periodic refresh anyway.
pub async fn publish_event(shared: &SharedRedis, event: SharedEvent) {
    static FAILED: Throttle = Throttle::new();
    let sent = shared.publish(&shared.key(&[CHANNEL]), &encode(event, shared.id())).await;
    if let Err(e) = sent {
        if FAILED.due(Instant::now()) {
            tracing::warn!(error = %e, ?event, "could not tell the other replicas about a change; they pick it up at their next periodic refresh");
        }
    }
}

/// Listen for the other replicas' events and hand each to `on_event`.
///
/// - **Once after every subscribe**, the first and each one after a drop,
///   `on_event(SharedEvent::Reload)` is called too: whatever was published
///   while this replica was not listening should not wait for the periodic
///   refresh.
/// - **Resubscribes** when the connection drops or cannot be made, waiting 1 s
///   at first and doubling up to 30 s. It does not give up, and a Redis that
///   is down for a day is a warning once a minute.
/// - **Pings** the connection every 30 s, giving each ping 5 s to be answered.
///   A connection that was dropped without a close (so the listener would
///   otherwise wait on it forever) is dropped here too and subscribed again,
///   with the catch-up reload above. The warning for it is part of the once
///   a minute, and never shows the URL.
/// - **Skips** this replica's own events (those `publish_event` sent through
///   `shared`, which carry its id), and ignores ones it cannot read.
///
/// The task never ends on its own: abort the handle to stop it.
pub fn spawn_event_listener(
    shared: Arc<SharedRedis>,
    on_event: impl Fn(SharedEvent) + Send + Sync + 'static,
) -> tokio::task::JoinHandle<()> {
    spawn_listener(shared, on_event, Liveness::PRODUCTION)
}

/// [`spawn_event_listener`] with the ping timing given, so a test need not
/// wait 30 s.
fn spawn_listener(
    shared: Arc<SharedRedis>,
    on_event: impl Fn(SharedEvent) + Send + Sync + 'static,
    liveness: Liveness,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let channel = shared.key(&[CHANNEL]);
        // Events from this client are this replica's own.
        let me = shared.id().to_string();
        let (connection_warning, message_warning) = (Throttle::new(), Throttle::new());
        let mut backoff = MIN_BACKOFF;
        loop {
            match shared.subscribe(&channel).await {
                Ok(pubsub) => {
                    let up_since = Instant::now();
                    tracing::info!("listening for the other replicas' events");
                    on_event(SharedEvent::Reload);
                    // Split, so a ping can go out while the stream waits for
                    // messages. The stream's drop ends the connection.
                    let (mut sink, mut messages) = pubsub.split();
                    let mut pings = tokio::time::interval_at(tokio::time::Instant::now() + liveness.every, liveness.every);
                    pings.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    let silent = loop {
                        tokio::select! {
                            message = messages.next() => {
                                let Some(message) = message else { break false };
                                let incoming = match message.get_payload::<String>() {
                                    Ok(payload) => decode(&payload, &me),
                                    Err(e) => Incoming::Unreadable(e.to_string()),
                                };
                                match incoming {
                                    Incoming::Event(event) => on_event(event),
                                    Incoming::Mine => {}
                                    Incoming::Unreadable(why) => {
                                        if message_warning.due(Instant::now()) {
                                            tracing::warn!(error = %why, "ignored a message on the replicas' event channel that this version cannot read");
                                        }
                                    }
                                }
                            }
                            // A connection that a NAT gateway, a load balancer
                            // or a proxy dropped without a FIN never ends the
                            // stream, so ask it: Redis answers a PING in
                            // subscribe mode. Messages that arrive while the
                            // ping waits are kept in the stream's queue.
                            _ = pings.tick() => {
                                match tokio::time::timeout(liveness.within, sink.ping::<redis::Value>()).await {
                                    Ok(Ok(_)) => {}
                                    // Closed under us: the same as the stream ending.
                                    Ok(Err(_)) => break false,
                                    Err(_elapsed) => break true,
                                }
                            }
                        }
                    };
                    if up_since.elapsed() >= MAX_BACKOFF {
                        backoff = MIN_BACKOFF;
                    }
                    if connection_warning.due(Instant::now()) {
                        if silent {
                            tracing::warn!(
                                "the replicas' event subscription stopped answering pings; dropping it and resubscribing (changes still arrive at each periodic refresh)"
                            );
                        } else {
                            tracing::warn!("lost the replicas' event subscription; resubscribing (changes still arrive at each periodic refresh)");
                        }
                    }
                }
                Err(e) => {
                    if connection_warning.due(Instant::now()) {
                        tracing::warn!(error = %e, "could not subscribe to the replicas' events; trying again (changes still arrive at each periodic refresh)");
                    }
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    })
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;
    use crate::config::connections::RedisSettings;
    use crate::shared::testing::{addr_of, dead_redis, test_prefix, test_url, two_shared, CutProxy};

    #[test]
    fn events_are_the_documented_json() {
        assert_eq!(encode(SharedEvent::Reload, "r1"), r#"{"event":"reload","from":"r1"}"#);
        assert_eq!(encode(SharedEvent::CacheClear, "r1"), r#"{"event":"cache-clear","from":"r1"}"#);
        for event in [SharedEvent::Reload, SharedEvent::CacheClear] {
            assert_eq!(decode(&encode(event, "r1"), "r2"), Incoming::Event(event), "round trip");
        }
    }

    #[test]
    fn a_replica_skips_its_own_events() {
        assert_eq!(decode(&encode(SharedEvent::Reload, "r1"), "r1"), Incoming::Mine);
        assert_eq!(decode(&encode(SharedEvent::CacheClear, "r1"), "r2"), Incoming::Event(SharedEvent::CacheClear));
    }

    #[test]
    fn what_this_version_cannot_read_is_ignored() {
        for payload in [
            "",
            "reload",
            "{",
            r#"{"event":"reload"}"#,
            r#"{"from":"r1"}"#,
            r#"{"event":"explode","from":"r1"}"#,
            r#"{"event":7,"from":"r1"}"#,
        ] {
            assert!(matches!(decode(payload, "r2"), Incoming::Unreadable(_)), "{payload}");
        }
        // Fields a newer version adds do not make an event unreadable.
        assert_eq!(decode(r#"{"event":"reload","from":"r1","at":5}"#, "r2"), Incoming::Event(SharedEvent::Reload));
    }

    /// A listener on `shared` whose events arrive on the returned channel.
    fn listen(shared: Arc<SharedRedis>) -> (tokio::task::JoinHandle<()>, mpsc::UnboundedReceiver<SharedEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (spawn_event_listener(shared, move |event| {
            let _ = tx.send(event);
        }), rx)
    }

    async fn next(rx: &mut mpsc::UnboundedReceiver<SharedEvent>, within: Duration) -> Option<SharedEvent> {
        tokio::time::timeout(within, rx.recv()).await.ok().flatten()
    }

    #[tokio::test]
    async fn an_event_published_on_one_client_arrives_on_another_but_not_the_senders_own() {
        let Some((a, b)) = two_shared().await else { return };
        let (task_a, mut on_a) = listen(a.clone());
        let (task_b, mut on_b) = listen(b.clone());
        assert_eq!(next(&mut on_a, Duration::from_secs(5)).await, Some(SharedEvent::Reload), "a subscribed");
        assert_eq!(next(&mut on_b, Duration::from_secs(5)).await, Some(SharedEvent::Reload), "b subscribed");

        // Both clients are this process, but each has its own id: a's event is
        // b's to hear and a's own to skip.
        publish_event(&a, SharedEvent::CacheClear).await;
        assert_eq!(next(&mut on_b, Duration::from_secs(5)).await, Some(SharedEvent::CacheClear), "b heard a's event");
        assert_eq!(next(&mut on_a, Duration::from_millis(300)).await, None, "a skipped its own");
        // And the other way round.
        publish_event(&b, SharedEvent::Reload).await;
        assert_eq!(next(&mut on_a, Duration::from_secs(5)).await, Some(SharedEvent::Reload), "a heard b's event");
        assert_eq!(next(&mut on_b, Duration::from_millis(300)).await, None, "b skipped its own");

        // A replica neither knows is heard by both; what cannot be read is ignored.
        let channel = a.key(&[CHANNEL]);
        a.publish(&channel, "not an event").await.unwrap();
        a.publish(&channel, &encode(SharedEvent::CacheClear, "another-replica")).await.unwrap();
        assert_eq!(next(&mut on_a, Duration::from_secs(5)).await, Some(SharedEvent::CacheClear), "a heard a third replica");
        assert_eq!(next(&mut on_b, Duration::from_secs(5)).await, Some(SharedEvent::CacheClear), "b heard it too: the unreadable message was ignored");
        assert_eq!(next(&mut on_a, Duration::from_millis(200)).await, None, "nothing else reached a");
        assert_eq!(next(&mut on_b, Duration::from_millis(200)).await, None, "nothing else reached b");
        task_a.abort();
        task_b.abort();
    }

    #[tokio::test]
    async fn the_listener_resubscribes_after_the_connection_drops() {
        let Some(url) = test_url() else { return };
        let proxy = CutProxy::start(addr_of(&url)).await;
        let shared = SharedRedis::connect(&RedisSettings::from_url(&proxy.url()), &test_prefix()).await.unwrap();
        let (task, mut rx) = listen(shared);
        assert_eq!(next(&mut rx, Duration::from_secs(5)).await, Some(SharedEvent::Reload), "subscribed");

        proxy.cut();
        tokio::time::sleep(Duration::from_millis(300)).await;
        proxy.restore();
        assert_eq!(next(&mut rx, Duration::from_secs(8)).await, Some(SharedEvent::Reload), "and again after the drop");
        task.abort();
    }

    /// What the log said while the guard lives, on this thread: a
    /// `#[tokio::test]` runs everything, the listener task included, on one.
    #[derive(Clone, Default)]
    struct Logs(Arc<std::sync::Mutex<Vec<u8>>>);

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

    fn capture_logs() -> (Logs, tracing::subscriber::DefaultGuard) {
        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt().with_writer(logs.clone()).with_ansi(false).with_max_level(tracing::Level::TRACE).finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    /// The case `cut()` does not cover: a NAT gateway or a load balancer that
    /// drops a connection without a FIN. The socket stays open and nothing
    /// comes down it, so the stream of messages never ends by itself. The
    /// listener has to notice through its own pings, while the connection is
    /// still hung: restoring the proxy closes the hung sockets, and the
    /// listener would then see an ordinary close, so the proof is the warning
    /// that comes before the restore. A listener that only waits for the
    /// stream to end logs nothing until then.
    #[tokio::test]
    async fn a_subscription_that_goes_silent_is_dropped_and_made_again() {
        let Some(url) = test_url() else { return };
        let (logs, _guard) = capture_logs();
        let proxy = CutProxy::start(addr_of(&url)).await;
        let prefix = test_prefix();
        let listening = SharedRedis::connect(&RedisSettings::from_url(&proxy.url()), &prefix).await.unwrap();
        // Another replica, straight to Redis.
        let other = SharedRedis::connect(&RedisSettings::from_url(&url), &prefix).await.unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let liveness = Liveness { every: Duration::from_millis(200), within: Duration::from_millis(300) };
        let task = spawn_listener(listening, move |event| {
            let _ = tx.send(event);
        }, liveness);
        assert_eq!(next(&mut rx, Duration::from_secs(5)).await, Some(SharedEvent::Reload), "subscribed");

        proxy.hang();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !logs.text().contains("stopped answering") && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(logs.text().contains("stopped answering"), "the listener noticed the silence on its own: {}", logs.text());

        proxy.restore();
        assert_eq!(
            next(&mut rx, Duration::from_secs(10)).await,
            Some(SharedEvent::Reload),
            "it subscribed again, with the catch-up reload"
        );
        publish_event(&other, SharedEvent::CacheClear).await;
        assert_eq!(
            next(&mut rx, Duration::from_secs(5)).await,
            Some(SharedEvent::CacheClear),
            "and what another replica publishes afterwards arrives"
        );

        let text = logs.text();
        assert!(
            !text.contains(&proxy.addr().to_string()) && !text.contains("redis://"),
            "the warning never shows the URL: {text}"
        );
        task.abort();
    }

    #[tokio::test]
    async fn publishing_with_redis_down_is_quiet_and_quick() {
        let shared = dead_redis(false).await;
        let started = Instant::now();
        publish_event(&shared, SharedEvent::Reload).await;
        assert!(started.elapsed() < Duration::from_secs(2), "bounded by the call timeout");
    }
}
