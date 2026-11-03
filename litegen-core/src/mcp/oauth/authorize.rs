//! Authorization. `GET /oauth/authorize` checks a client's request, keeps it
//! for ten minutes and sends the browser to the dashboard's consent page.
//! There the signed-in person sees who is asking for what, picks an org, an
//! app and scopes, and approves or denies through the consent API
//! (`/v1/oauth/requests/{id}`, `…/approve`, `…/deny`), which answers with
//! where to send the browser back.
//!
//! A request that names an unknown client or an undeclared redirect URI, or
//! names either twice, gets an error page: LiteGen never redirects to a URI it
//! has not verified. Every other error, and every answer, goes back to the
//! client's redirect URI with `iss` (RFC 9207).

use std::sync::Arc;

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::clients::{redirect_allowed, resolve_client};
use super::crypto::{random_token, sha256_hex};
use super::discovery::{issuer, resource_target, scopes as surface_scopes, surfaces, ResourceTarget};
use super::store::{AuthRequest, Decision, McpOAuthStore};
use super::OAuthShared;
use crate::api::handlers::orgs::log_org_audit;
use crate::api::middleware::{AppState, KeyContext};
use crate::auth::permissions::{permissions_for, Permission};
use crate::mcp::Surface;
use crate::types::Role;

/// How long a person has to decide a request.
const REQUEST_TTL: Duration = Duration::minutes(10);
/// How long an authorization code can be redeemed.
const CODE_TTL: Duration = Duration::seconds(60);

// ─── GET /oauth/authorize ─────────────────────────────────────────────────────

/// The parameters LiteGen reads, the first of each.
#[derive(Debug, Default)]
pub struct AuthorizeParams {
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub scope: Option<String>,
    pub state: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub resource: Option<String>,
    /// Those given more than once, which RFC 6749 §3.1 does not allow.
    pub repeated: Vec<String>,
}

impl AuthorizeParams {
    fn from_pairs(pairs: &[(String, String)]) -> AuthorizeParams {
        let mut p = AuthorizeParams::default();
        for (key, value) in pairs {
            let slot = match key.as_str() {
                "response_type" => &mut p.response_type,
                "client_id" => &mut p.client_id,
                "redirect_uri" => &mut p.redirect_uri,
                "scope" => &mut p.scope,
                "state" => &mut p.state,
                "code_challenge" => &mut p.code_challenge,
                "code_challenge_method" => &mut p.code_challenge_method,
                "resource" => &mut p.resource,
                _ => continue,
            };
            if slot.is_none() {
                *slot = Some(value.clone());
            } else if !p.repeated.contains(key) {
                p.repeated.push(key.clone());
            }
        }
        p
    }

    fn is_repeated(&self, key: &str) -> bool {
        self.repeated.iter().any(|r| r == key)
    }
}

