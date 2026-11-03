//! Who is calling an MCP endpoint. Credentials resolve exactly as the HTTP
//! API's own: an API key (or the master key) as a Bearer token, or nothing in
//! dev mode. Session cookies are never read here: a web page could otherwise
//! drive a signed-in browser's session through MCP.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::Instrument;

use super::oauth::discovery;
use super::oauth::principal::{resolve_access_token, AccessError};
use super::oauth::token::ACCESS_PREFIX;
use super::{McpShared, Surface};
use crate::api::middleware::{dev_bypass_context, missing_credentials_response, resolve_api_key, AppState, KeyContext};

/// How the caller authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerKind {
    /// A DB-backed API key.
    ApiKey,
    /// The deployment's master key.
    MasterKey,
    /// Dev mode: single-tenant with no master key, so no credentials at all.
    Dev,
    /// An OAuth access token: the person who approved the app, as their role
    /// lets them, within the grant's org and scopes.
    OAuth,
}

/// The authenticated caller of one MCP request, put into the request's
/// extensions for the handler.
#[derive(Debug, Clone)]
pub struct Caller {
    pub ctx: KeyContext,
    pub kind: CallerKind,
    /// The caller's `Authorization` header, replayed on every tool call.
    pub authorization: Option<HeaderValue>,
    pub surface: Surface,
    /// The `?toolsets=` the client asked for; `None` means all.
    pub toolsets: Option<Vec<String>>,
}

/// Which browser origins may call MCP.
#[derive(Debug, Clone)]
pub struct OriginPolicy {
    pub any: bool,
    pub allowed: Vec<String>,
}

impl OriginPolicy {
    pub fn new(origins: impl IntoIterator<Item = String>) -> Self {
        let allowed: Vec<String> = origins.into_iter().map(|o| normalize_origin(&o)).collect();
        OriginPolicy { any: allowed.iter().any(|o| o == "*"), allowed }
    }

    pub fn allows(&self, origin: &str) -> bool {
        self.any || self.allowed.iter().any(|o| *o == normalize_origin(origin))
    }
}

