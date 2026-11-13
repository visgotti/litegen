//! LiteGen as the OAuth 2.1 authorization server for its two MCP endpoints,
//! following the MCP authorization spec: discovery documents, client
//! registration, a dashboard consent step, and opaque tokens that each work
//! on one endpoint only. ChatGPT and most claude.ai users cannot send an API
//! key, so for them this is the only way in.

pub mod assertion;
pub mod authorize;
pub mod clients;
pub mod crypto;
pub mod discovery;
pub mod grants;
pub mod limit;
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
    /// Fetches clients' metadata documents and key sets.
    pub fetcher: Arc<dyn MetadataFetcher>,
    /// The key sets of clients that sign their token requests.
    pub keys: assertion::KeyCache,
    /// What each address has left of its sign-in allowance.
    pub limiter: limit::AddressLimiter,
}

/// Whether this deployment offers OAuth: switched on, and its database has
/// the OAuth tables (test doubles do not).
pub fn available(state: &AppState) -> bool {
    state.router.config.mcp.oauth.enabled && state.db.mcp_oauth().is_some()
}

/// The discovery documents and the authorization server's endpoints, all
/// outside MCP's authentication.
pub fn routes(state: Arc<AppState>) -> Router {
    let limiter = state.router.shared().map_or_else(limit::AddressLimiter::default, |s| limit::AddressLimiter::with_shared(s.clone()));
    let shared = Arc::new(OAuthShared { state: state.clone(), fetcher: Arc::new(PublicFetcher), keys: Default::default(), limiter });
    let limited = axum::middleware::from_fn_with_state(shared.clone(), limit::by_address);
    Router::new()
        .route("/oauth/register", post(clients::register).layer(DefaultBodyLimit::max(clients::REGISTER_MAX_BYTES)).layer(limited.clone()))
        .route("/oauth/authorize", get(authorize::authorize).layer(limited))
        .route("/oauth/token", post(token::token).layer(DefaultBodyLimit::max(token::FORM_MAX_BYTES)))
        .route("/oauth/revoke", post(token::revoke).layer(DefaultBodyLimit::max(token::FORM_MAX_BYTES)))
        .with_state(shared)
        .merge(discovery::routes(state))
}

/// Every hour, delete what OAuth no longer needs (see
/// [`store::McpOAuthStore::purge_expired`]). Spent codes and tokens stay a
/// day, so a replay is still recognized after they expire.
pub fn spawn_purger(db: Arc<dyn DatabaseStore>, shutdown: impl Future<Output = ()> + Send + 'static) -> tokio::task::JoinHandle<()> {
    spawn_purge_loop(std::time::Duration::from_secs(3600), shutdown, move || {
        let db = db.clone();
        async move {
            let Some(store) = db.mcp_oauth() else { return false };
            match store.purge_expired(chrono::Utc::now()).await {
                Ok(0) => {}
                Ok(removed) => tracing::info!(removed, "purged expired MCP OAuth rows"),
                Err(e) => tracing::warn!(error = %e, "MCP OAuth purge failed"),
            }
            true
        }
    })
}

/// [`spawn_purger`]'s loop: one `purge` every `every` (a test passes its own)
/// until `shutdown`, or until a purge answers `false` (a database with no
/// OAuth store has nothing to purge).
fn spawn_purge_loop<P, F>(
    every: std::time::Duration,
    shutdown: impl Future<Output = ()> + Send + 'static,
    mut purge: P,
) -> tokio::task::JoinHandle<()>
where
    P: FnMut() -> F + Send + 'static,
    F: Future<Output = bool> + Send,
{
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    // A purge that panics is logged by the guard; the next
                    // tick purges again.
                    if crate::supervise::guarded("an MCP OAuth purge", purge()).await == Some(false) {
                        return;
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