pub async fn authorize(State(shared): State<Arc<OAuthShared>>, query: Result<Query<Vec<(String, String)>>, QueryRejection>) -> Response {
    let state = &shared.state;
    let Some(store) = state.db.mcp_oauth() else { return error_page("OAuth is not available on this server.") };
    let Ok(Query(pairs)) = query else { return error_page("its parameters can't be read.") };
    let p = AuthorizeParams::from_pairs(&pairs);
    // Where to answer must be beyond doubt before anything goes back to the app.
    if let Some(key) = ["client_id", "redirect_uri"].into_iter().find(|key| p.is_repeated(key)) {
        return error_page(&format!("it gives {key} more than once."));
    }
    let Some(client_id) = p.client_id.as_deref().filter(|s| !s.is_empty()) else {
        return error_page("it names no client_id.");
    };
    let now = Utc::now();
    let client = match resolve_client(store, shared.fetcher.as_ref(), client_id, now).await {
        Ok(client) => client,
        Err(e) => return error_page(&e.to_string()),
    };
    let redirect_uri = match p.redirect_uri.as_deref() {
        Some(uri) if redirect_allowed(&client.redirect_uris, uri) => uri.to_string(),
        Some(uri) => return error_page(&format!("'{uri}' is not a redirect URI {} declared.", client.client_name)),
        None if client.redirect_uris.len() == 1 => client.redirect_uris[0].clone(),
        None => return error_page("it names no redirect_uri, and the app declared several."),
    };

    // The redirect URI is verified: from here on the client hears about
    // problems there.
    let back = Back { redirect_uri: &redirect_uri, state: p.state.as_deref(), issuer: issuer(state) };
    if p.is_repeated("resource") {
        return back.error("invalid_target", "name one resource: one sign-in is for one MCP endpoint");
    }
    if let Some(key) = p.repeated.first() {
        return back.error("invalid_request", &format!("{key} is given more than once"));
    }
    if p.response_type.as_deref() != Some("code") {
        return back.error("unsupported_response_type", "response_type must be code");
    }
    if p.code_challenge_method.as_deref() != Some("S256") {
        return back.error("invalid_request", "PKCE with code_challenge_method S256 is required");
    }
    let Some(challenge) = p.code_challenge.as_deref().filter(|c| valid_challenge(c)) else {
        return back.error("invalid_request", "code_challenge must be an S256 challenge: 43 base64url characters");
    };
    let requested = known_scopes(p.scope.as_deref());
    let surface = match surface_for(state, p.resource.as_deref(), &requested) {
        Ok(surface) => surface,
        Err((error, description)) => return back.error(error, description),
    };
    let mut scopes: Vec<String> = requested.into_iter().filter(|s| surface_scopes(surface).contains(&s.as_str())).collect();
    if scopes.is_empty() {
        scopes = surface_scopes(surface).iter().map(|s| s.to_string()).collect();
    }

    let request = AuthRequest {
        id: random_token("", 32),
        client_id: client.client_id,
        redirect_uri: redirect_uri.clone(),
        state: p.state.clone(),
        code_challenge: challenge.to_string(),
        scopes,
        surface,
        created_at: now,
        expires_at: now + REQUEST_TTL,
        user_id: None,
        org_id: None,
        app_id: None,
        granted_scopes: None,
        code_hash: None,
        decided_at: None,
        consumed_at: None,
    };
    if let Err(e) = store.insert_request(&request).await {
        tracing::error!(error = %e, "storing an OAuth authorization request failed");
        return back.error("server_error", "the request could not be stored");
    }
    found(&with_query(&consent_url(state), &[("request", &request.id)]))
}

/// The endpoint a request is for: its `resource` when that names an
/// endpoint, else its scopes' prefix (`ai:` or `admin:`), else the AI one.
/// One grant is for one endpoint, so mixed prefixes are refused.
fn surface_for(state: &AppState, resource: Option<&str>, scopes: &[String]) -> Result<Surface, (&'static str, &'static str)> {
    if let Some(resource) = resource {
        match resource_target(state, resource) {
            ResourceTarget::Endpoint(surface) => return Ok(surface),
            ResourceTarget::Prefix => {}
            ResourceTarget::Foreign => return Err(("invalid_target", "resource is not one of this server's MCP endpoints")),
        }
    }
    let ai = scopes.iter().any(|s| s.starts_with("ai:"));
    let admin = scopes.iter().any(|s| s.starts_with("admin:"));
    match (ai, admin) {
        (true, true) => Err(("invalid_scope", "ask for ai: or admin: scopes, not both: one sign-in is for one endpoint")),
        (false, true) if !surfaces(state).contains(&Surface::Admin) => Err(("invalid_scope", "this server has no admin endpoint")),
        (false, true) => Ok(Surface::Admin),
        _ => Ok(Surface::Ai),
    }
}

/// The scopes a request names that LiteGen knows, in order, once each.
fn known_scopes(scope: Option<&str>) -> Vec<String> {
    let known: Vec<&str> = [Surface::Ai, Surface::Admin].into_iter().flat_map(|s| surface_scopes(s).iter().copied()).collect();
    let mut out: Vec<String> = Vec::new();
    for s in scope.unwrap_or_default().split_whitespace() {
        if known.contains(&s) && !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    }
    out
}

/// An S256 challenge: base64url(SHA-256), 43 characters.
fn valid_challenge(challenge: &str) -> bool {
    challenge.len() == 43 && challenge.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The dashboard's consent page: `mcp.oauth.consent_url`, or `/oauth/consent`
/// on the public origin.
fn consent_url(state: &AppState) -> String {
    let configured = state.router.config.mcp.oauth.consent_url.as_deref().map(str::trim).filter(|s| !s.is_empty());
    match configured {
        Some(url) => url.to_string(),
        None => {
            let issuer = issuer(state);
            let origin = reqwest::Url::parse(&issuer).map(|u| u.origin().ascii_serialization()).unwrap_or(issuer);
            format!("{origin}/oauth/consent")
        }
    }
}

/// Where answers go: the client's verified redirect URI, with the request's
/// `state` and LiteGen's issuer (RFC 9207).
struct Back<'a> {
    redirect_uri: &'a str,
    state: Option<&'a str>,
    issuer: String,
}

