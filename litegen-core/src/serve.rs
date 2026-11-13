//! litegen's HTTP server loop: what `axum::serve` does, plus the two deadlines
//! it cannot set.
//!
//! - **Headers.** axum builds hyper's connection with no timer, so hyper's
//!   header read timeout never fires: a client could hold a connection open
//!   forever by sending part of a request head, or nothing at all. Here each
//!   connection gets `header_read_timeout`, which also closes a keep-alive
//!   connection left idle that long (a reverse proxy in front must close its
//!   idle upstream connections sooner). Before the protocol is known (HTTP/1
//!   or HTTP/2) hyper has no timer at all, so a connection that has not sent
//!   its first byte within the same deadline is closed here. One that sends
//!   only part of the HTTP/2 preface is still not timed, nor is an HTTP/2
//!   connection without TLS (h2c) once its preface is in: the timer is
//!   HTTP/1's. Neither holds a generation slot, and a stop closes both.
//! - **The stop.** axum waits for every connection with no limit, so one
//!   client stalled mid-body held a shutdown for as long as it liked. Here a
//!   stop closes idle connections at once, lets requests in progress finish
//!   within the grace, and then closes whatever is left.
//!
//! Each request carries the peer's address (`ConnectInfo<SocketAddr>`), as
//! `into_make_service_with_connect_info` gives it: the MCP OAuth endpoints
//! limit sign-ins per address. TCP_NODELAY is left as axum leaves it.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::ConnectInfo;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpStream;
use tokio::sync::watch;

/// The deadlines the server loop applies to every connection.
#[derive(Debug, Clone, Copy)]
pub struct ServeLimits {
    /// How long a connection may take to send a request's headers.
    pub header_read_timeout: Duration,
}

/// How [`serve`] ended after the stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// Every connection finished within the grace.
    Drained,
    /// The grace ran out: `cut` connections were closed with their requests.
    GraceExpired { cut: usize },
}

