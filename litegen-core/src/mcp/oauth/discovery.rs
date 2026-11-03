//! How a client finds out how to sign in: protected-resource metadata
//! (RFC 9728) for each MCP endpoint, authorization-server metadata
//! (RFC 8414), and the `WWW-Authenticate` challenge that points at them.
//!
//! The issuer is `server.public_base_url`, path prefix included
//! (`https://app.litegen.ai/api`). Clients derive the documents' URLs
//! differently, so each is served at every path one may ask: the bare
//! well-known path, and the well-known path followed by the resource's or the
//! issuer's own path.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::api::middleware::AppState;
use crate::config::Mode;
use crate::mcp::Surface;

/// The authorization server's issuer: `server.public_base_url`.
pub fn issuer(state: &AppState) -> String {
    state.router.config.server.public_base_url()
}

/// An endpoint's resource identifier: its public URL.
pub fn resource_url(state: &AppState, surface: Surface) -> String {
    format!("{}{}", issuer(state), surface.path())
}

/// The scopes a grant for an endpoint may hold. Each names its endpoint, so a
/// request that only sends the bare origin as `resource` still says which.
pub fn scopes(surface: Surface) -> &'static [&'static str] {
    match surface {
        Surface::Ai => &["ai:generate", "ai:read"],
        Surface::Admin => &["admin:read", "admin:write"],
    }
}

/// The endpoints this deployment serves.
pub fn surfaces(state: &AppState) -> Vec<Surface> {
    let mcp = &state.router.config.mcp;
    [Surface::Ai, Surface::Admin].into_iter().filter(|s| *s == Surface::Ai || mcp.admin_enabled).collect()
}

/// `WWW-Authenticate` for a 401 on the MCP request `path` (`/mcp`,
/// `/admin/mcp/`, …): where its protected-resource metadata is and which
/// scopes it takes, plus `error` when a token was refused. The metadata URL
/// keeps the client's trailing slash, so the `resource` it names is the URL
/// the client called (RFC 9728 §3.3). Without OAuth, a plain Bearer challenge.
pub fn challenge_header(state: &AppState, path: &str, error: Option<&str>) -> String {
    match Surface::from_path(path) {
        Some(surface) if state.router.config.mcp.oauth.enabled => {
            let mut value = format!(
                r#"Bearer resource_metadata="{}/.well-known/oauth-protected-resource{path}", scope="{}""#,
                issuer(state),
                scopes(surface).join(" ")
            );
            if let Some(error) = error {
                value.push_str(&format!(r#", error="{error}""#));
            }
            value
        }
        _ => r#"Bearer realm="litegen""#.to_string(),
    }
}

/// The discovery documents, served outside MCP's authentication.
pub fn routes(state: Arc<AppState>) -> Router {
    warn_if_unreachable(&state);
    Router::new()
        .route("/.well-known/oauth-protected-resource", get(protected_resource_root))
        .route("/.well-known/oauth-protected-resource/{*rest}", get(protected_resource))
        .route("/.well-known/oauth-authorization-server", get(authorization_server))
        .route("/.well-known/oauth-authorization-server/{*rest}", get(authorization_server))
        .with_state(state)
}

/// A hosted deployment whose issuer is loopback cannot be signed in to from
/// ChatGPT or claude.ai: say so at startup.
fn warn_if_unreachable(state: &AppState) {
    let issuer = issuer(state);
    let host = reqwest::Url::parse(&issuer).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    let loopback = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()) || host == "[::1]";
    if state.mode == Mode::Hosted && loopback {
        tracing::warn!(
            issuer = %issuer,
            "MCP OAuth is on but server.public_base_url is a loopback address: hosted clients (ChatGPT, claude.ai) cannot sign in; set it to the public HTTPS URL"
        );
    }
}

async fn protected_resource_root(State(state): State<Arc<AppState>>) -> Response {
    resource_document(&state, "")
}

async fn protected_resource(State(state): State<Arc<AppState>>, Path(rest): Path<String>) -> Response {
    resource_document(&state, &rest)
}

/// The document for the resource whose URL path ends with `rest`: the
/// management endpoint when it ends in `admin/mcp`, otherwise the AI one.
/// `resource` keeps a trailing slash the client called with.
fn resource_document(state: &AppState, rest: &str) -> Response {
    let (path, slash) = match rest.strip_suffix('/') {
        Some(path) => (path, "/"),
        None => (rest, ""),
    };
    let surface = if path.ends_with("admin/mcp") { Surface::Admin } else { Surface::Ai };
    if !surfaces(state).contains(&surface) {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(json!({
        "resource": format!("{}{slash}", resource_url(state, surface)),
        "authorization_servers": [issuer(state)],
        "scopes_supported": scopes(surface),
        "bearer_methods_supported": ["header"],
        "resource_name": surface.title(),
    }))
    .into_response()
}

async fn authorization_server(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let issuer = issuer(&state);
    let scopes: Vec<&str> = surfaces(&state).into_iter().flat_map(|s| scopes(s).iter().copied()).collect();
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "registration_endpoint": format!("{issuer}/oauth/register"),
        "revocation_endpoint": format!("{issuer}/oauth/revoke"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "revocation_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": scopes,
        "client_id_metadata_document_supported": true,
        "authorization_response_iss_parameter_supported": true,
    }))
}
