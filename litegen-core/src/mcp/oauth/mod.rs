//! LiteGen as the OAuth 2.1 authorization server for its two MCP endpoints,
//! following the MCP authorization spec: discovery documents, client
//! registration, a dashboard consent step, and opaque tokens that each work
//! on one endpoint only. ChatGPT and most claude.ai users cannot send an API
//! key, so for them this is the only way in.

pub mod authorize;
pub mod clients;
pub mod crypto;
pub mod discovery;
pub mod principal;
pub mod sql;
pub mod store;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;
pub mod token;

use std::future::Future;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;

use crate::api::middleware::AppState;
use crate::db::DatabaseStore;
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
        .route("/oauth/token", post(token::token).layer(DefaultBodyLimit::max(token::FORM_MAX_BYTES)))
        .route("/oauth/revoke", post(token::revoke).layer(DefaultBodyLimit::max(token::FORM_MAX_BYTES)))
        .with_state(shared)
        .merge(discovery::routes(state))
}

/// Every hour, delete what OAuth no longer needs: requests and tokens that
/// expired (or tokens revoked) more than a day ago, and revoked grants with
/// no tokens left. The day keeps a replayed code or refresh token
/// recognizable after it expires.
pub fn spawn_purger(db: Arc<dyn DatabaseStore>, shutdown: impl Future<Output = ()> + Send + 'static) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let Some(store) = db.mcp_oauth() else { return };
                    match store.purge_expired(chrono::Utc::now() - chrono::Duration::days(1)).await {
                        Ok(0) => {}
                        Ok(removed) => tracing::info!(removed, "purged expired MCP OAuth rows"),
                        Err(e) => tracing::warn!(error = %e, "MCP OAuth purge failed"),
                    }
                }
                _ = &mut shutdown => return,
            }
        }
    })
}

/// The OAuth paths any browser origin may call: the discovery documents and
/// the endpoints a public client posts to. None reads a cookie, and an MCP
/// client running in a browser (MCP Inspector) signs in through them, so
/// they answer CORS from anywhere, as the MCP TypeScript SDK's own
/// authorization server does. `/oauth/authorize` is a page, not among them.
pub fn is_public_path(path: &str) -> bool {
    path.starts_with("/.well-known/oauth-") || matches!(path, "/oauth/register" | "/oauth/token" | "/oauth/revoke")
}
