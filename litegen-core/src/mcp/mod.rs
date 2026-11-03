//! LiteGen as an MCP server. Two Streamable HTTP endpoints, on LiteLLM's
//! paths: `/mcp` serves the AI API (generate, quote, poll, models,
//! generations) and `/admin/mcp` the management API (keys, budgets, end users,
//! spend, logs, orgs, gateway). Every tool call is replayed as an internal HTTP
//! request through the real API router, so MCP runs with exactly the
//! permissions the HTTP API enforces.

pub mod auth;
pub mod catalog;
pub mod content;
pub mod dispatch;
pub mod gate;
pub mod oauth;
pub mod registry;
pub mod schema;
pub mod server;
pub mod wait;
#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::api::middleware::AppState;
use auth::OriginPolicy;
use registry::Registry;

/// The largest MCP request body: room for a base64-encoded mesh upload.
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// The tool registry: built from the OpenAPI document once per process. A
/// tool whose schema cannot be built stops startup, naming the tool.
pub fn tool_registry() -> Arc<Registry> {
    static REGISTRY: OnceLock<Arc<Registry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            use utoipa::OpenApi;
            let doc = serde_json::to_value(crate::api::openapi::ApiDoc::openapi()).expect("OpenAPI document serializes");
            Arc::new(Registry::build(&doc).unwrap_or_else(|e| panic!("{e}")))
        })
        .clone()
}

/// Which of the two MCP endpoints a request came in on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    /// `/mcp`: the AI API.
    Ai,
    /// `/admin/mcp`: the management API.
    Admin,
}

impl Surface {
    pub fn path(self) -> &'static str {
        match self {
            Surface::Ai => "/mcp",
            Surface::Admin => "/admin/mcp",
        }
    }

    /// The endpoint a request path names, with or without a trailing slash.
    pub fn from_path(path: &str) -> Option<Surface> {
        let path = path.strip_suffix('/').unwrap_or(path);
        [Surface::Ai, Surface::Admin].into_iter().find(|s| s.path() == path)
    }

    /// The endpoint's name in storage, metrics and the consent API.
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Ai => "ai",
            Surface::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Option<Surface> {
        [Surface::Ai, Surface::Admin].into_iter().find(|surface| surface.as_str() == s)
    }

    /// The `?toolsets=` names a client may pick on this endpoint.
    pub fn toolsets(self) -> &'static [&'static str] {
        match self {
            Surface::Ai => &["images", "videos", "models3d", "catalog", "generations", "account"],
            Surface::Admin => &[
                "keys", "budgets", "end_users", "spend", "logs", "audit", "orgs", "members", "apps", "users",
                "gateway", "system", "account",
            ],
        }
    }

    /// The usage guide returned by `initialize` and `server/discover`.
    pub fn instructions(self) -> &'static str {
        match self {
            Surface::Ai => include_str!("instructions/ai.md"),
            Surface::Admin => include_str!("instructions/admin.md"),
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Surface::Ai => "LiteGen",
            Surface::Admin => "LiteGen admin",
        }
    }
}

/// What every MCP request handler shares: the app state, the API router tool
/// calls are dispatched through, and an HTTP client for fetching previews.
pub struct McpShared {
    pub state: Arc<AppState>,
    pub api: axum::Router,
    pub registry: Arc<Registry>,
    pub http: reqwest::Client,
    pub origins: OriginPolicy,
}

/// The `/mcp` and `/admin/mcp` routes, as the config enables them. `api` is the
/// HTTP API router without these routes: the router tool calls go through.
pub fn router(state: Arc<AppState>, api: axum::Router) -> axum::Router {
    let config = state.router.config.clone();
    let public = reqwest::Url::parse(&config.server.public_base_url()).ok();

    // Browsers may call MCP from the CORS origins, LiteGen's own public origin
    // and any origin the operator adds; everything else gets a 403.
    let mut origins = crate::api::cors::configured_origins(&config.cors, std::env::var("LITEGEN_CORS_ORIGINS").ok().as_deref());
    if let Some(url) = &public {
        origins.push(url.origin().ascii_serialization());
    }
    origins.extend(config.mcp.allowed_origins.iter().cloned());

    // The Host check guards a local server against DNS rebinding; a public
    // deployment's own host must pass.
    let hosts = if config.mcp.allowed_hosts.iter().any(|h| h == "*") {
        None
    } else {
        let mut hosts: Vec<String> = ["localhost", "127.0.0.1", "::1"].map(String::from).to_vec();
        if let Some(host) = public.as_ref().and_then(|u| u.host_str()) {
            hosts.push(host.to_string());
            if let Some(port) = public.as_ref().and_then(|u| u.port()) {
                hosts.push(format!("{host}:{port}"));
            }
        }
        hosts.extend(config.mcp.allowed_hosts.iter().cloned());
        Some(hosts)
    };

    let shared = Arc::new(McpShared { state, api, registry: tool_registry(), http: preview_client(), origins: OriginPolicy::new(origins) });

    let mut routes = axum::Router::new();
    if !config.mcp.enabled {
        return routes;
    }
    routes = mount(routes, Surface::Ai, service(Surface::Ai, shared.clone(), hosts.clone()));
    if config.mcp.admin_enabled {
        routes = mount(routes, Surface::Admin, service(Surface::Admin, shared.clone(), hosts));
    }
    routes.layer(axum::middleware::from_fn_with_state(shared, auth::mcp_auth))
}

/// An endpoint at its path and with a trailing slash: people and clients
/// paste either, and one form must not 404 while the other works.
fn mount(routes: axum::Router, surface: Surface, svc: StreamableHttpService<server::LiteGenMcp, NeverSessionManager>) -> axum::Router {
    routes.route_service(surface.path(), svc.clone()).route_service(&format!("{}/", surface.path()), svc)
}

/// The client that fetches external images for previews. It never follows a
/// redirect: a URL passes `validate_public_url` first, and a public host must
/// not be able to 3xx the fetch into a private address.
pub(crate) fn preview_client() -> reqwest::Client {
    crate::util::ssrf::no_redirect_client_with_timeout(Duration::from_secs(10))
}

fn service(
    surface: Surface,
    shared: Arc<McpShared>,
    hosts: Option<Vec<String>>,
) -> StreamableHttpService<server::LiteGenMcp, NeverSessionManager> {
    // Stateless: no session ids, so any replica can serve any request, and
    // plain JSON responses unless a tool streams progress. Origin is checked by
    // `auth::mcp_auth`, so rmcp's own Origin check stays off.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(MAX_BODY_BYTES);
    let config = match hosts {
        Some(hosts) => config.with_allowed_hosts(hosts),
        None => config.disable_allowed_hosts(),
    };
    StreamableHttpService::new(
        move || Ok(server::LiteGenMcp { surface, shared: shared.clone() }),
        Arc::new(NeverSessionManager::default()),
        config,
    )
}
