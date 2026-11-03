//! LiteGen as the OAuth 2.1 authorization server for its two MCP endpoints,
//! following the MCP authorization spec: discovery documents, client
//! registration, a dashboard consent step, and opaque tokens that each work
//! on one endpoint only. ChatGPT and most claude.ai users cannot send an API
//! key, so for them this is the only way in.

pub mod discovery;
pub mod sql;
pub mod store;
#[cfg(test)]
mod store_tests;

/// The OAuth paths any browser origin may call: the discovery documents and
/// the endpoints a public client posts to. None reads a cookie, and an MCP
/// client running in a browser (MCP Inspector) signs in through them, so
/// they answer CORS from anywhere, as the MCP TypeScript SDK's own
/// authorization server does. `/oauth/authorize` is a page, not among them.
pub fn is_public_path(path: &str) -> bool {
    path.starts_with("/.well-known/oauth-") || matches!(path, "/oauth/register" | "/oauth/token" | "/oauth/revoke")
}
