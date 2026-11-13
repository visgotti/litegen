//! The CORS policy: which origins may call litegen from a browser (any
//! request header, as LiteLLM allows), and which response headers their
//! scripts may read (L19: an SDK that shows headers).

use axum::http::{header, HeaderName, HeaderValue, Method};
use tower_http::cors::{AllowHeaders, AllowOrigin, Any, CorsLayer};

use crate::config::CorsConfig;

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

/// The browser origins the deployment allows: `LITEGEN_CORS_ORIGINS` (CSV)
/// when set, else `cors.allowed_origins`. `"*"` in the list means any origin.
pub fn configured_origins(config: &CorsConfig, origins_override: Option<&str>) -> Vec<String> {
    match origins_override.filter(|s| !s.is_empty()) {
        Some(csv) => csv.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        None => config.allowed_origins.clone(),
    }
}

/// The CORS layer for `config`. `origins_override` (`LITEGEN_CORS_ORIGINS`,
/// a CSV) replaces the configured origins when set: `*` allows any origin,
/// none denies every cross-origin call. Credentials are allowed only with an
/// explicit origin list, never with `*`. OAuth's public endpoints answer any
/// origin regardless ([`crate::mcp::oauth::is_public_path`]).
pub fn layer(config: &CorsConfig, origins_override: Option<&str>) -> CorsLayer {
    let methods = [Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS];
    let origins = configured_origins(config, origins_override);
    let layer = CorsLayer::new()
        .allow_methods(methods.to_vec())
        // Any request header, as LiteLLM allows, so a customer's own auth
        // header reaches a generation hook (docs/hooks.md). Mirrored rather
        // than `*`, which browsers refuse alongside credentials.
        .allow_headers(AllowHeaders::mirror_request())
        .expose_headers(exposed_response_headers());
    let wildcard = origins.iter().any(|o| o == "*");
    let layer = if wildcard {
        layer.allow_origin(Any)
    } else {
        let allowed: Vec<HeaderValue> = origins.iter().filter_map(|o| HeaderValue::from_str(o).ok()).collect();
        layer.allow_origin(AllowOrigin::predicate(move |origin, request| {
            allowed.contains(origin) || crate::mcp::oauth::is_public_path(request.uri.path())
        }))
    };
    match (config.allow_credentials, wildcard) {
        (true, false) => layer.allow_credentials(true),
        (true, true) => {
            tracing::warn!("cors: credentials are never allowed with a * origin; list the origins to allow them");
            layer
        }
        (false, _) => layer,
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

    /// A customer's own auth header, for a generation hook to read
    /// (`docs/hooks.md`), passes the preflight: litegen allows any request
    /// header, as LiteLLM does.
    #[tokio::test]
    async fn a_browser_may_send_any_request_header() {
        let resp = answer(
            Request::options("/v1/x")
                .header("origin", ORIGIN)
                .header("access-control-request-method", "PUT")
                .header("access-control-request-headers", "x-acme-token, authorization")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let allowed = listed(&resp, "access-control-allow-headers");
        for h in ["x-acme-token", "authorization"] {
            assert!(allowed.iter().any(|a| a == h), "{h} not in {allowed:?}");
        }
    }

    /// As LiteLLM switches credentials off under a `*` origin: a browser must
    /// never send its cookies to an origin-blind server.
    #[tokio::test]
    async fn credentials_are_never_allowed_with_a_wildcard_origin() {
        let config = CorsConfig { allowed_origins: vec!["*".into()], allow_credentials: true };
        let app = axum::Router::new().route("/v1/x", axum::routing::get(|| async { "ok" })).layer(layer(&config, None));
        let resp = app.oneshot(Request::get("/v1/x").header("origin", ORIGIN).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.headers()["access-control-allow-origin"], "*");
        assert!(resp.headers().get("access-control-allow-credentials").is_none(), "{:?}", resp.headers());
    }

    #[tokio::test]
    async fn any_origin_may_use_the_oauth_endpoints_and_nothing_else() {
        // An MCP client in a browser (MCP Inspector) signs in from its own origin.
        let elsewhere = "http://localhost:6274";
        let config = CorsConfig { allowed_origins: vec![ORIGIN.into()], allow_credentials: true };
        let ok = || axum::routing::post(|| async { "ok" }).get(|| async { "ok" });
        let app = axum::Router::new()
            .route("/oauth/register", ok())
            .route("/oauth/token", ok())
            .route("/.well-known/oauth-authorization-server", ok())
            .route("/oauth/authorize", ok())
            .route("/v1/x", ok())
            .layer(layer(&config, None));
        let preflight = |path: &str| {
            Request::options(path)
                .header("origin", elsewhere)
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "content-type")
                .body(Body::empty())
                .unwrap()
        };
        for path in ["/oauth/register", "/oauth/token", "/.well-known/oauth-authorization-server"] {
            let resp = app.clone().oneshot(preflight(path)).await.unwrap();
            assert_eq!(resp.headers().get("access-control-allow-origin").map(|v| v.to_str().unwrap()), Some(elsewhere), "{path}");
        }
        for path in ["/v1/x", "/oauth/authorize"] {
            let resp = app.clone().oneshot(preflight(path)).await.unwrap();
            assert!(resp.headers().get("access-control-allow-origin").is_none(), "{path} stays closed to other origins");
        }
    }
}