impl Back<'_> {
    fn url(&self, params: &[(&str, &str)]) -> String {
        let mut pairs = params.to_vec();
        if let Some(state) = self.state {
            pairs.push(("state", state));
        }
        pairs.push(("iss", &self.issuer));
        with_query(self.redirect_uri, &pairs)
    }

    fn error(&self, error: &str, description: &str) -> Response {
        found(&self.url(&[("error", error), ("error_description", description)]))
    }
}

/// `url` with `pairs` added to its query.
fn with_query(url: &str, pairs: &[(&str, &str)]) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            parsed.query_pairs_mut().extend_pairs(pairs);
            parsed.to_string()
        }
        Err(_) => url.to_string(),
    }
}

/// A 302 to `location`, never cached.
fn found(location: &str) -> Response {
    (StatusCode::FOUND, [(header::LOCATION, location.to_string()), (header::CACHE_CONTROL, "no-store".to_string())]).into_response()
}

/// The page for a request LiteGen cannot answer at a verified redirect URI.
fn error_page(reason: &str) -> Response {
    page(
        StatusCode::BAD_REQUEST,
        &format!("This app's sign-in link is invalid: {}", html_escape(reason)),
        "Go back to the app and connect again. If this keeps happening, the app's LiteGen setup is wrong.",
    )
}

/// The page for a sign-in from a network that started too many at once.
pub(super) fn busy_page(wait_secs: u64) -> Response {
    page(
        StatusCode::TOO_MANY_REQUESTS,
        "Too many sign-in attempts came from your network.",
        &format!("Wait {wait_secs} seconds, then connect again from the app."),
    )
}

/// A sign-in page that ends there, never cached or framed. `problem` and
/// `advice` are HTML.
fn page(status: StatusCode, problem: &str, advice: &str) -> Response {
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>LiteGen sign-in</title>\
         <style>body{{font-family:system-ui,sans-serif;max-width:36rem;margin:4rem auto;padding:0 1rem;line-height:1.5}}</style>\
         </head><body><h1>Can't sign in to LiteGen</h1><p>{problem}</p><p>{advice}</p></body></html>"
    );
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'; style-src 'unsafe-inline'"),
        ],
        body,
    )
        .into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

// ─── The consent API ──────────────────────────────────────────────────────────

/// A pending sign-in request, as the consent page shows it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OAuthRequestView {
    pub id: String,
    pub client: OAuthClientView,
    /// The endpoint the grant would be for: `ai` (`/mcp`) or `admin` (`/admin/mcp`).
    pub surface: String,
    /// The scopes the app asked for, all of one endpoint.
    pub requested_scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
    /// The person's orgs, each with its apps and the requested scopes their
    /// role there lets them grant.
    pub orgs: Vec<OAuthOrgChoice>,
}

/// The app asking to sign in.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OAuthClientView {
    pub client_id: String,
    pub name: String,
    pub logo_uri: Option<String>,
    pub client_uri: Option<String>,
    /// The host the browser is sent back to after the decision.
    pub redirect_host: String,
    /// `cimd`: identified by its metadata document's URL. `dcr`: registered here.
    pub registration: String,
}

/// An org the person may approve the request in.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OAuthOrgChoice {
    pub id: String,
    pub name: String,
    /// The person's role in the org.
    pub role: String,
    pub apps: Vec<OAuthAppChoice>,
    /// The requested scopes the role can grant: what it could give an API key.
    pub grantable_scopes: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OAuthAppChoice {
    pub id: String,
    pub name: String,
}

/// The person's approval.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ApproveOAuthRequest {
    pub org_id: String,
    /// The app the app acts in; the org's first app when omitted.
    #[serde(default)]
    pub app_id: Option<String>,
    /// A subset of the requested scopes that the person's role can grant.
    pub scopes: Vec<String>,
}

/// Where to send the browser next.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OAuthRedirect {
    /// The app's redirect URI, carrying the code or the error, `state` and `iss`.
    pub redirect_to: String,
}

/// The scopes a person with `role` in an org may grant: the ones they could
/// give an API key there.
pub fn grantable_scopes(role: Role) -> Vec<&'static str> {
    let permissions = permissions_for(role);
    let has = |p: Permission| permissions.contains(&p);
    let mut out = Vec::new();
    if has(Permission::GenerationCreate) {
        out.push("ai:generate");
    }
    if has(Permission::GenerationReadOwn) || has(Permission::OrgRead) {
        out.extend(["ai:read", "admin:read"]);
    }
    if has(Permission::KeyWriteAny) {
        out.push("admin:write");
    }
    out
}

