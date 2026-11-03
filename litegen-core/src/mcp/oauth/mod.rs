//! LiteGen as the OAuth 2.1 authorization server for its two MCP endpoints,
//! following the MCP authorization spec: discovery documents, client
//! registration, a dashboard consent step, and opaque tokens that each work
//! on one endpoint only. ChatGPT and most claude.ai users cannot send an API
//! key, so for them this is the only way in.

pub mod authorize;
pub mod clients;
pub mod crypto;
pub mod discovery;
pub mod sql;
pub mod store;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;

use crate::api::middleware::AppState;
use clients::{MetadataFetcher, PublicFetcher};

/// What the OAuth endpoints share.
pub struct OAuthShared {
    pub state: Arc<AppState>,
    /// Fetches clients' metadata documents.
    pub fetcher: Arc<dyn MetadataFetcher>,
}

/// Whether this deployment offers OAuth: switched on, and its database has
/// the OAuth tables (test doubles do not).
pub fn available(state: &AppState) -> bool {
    state.router.config.mcp.oauth.enabled && state.db.mcp_oauth().is_some()
}

/// The discovery documents and the authorization server's endpoints, all
/// outside MCP's authentication.
pub fn routes(state: Arc<AppState>) -> Router {
    let shared = Arc::new(OAuthShared { state: state.clone(), fetcher: Arc::new(PublicFetcher) });
    Router::new()
        .route("/oauth/register", post(clients::register).layer(DefaultBodyLimit::max(clients::REGISTER_MAX_BYTES)))
        .route("/oauth/authorize", get(authorize::authorize))
        .with_state(shared)
        .merge(discovery::routes(state))
}

/// The OAuth paths any browser origin may call: the discovery documents and
/// the endpoints a public client posts to. None reads a cookie, and an MCP
/// client running in a browser (MCP Inspector) signs in through them, so
/// they answer CORS from anywhere, as the MCP TypeScript SDK's own
/// authorization server does. `/oauth/authorize` is a page, not among them.
pub fn is_public_path(path: &str) -> bool {
    path.starts_with("/.well-known/oauth-") || matches!(path, "/oauth/register" | "/oauth/token" | "/oauth/revoke")
}
