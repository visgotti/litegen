pub mod blocking;
pub mod content;
pub mod http_clients;
pub mod ssrf;

/// Test only: a stand-in vendor on loopback serving `app`, with its base URL
/// and the number of TCP connections it has accepted. Every fresh HTTP
/// client opens a connection of its own, so the count shows client reuse
/// (P8, code review 2026-11-11).
#[cfg(test)]
pub(crate) async fn counting_vendor(app: axum::Router) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use axum::serve::ListenerExt;
    let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind a loopback port");
    let base = format!("http://{}", listener.local_addr().expect("its address"));
    let counter = accepted.clone();
    let listener = listener.tap_io(move |_| {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base, accepted)
}