/// GET /v1/oauth/requests/{id} — A pending sign-in request, for the consent page.
#[utoipa::path(
    get,
    path = "/v1/oauth/requests/{id}",
    params(("id" = String, Path, description = "The request id from the consent page's `request` parameter")),
    responses(
        (status = 200, description = "The request, the app and the orgs the person can approve it in", body = OAuthRequestView),
        (status = 401, description = "No dashboard session", body = crate::types::ErrorResponse),
        (status = 404, description = "Unknown or expired", body = crate::types::ErrorResponse),
        (status = 409, description = "Already approved or denied", body = crate::types::ErrorResponse),
    ),
    tag = "OAuth"
)]
pub async fn get_request(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
) -> Result<Json<OAuthRequestView>, ConsentError> {
    let user_id = browser_user(&ctx)?;
    let (store, request) = pending(&state, &id).await?;
    let client = store.get_client(&request.client_id).await?.ok_or(ConsentError::Gone)?;
    let memberships = state.db.list_orgs_for_user(user_id).await?;
    let mut orgs = Vec::with_capacity(memberships.len());
    for (org, role) in memberships {
        let apps = state.db.list_apps_for_org(&org.id).await?;
        let grantable = grantable_scopes(role).into_iter().filter(|s| request.scopes.iter().any(|r| r == s)).map(String::from).collect();
        orgs.push(OAuthOrgChoice {
            id: org.id,
            name: org.name,
            role: role.as_str().to_string(),
            apps: apps.into_iter().map(|a| OAuthAppChoice { id: a.id, name: a.name }).collect(),
            grantable_scopes: grantable,
        });
    }
    let redirect_host = reqwest::Url::parse(&request.redirect_uri).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    Ok(Json(OAuthRequestView {
        id: request.id,
        client: OAuthClientView {
            client_id: client.client_id,
            name: client.client_name,
            logo_uri: client.logo_uri,
            client_uri: client.client_uri,
            redirect_host,
            registration: client.registration.as_str().to_string(),
        },
        surface: request.surface.as_str().to_string(),
        requested_scopes: request.scopes,
        expires_at: request.expires_at,
        orgs,
    }))
}

/// POST /v1/oauth/requests/{id}/approve — Approve a sign-in request.
#[utoipa::path(
    post,
    path = "/v1/oauth/requests/{id}/approve",
    params(("id" = String, Path, description = "The request id")),
    request_body = ApproveOAuthRequest,
    responses(
        (status = 200, description = "Send the browser to `redirect_to`: the app's redirect URI with an authorization code", body = OAuthRedirect),
        (status = 400, description = "No scopes, or an app outside the org", body = crate::types::ErrorResponse),
        (status = 401, description = "No dashboard session", body = crate::types::ErrorResponse),
        (status = 403, description = "`not_a_member` of the org, or `scope_not_grantable` by the person's role there", body = crate::types::ErrorResponse),
        (status = 404, description = "Unknown or expired", body = crate::types::ErrorResponse),
        (status = 409, description = "Already approved or denied", body = crate::types::ErrorResponse),
    ),
    tag = "OAuth"
)]
pub async fn approve_request(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
    Json(body): Json<ApproveOAuthRequest>,
) -> Result<Json<OAuthRedirect>, ConsentError> {
    let user_id = browser_user(&ctx)?;
    let (store, request) = pending(&state, &id).await?;
    let role = state.db.get_membership(&body.org_id, user_id).await?.ok_or(ConsentError::NotAMember)?;
    if let Some(app_id) = &body.app_id {
        let app = state.db.get_application(app_id).await?;
        if app.is_none_or(|app| app.org_id != body.org_id) {
            return Err(ConsentError::AppNotInOrg);
        }
    }
    if body.scopes.is_empty() {
        return Err(ConsentError::NoScopes);
    }
    let grantable = grantable_scopes(role);
    if let Some(scope) = body.scopes.iter().find(|s| !request.scopes.contains(s) || !grantable.contains(&s.as_str())) {
        return Err(ConsentError::NotGrantable(scope.clone()));
    }
    let scopes: Vec<String> = request.scopes.iter().filter(|s| body.scopes.contains(s)).cloned().collect();

    let code = random_token("", 32);
    let now = Utc::now();
    let decision = Decision::Approve {
        user_id: user_id.to_string(),
        org_id: body.org_id.clone(),
        app_id: body.app_id.clone(),
        scopes,
        code_hash: sha256_hex(&code),
        code_expires_at: now + CODE_TTL,
        at: now,
    };
    if !store.decide_request(&id, decision).await? {
        return Err(ConsentError::Decided);
    }
    log_org_audit(&state, &ctx, &body.org_id, "oauth.approve", "oauth_client", &request.client_id);
    let back = Back { redirect_uri: &request.redirect_uri, state: request.state.as_deref(), issuer: issuer(&state) };
    Ok(Json(OAuthRedirect { redirect_to: back.url(&[("code", &code)]) }))
}

