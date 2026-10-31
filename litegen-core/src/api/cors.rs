//! The CORS policy: which origins may call litegen from a browser, which
//! request headers they may send, and which response headers their scripts
//! may read (L19: an SDK that shows headers).

use axum::http::{header, HeaderName, HeaderValue, Method};
use tower_http::cors::{Any, CorsLayer};

use crate::config::CorsConfig;

/// Request headers a cross-origin caller may send: every header the SDKs set.
pub fn allowed_request_headers() -> Vec<HeaderName> {
    let litegen = [
        "x-litegen-strict",
        "x-csrf-token",
        "idempotency-key",
        "x-litegen-org-id",
        "x-litegen-app-id",
        "x-litegen-end-user",
        "x-litegen-tags",
    ];
    [header::AUTHORIZATION, header::CONTENT_TYPE].into_iter().chain(litegen.map(HeaderName::from_static)).collect()
}

/// Response headers a cross-origin script may READ. CORS hides every
/// response header outside the safelist, so without this a browser SDK sees
/// these as absent though the server sent them.
pub fn exposed_response_headers() -> Vec<HeaderName> {
    let litegen = [
        "x-litegen-dropped-params",
        "x-litegen-conversion-losses",
        "x-ratelimit-limit",
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
        "idempotent-replayed",
    ];
    [header::CONTENT_DISPOSITION, header::RETRY_AFTER].into_iter().chain(litegen.map(HeaderName::from_static)).collect()
}

/// The CORS layer for `config`. `origins_override` (`LITEGEN_CORS_ORIGINS`,
/// a CSV) replaces the configured origins when set: `*` allows any origin,
/// none denies every cross-origin call.
pub fn layer(config: &CorsConfig, origins_override: Option<&str>) -> CorsLayer {
    let methods = [Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS];
    let origins: Vec<String> = match origins_override.filter(|s| !s.is_empty()) {
        Some(csv) => csv.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        None => config.allowed_origins.clone(),
    };
    let layer = CorsLayer::new()
        .allow_methods(methods.to_vec())
        .allow_headers(allowed_request_headers())
        .expose_headers(exposed_response_headers());
    let layer = if origins.iter().any(|o| o == "*") {
        layer.allow_origin(Any)
    } else if origins.is_empty() {
        layer
    } else {
        layer.allow_origin(origins.iter().filter_map(|o| HeaderValue::from_str(o).ok()).collect::<Vec<_>>())
    };
    if config.allow_credentials {
        layer.allow_credentials(true)
    } else {
        layer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    const ORIGIN: &str = "https://app.example";

    async fn answer(req: Request<Body>) -> axum::response::Response {
        let config = CorsConfig { allowed_origins: vec![ORIGIN.into()], allow_credentials: false };
        let app = axum::Router::new()
            .route("/v1/x", axum::routing::get(|| async { "ok" }).put(|| async { "ok" }))
            .layer(layer(&config, None));
        app.oneshot(req).await.unwrap()
    }

    fn listed(resp: &axum::response::Response, name: &str) -> Vec<String> {
        resp.headers()
            .get(name)
            .map(|v| v.to_str().unwrap().split(',').map(|s| s.trim().to_ascii_lowercase()).collect())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn a_browser_can_read_every_header_the_sdks_report() {
        let resp = answer(Request::get("/v1/x").header("origin", ORIGIN).body(Body::empty()).unwrap()).await;
        let exposed = listed(&resp, "access-control-expose-headers");
        for h in [
            "x-litegen-dropped-params",
            "x-litegen-conversion-losses",
            "x-ratelimit-limit",
            "x-ratelimit-remaining",
            "x-ratelimit-reset",
            "retry-after",
            "idempotent-replayed",
            "content-disposition",
        ] {
            assert!(exposed.iter().any(|e| e == h), "{h} not in {exposed:?}");
        }
    }

    #[tokio::test]
    async fn a_browser_may_send_every_header_the_sdks_set_with_every_method_they_use() {
        let sent = ["idempotency-key", "x-litegen-org-id", "x-litegen-app-id", "x-litegen-end-user", "x-litegen-tags", "x-csrf-token", "x-litegen-strict"];
        let resp = answer(
            Request::options("/v1/x")
                .header("origin", ORIGIN)
                .header("access-control-request-method", "PUT")
                .header("access-control-request-headers", sent.join(","))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert!(listed(&resp, "access-control-allow-methods").iter().any(|m| m == "put"), "{:?}", resp.headers());
        let allowed = listed(&resp, "access-control-allow-headers");
        for h in sent {
            assert!(allowed.iter().any(|a| a == h), "{h} not in {allowed:?}");
        }
    }

    #[tokio::test]
    async fn no_origin_is_allowed_unless_configured_and_an_override_replaces_the_list() {
        let config = CorsConfig { allowed_origins: vec![], allow_credentials: false };
        let app = axum::Router::new().route("/v1/x", axum::routing::get(|| async { "ok" }));
        let get = |app: axum::Router| async move {
            app.oneshot(Request::get("/v1/x").header("origin", ORIGIN).body(Body::empty()).unwrap()).await.unwrap()
        };
        let denied = get(app.clone().layer(layer(&config, None))).await;
        assert!(denied.headers().get("access-control-allow-origin").is_none());
        let allowed = get(app.layer(layer(&config, Some(&format!("https://other.example, {ORIGIN}"))))).await;
        assert_eq!(allowed.headers()["access-control-allow-origin"], ORIGIN);
    }
}