fn normalize_origin(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// The `toolsets` query parameter, checked against the endpoint's toolsets;
/// the error is the message for a 400.
fn requested_toolsets(req: &Request, surface: Surface) -> Result<Option<Vec<String>>, String> {
    let query = Query::<HashMap<String, String>>::try_from_uri(req.uri()).map(|q| q.0).unwrap_or_default();
    let Some(raw) = query.get("toolsets") else { return Ok(None) };
    let names: Vec<String> = raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if names.is_empty() {
        return Ok(None);
    }
    if let Some(unknown) = names.iter().find(|n| !surface.toolsets().contains(&n.as_str())) {
        return Err(format!("unknown toolset '{unknown}'; valid toolsets: {}", surface.toolsets().join(", ")));
    }
    Ok(Some(names))
}

/// 401 that tells an MCP client on `path` how to authenticate (see
/// [`discovery::challenge_header`]), with the API's JSON error. `error` names
/// why a token was refused.
pub fn challenge(state: &AppState, path: &str, body: Option<Response>, error: Option<&str>) -> Response {
    let mut resp = body.unwrap_or_else(missing_credentials_response);
    *resp.status_mut() = StatusCode::UNAUTHORIZED;
    if let Ok(value) = HeaderValue::from_str(&discovery::challenge_header(state, path, error)) {
        resp.headers_mut().insert(header::WWW_AUTHENTICATE, value);
    }
    resp
}

/// Authenticate an MCP request: Origin, then `?toolsets=`, then credentials.
pub async fn mcp_auth(State(shared): State<Arc<McpShared>>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let Some(surface) = Surface::from_path(&path) else {
        return next.run(req).await;
    };

    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let allowed = origin.to_str().map(|o| shared.origins.allows(o)).unwrap_or(false);
        if !allowed {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": { "message": "Origin not allowed", "type": "forbidden", "code": "forbidden_origin" } })),
            )
                .into_response();
        }
    }

    let toolsets = match requested_toolsets(&req, surface) {
        Ok(t) => t,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": { "message": message, "type": "unknown_toolset", "code": "unknown_toolset" } })),
            )
                .into_response()
        }
    };

    // The API accepts the token with or without the `Bearer ` prefix.
    let authorization = req.headers().get(header::AUTHORIZATION).cloned();
    let token = authorization
        .as_ref()
        .and_then(|v| v.to_str().ok())
        .map(|v| v.strip_prefix("Bearer ").unwrap_or(v).trim().to_string())
        .filter(|t| !t.is_empty());

    let (ctx, kind) = match token {
        Some(token) if token.starts_with(ACCESS_PREFIX) => match resolve_access_token(&shared.state, &token, surface).await {
            Ok(ctx) => (ctx, CallerKind::OAuth),
            Err(AccessError::Invalid(reason)) => {
                let body = (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({ "error": { "message": reason, "type": "invalid_token", "code": "invalid_token" } })),
                )
                    .into_response();
                return challenge(&shared.state, &path, Some(body), Some("invalid_token"));
            }
            Err(AccessError::Store(e)) => {
                tracing::error!(error = %e, "resolving an MCP OAuth token failed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "error": { "message": "try again", "type": "service_unavailable", "code": "service_unavailable" } })),
                )
                    .into_response();
            }
        },
        Some(token) => match resolve_api_key(&shared.state, &token).await {
            Ok(ctx) => {
                let kind = if ctx.key_id.is_some() {
                    CallerKind::ApiKey
                } else if shared.state.master_key.is_some() {
                    CallerKind::MasterKey
                } else {
                    CallerKind::Dev
                };
                (ctx, kind)
            }
            Err(resp) if resp.status() == StatusCode::UNAUTHORIZED => return challenge(&shared.state, &path, Some(resp), None),
            Err(resp) => return resp,
        },
        None => match dev_bypass_context(&shared.state) {
            Some(ctx) => (ctx, CallerKind::Dev),
            None => return challenge(&shared.state, &path, None, None),
        },
    };

    // Tool calls replay the API key; an OAuth caller's resolved principal
    // travels as a request extension instead (see `dispatch::authorized`).
    let authorization = match kind {
        CallerKind::Dev | CallerKind::OAuth => None,
        _ => authorization,
    };
    req.extensions_mut().insert(Caller { ctx, kind, authorization, surface, toolsets });

    // Read the body once to name the JSON-RPC method for metrics and tracing,
    // then hand it on unchanged.
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, super::MAX_BODY_BYTES).await else {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({ "error": { "message": "The request is over MCP's 16 MiB limit", "type": "payload_too_large", "code": "payload_too_large" } })),
        )
            .into_response();
    };
    let method = rpc_method(&bytes);
    metrics::counter!("litegen_mcp_requests_total", "surface" => surface.as_str(), "method" => method).increment(1);
    let span = tracing::info_span!("mcp.request", method, surface = surface.as_str(), caller = ?kind);
    next.run(Request::from_parts(parts, axum::body::Body::from(bytes))).instrument(span).await
}

/// The JSON-RPC method a request names, from a fixed list (a client must not
/// be able to mint metric labels).
fn rpc_method(body: &[u8]) -> &'static str {
    const KNOWN: [&str; 14] = [
        "initialize",
        "notifications/initialized",
        "server/discover",
        "ping",
        "tools/list",
        "tools/call",
        "notifications/cancelled",
        "resources/list",
        "resources/templates/list",
        "resources/read",
        "prompts/list",
        "prompts/get",
        "completion/complete",
        "subscriptions/listen",
    ];
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else { return "invalid" };
    if value.is_array() {
        return "batch";
    }
    let method = value.get("method").and_then(serde_json::Value::as_str).unwrap_or("");
    KNOWN.iter().find(|m| **m == method).copied().unwrap_or("other")
}