/// POST /v1/oauth/requests/{id}/deny — Deny a sign-in request.
#[utoipa::path(
    post,
    path = "/v1/oauth/requests/{id}/deny",
    params(("id" = String, Path, description = "The request id")),
    responses(
        (status = 200, description = "Send the browser to `redirect_to`: the app's redirect URI with `error=access_denied`", body = OAuthRedirect),
        (status = 401, description = "No dashboard session", body = crate::types::ErrorResponse),
        (status = 404, description = "Unknown or expired", body = crate::types::ErrorResponse),
        (status = 409, description = "Already approved or denied", body = crate::types::ErrorResponse),
    ),
    tag = "OAuth"
)]
pub async fn deny_request(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
) -> Result<Json<OAuthRedirect>, ConsentError> {
    browser_user(&ctx)?;
    let (store, request) = pending(&state, &id).await?;
    if !store.decide_request(&id, Decision::Deny { at: Utc::now() }).await? {
        return Err(ConsentError::Decided);
    }
    let back = Back { redirect_uri: &request.redirect_uri, state: request.state.as_deref(), issuer: issuer(&state) };
    let redirect_to = back.url(&[("error", "access_denied"), ("error_description", "the person declined the request")]);
    Ok(Json(OAuthRedirect { redirect_to }))
}

/// The person deciding: a dashboard session's user. API keys and OAuth
/// tokens cannot approve sign-ins.
fn browser_user(ctx: &KeyContext) -> Result<&str, ConsentError> {
    match (&ctx.user, &ctx.session_id) {
        (Some(user), Some(_)) => Ok(&user.user_id),
        _ => Err(ConsentError::SessionRequired),
    }
}

/// The store and the request `id`, when the request is still waiting for a decision.
async fn pending<'a>(state: &'a AppState, id: &str) -> Result<(&'a dyn McpOAuthStore, AuthRequest), ConsentError> {
    let store = state.db.mcp_oauth().ok_or(ConsentError::Gone)?;
    match store.get_request(id).await? {
        Some(request) if request.decided_at.is_some() => Err(ConsentError::Decided),
        Some(request) if request.expires_at > Utc::now() => Ok((store, request)),
        _ => Err(ConsentError::Gone),
    }
}

/// Why the consent API refused, as the API's JSON error.
#[derive(Debug)]
pub enum ConsentError {
    SessionRequired,
    /// Unknown or expired.
    Gone,
    Decided,
    NotAMember,
    AppNotInOrg,
    NoScopes,
    NotGrantable(String),
    Store(sqlx::Error),
}

impl From<sqlx::Error> for ConsentError {
    fn from(e: sqlx::Error) -> Self {
        ConsentError::Store(e)
    }
}

impl IntoResponse for ConsentError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            ConsentError::SessionRequired => (StatusCode::UNAUTHORIZED, "session_required", "Sign in to the LiteGen dashboard to approve apps.".to_string()),
            ConsentError::Gone => (StatusCode::NOT_FOUND, "request_not_found", "This sign-in request has expired. Go back to the app and connect again.".to_string()),
            ConsentError::Decided => (StatusCode::CONFLICT, "request_decided", "This sign-in request was already approved or denied.".to_string()),
            ConsentError::NotAMember => (StatusCode::FORBIDDEN, "not_a_member", "You are not a member of that organization.".to_string()),
            ConsentError::AppNotInOrg => (StatusCode::BAD_REQUEST, "app_not_in_org", "That app is not in the organization you picked.".to_string()),
            ConsentError::NoScopes => (StatusCode::BAD_REQUEST, "invalid_request", "Pick at least one scope, or deny the request.".to_string()),
            ConsentError::NotGrantable(scope) => (
                StatusCode::FORBIDDEN,
                "scope_not_grantable",
                format!("Your role in this organization can't grant {scope}, or the app did not ask for it."),
            ),
            ConsentError::Store(e) => {
                tracing::error!(error = %e, "OAuth consent API failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "internal server error".to_string())
            }
        };
        (status, Json(json!({ "error": { "code": code, "message": message, "type": "request_error" } }))).into_response()
    }
}
