/// Prometheus metrics setup and handler.
///
/// Initialises a `PrometheusHandle` (without spawning its own HTTP listener)
/// via `PrometheusBuilder::install_recorder()` and stores it in a process-wide
/// `OnceLock`.  The axum handler at `GET /metrics` calls `handle.render()` to
/// produce the standard Prometheus text exposition format.
use std::sync::OnceLock;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the Prometheus recorder (no-HTTP variant) and stash the handle.
///
/// Calling this more than once is a no-op; the first call wins.
/// Returns a reference to the installed handle.
pub fn init_prometheus() -> &'static PrometheusHandle {
    PROMETHEUS_HANDLE.get_or_init(|| {
        PrometheusBuilder::new()
            .install_recorder()
            .expect("failed to install Prometheus recorder")
    })
}

/// Get the handle if already installed.
pub fn prometheus_handle() -> Option<&'static PrometheusHandle> {
    PROMETHEUS_HANDLE.get()
}

/// `GET /metrics` — Render all registered metrics in Prometheus text format.
pub async fn metrics_handler() -> impl axum::response::IntoResponse {
    let body = match PROMETHEUS_HANDLE.get() {
        Some(h) => h.render(),
        None => String::new(),
    };
    (
        axum::http::StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn metrics_endpoint_returns_200_with_prometheus_body() {
        // Ensure the recorder is installed
        init_prometheus();

        // Record a test counter so there is at least one metric
        metrics::counter!("litegen_test_counter_total").increment(1);

        let app = axum::Router::new()
            .route("/metrics", axum::routing::get(metrics_handler));

        let req = Request::builder()
            .method("GET")
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();

        // Prometheus text format always has lines like "# TYPE ..." or "# HELP ..."
        assert!(
            body.contains("# TYPE") || body.contains("# HELP") || body.contains("litegen_test_counter_total"),
            "expected Prometheus format markers in body, got: {:?}",
            &body[..body.len().min(200)]
        );
    }
}
