//! LiteGen as an MCP server. Two Streamable HTTP endpoints, on LiteLLM's
//! paths: `/mcp` serves the AI API (generate, quote, poll, models,
//! generations) and `/admin/mcp` the management API (keys, budgets, end users,
//! spend, logs, orgs, gateway). Every tool call is replayed as an internal HTTP
//! request through the real API router, so MCP runs with exactly the
//! permissions the HTTP API enforces.

pub mod server;

use std::sync::Arc;
use std::time::Duration;

use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::api::middleware::AppState;

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
    pub http: reqwest::Client,
}

/// The `/mcp` and `/admin/mcp` routes, as the config enables them. `api` is the
/// HTTP API router without these routes: the router tool calls go through.
pub fn router(state: Arc<AppState>, api: axum::Router) -> axum::Router {
    let config = state.router.config.mcp.clone();
    let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap_or_default();
    let shared = Arc::new(McpShared { state, api, http });

    let mut routes = axum::Router::new();
    if !config.enabled {
        return routes;
    }
    routes = routes.route_service(Surface::Ai.path(), service(Surface::Ai, shared.clone()));
    if config.admin_enabled {
        routes = routes.route_service(Surface::Admin.path(), service(Surface::Admin, shared));
    }
    routes
}

fn service(surface: Surface, shared: Arc<McpShared>) -> StreamableHttpService<server::LiteGenMcp, NeverSessionManager> {
    // Stateless: no session ids, so any replica can serve any request, and
    // plain JSON responses unless a tool streams progress.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(16 * 1024 * 1024);
    StreamableHttpService::new(
        move || Ok(server::LiteGenMcp { surface, shared: shared.clone() }),
        Arc::new(NeverSessionManager::default()),
        config,
    )
}