/// Serve `app` on `listener` until `stop` resolves with the grace to give the
/// open connections. Then no connection is accepted, idle ones close at once,
/// and requests in progress get the grace; whatever is still open at its end
/// is closed. Returns once every connection is closed.
pub async fn serve(
    mut listener: tokio::net::TcpListener,
    app: axum::Router,
    stop: impl Future<Output = Duration> + Send + 'static,
    limits: ServeLimits,
) -> std::io::Result<Stopped> {
    let (closing_tx, closing_rx) = watch::channel(false);
    let mut connections = tokio::task::JoinSet::new();
    let mut stop = std::pin::pin!(stop);
    let grace = loop {
        tokio::select! {
            biased;
            grace = &mut stop => break grace,
            // axum's accept: a failed accept is retried, with a pause when
            // the process is out of file descriptors.
            (tcp, peer) = axum::serve::Listener::accept(&mut listener) => {
                while connections.try_join_next().is_some() {}
                connections.spawn(connection(tcp, peer, app.clone(), limits.header_read_timeout, closing_rx.clone()));
            }
        }
    };
    // A client that connects from now on is refused.
    drop(listener);
    let _ = closing_tx.send(true);
    let drained = tokio::time::timeout(grace, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    if drained.is_ok() {
        return Ok(Stopped::Drained);
    }
    // Connections that ended just as the grace ran out are not cut.
    while connections.try_join_next().is_some() {}
    let stopped = after_the_grace(connections.len());
    connections.abort_all();
    // Each aborted task drops its socket: once this returns, they are closed.
    while connections.join_next().await.is_some() {}
    Ok(stopped)
}

/// How a stop ends when its grace ran out, given the connections still open
/// once those that ended meanwhile are reaped: those are cut. When none is
/// left, every connection ended within the grace after all.
fn after_the_grace(still_open: usize) -> Stopped {
    match still_open {
        0 => Stopped::Drained,
        cut => Stopped::GraceExpired { cut },
    }
}

/// One connection, from its first byte to its close. `closing` turns true at
/// the stop.
async fn connection(tcp: TcpStream, peer: SocketAddr, app: axum::Router, head_deadline: Duration, mut closing: watch::Receiver<bool>) {
    // hyper's header timer starts only once the protocol is known, so a
    // client that sends nothing at all is closed here, at the same deadline.
    let mut first = [0u8; 1];
    tokio::select! {
        biased;
        _ = closing.wait_for(|stopping| *stopping) => return,
        sent = tokio::time::timeout(head_deadline, tcp.peek(&mut first)) => match sent {
            Ok(Ok(n)) if n > 0 => {}
            // Nothing within the deadline, the client left, or the socket failed.
            _ => return,
        },
    }

    let service = tower::ServiceExt::map_request(app, move |mut req: axum::http::Request<hyper::body::Incoming>| {
        req.extensions_mut().insert(ConnectInfo(peer));
        req.map(axum::body::Body::new)
    });
    let mut builder = Builder::new(TokioExecutor::new());
    builder.http1().timer(TokioTimer::new()).header_read_timeout(head_deadline);
    // The CONNECT protocol, for HTTP/2 websockets, as axum enables it.
    builder.http2().enable_connect_protocol();
    let conn = builder.serve_connection_with_upgrades(TokioIo::new(tcp), TowerToHyperService::new(service));
    let mut conn = std::pin::pin!(conn);
    let mut stopping = false;
    loop {
        tokio::select! {
            served = conn.as_mut() => {
                if let Err(e) = served {
                    tracing::trace!(%peer, error = %e, "connection ended with an error");
                }
                return;
            }
            // At the stop: an idle connection closes now, one with a request
            // in progress after its response (the grace bounds that).
            _ = closing.wait_for(|stopping| *stopping), if !stopping => {
                stopping = true;
                conn.as_mut().graceful_shutdown();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ConnectInfo;
    use axum::routing::{get, post};
    use axum::Router;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// Generous next to the 300 ms deadlines below: under a loaded test run
    /// the deadline still fires long before this. A server that never closes
    /// fails here.
    const WAIT: Duration = Duration::from_secs(5);

    fn app() -> Router {
        Router::new()
            .route("/hello", get(|| async { "hello" }))
            .route("/slow", get(|| async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                "slow"
            }))
            .route("/upload", post(|body: axum::body::Bytes| async move { body.len().to_string() }))
            .route("/peer", get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.to_string() }))
    }

    /// When every connection ended just as the grace ran out (between the
    /// timer firing and the reap, which only a multi-thread runtime can
    /// interleave), nothing was cut: the stop drained, and `main` must not
    /// warn that it cut 0 connections.
    #[test]
    fn a_grace_that_ran_out_with_nothing_left_open_is_a_drain() {
        assert_eq!(after_the_grace(0), Stopped::Drained, "connections that all ended as the grace ran out were reported as cut");
        assert_eq!(after_the_grace(2), Stopped::GraceExpired { cut: 2 });
    }

    /// A server on a free loopback port, and the sender that stops it with a
    /// grace.
    async fn start(
        header_read_timeout: Duration,
    ) -> (SocketAddr, tokio::sync::oneshot::Sender<Duration>, tokio::task::JoinHandle<std::io::Result<Stopped>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<Duration>();
        let stop = async move { stop_rx.await.unwrap_or(Duration::ZERO) };
        let server = tokio::spawn(serve(listener, app(), stop, ServeLimits { header_read_timeout }));
        (addr, stop_tx, server)
    }

    /// Read until the server closes the connection; what it sent.
    async fn until_closed(client: &mut TcpStream) -> String {
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match client.read(&mut buf).await {
                Ok(0) | Err(_) => return String::from_utf8_lossy(&got).into_owned(),
                Ok(n) => got.extend_from_slice(&buf[..n]),
            }
        }
    }

    /// Read until what arrived ends with `tail`.
    async fn until_ends_with(client: &mut TcpStream, tail: &str) -> String {
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while !String::from_utf8_lossy(&got).ends_with(tail) {
            let n = client.read(&mut buf).await.unwrap();
            assert!(n > 0, "closed before {tail:?}: {}", String::from_utf8_lossy(&got));
            got.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&got).into_owned()
    }

    #[tokio::test]
    async fn a_client_that_stalls_its_headers_is_disconnected() {
        let (addr, _stop, _server) = start(Duration::from_millis(300)).await;
        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"GET /hello HTTP/1.1\r\nHost: litegen\r\n").await.unwrap();
        assert!(
            tokio::time::timeout(WAIT, until_closed(&mut client)).await.is_ok(),
            "a client that stalled mid-headers was still connected {WAIT:?} later"
        );
    }

    #[tokio::test]
    async fn a_client_that_sends_nothing_is_disconnected_at_the_header_deadline() {
        let (addr, _stop, _server) = start(Duration::from_millis(300)).await;
        let mut client = TcpStream::connect(addr).await.unwrap();
        assert!(
            tokio::time::timeout(WAIT, until_closed(&mut client)).await.is_ok(),
            "a client that sent nothing was still connected {WAIT:?} later"
        );
    }

    #[tokio::test]
    async fn the_stop_gives_up_on_a_stalled_request_at_the_grace() {
        let (addr, stop, server) = start(Duration::from_secs(60)).await;
        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"POST /upload HTTP/1.1\r\nHost: litegen\r\nContent-Length: 100\r\n\r\n0123456789")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        stop.send(Duration::from_millis(300)).unwrap();
        let stopped = tokio::time::timeout(WAIT, server).await.expect("serve returned once the grace ran out").unwrap().unwrap();
        assert_eq!(stopped, Stopped::GraceExpired { cut: 1 });
        assert!(tokio::time::timeout(WAIT, until_closed(&mut client)).await.is_ok(), "the cut client's connection was closed");
    }

    #[tokio::test]
    async fn a_request_in_progress_finishes_within_the_grace() {
        let (addr, stop, server) = start(Duration::from_secs(60)).await;
        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"GET /slow HTTP/1.1\r\nHost: litegen\r\n\r\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        stop.send(Duration::from_secs(2)).unwrap();
        let answer = tokio::time::timeout(WAIT, until_closed(&mut client)).await.expect("the answer, then the close");
        assert!(answer.starts_with("HTTP/1.1 200") && answer.ends_with("slow"), "{answer}");
        let stopped = tokio::time::timeout(WAIT, server).await.expect("serve returned").unwrap().unwrap();
        assert_eq!(stopped, Stopped::Drained);
    }

    #[tokio::test]
    async fn an_idle_keep_alive_connection_does_not_hold_the_stop() {
        let (addr, stop, server) = start(Duration::from_secs(60)).await;
        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"GET /hello HTTP/1.1\r\nHost: litegen\r\n\r\n").await.unwrap();
        until_ends_with(&mut client, "hello").await;
        // The connection stays open, idle, between requests.
        stop.send(Duration::from_secs(30)).unwrap();
        let stopped = tokio::time::timeout(WAIT, server).await.expect("an idle connection did not hold the stop").unwrap().unwrap();
        assert_eq!(stopped, Stopped::Drained);
        assert!(tokio::time::timeout(WAIT, until_closed(&mut client)).await.is_ok(), "the idle connection was closed");
    }

    #[tokio::test]
    async fn handlers_see_the_peer_address() {
        let (addr, _stop, _server) = start(Duration::from_secs(60)).await;
        let mut client = TcpStream::connect(addr).await.unwrap();
        let me = client.local_addr().unwrap().to_string();
        client.write_all(b"GET /peer HTTP/1.1\r\nHost: litegen\r\n\r\n").await.unwrap();
        let answer = until_ends_with(&mut client, &me).await;
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    }
}
