//! The token endpoint (RFC 6749 §3.2) and revocation (RFC 7009).
//!
//! A code is redeemed once, with its PKCE verifier, by the client and
//! redirect URI it was issued to; a refresh token is exchanged once for a new
//! pair. A second use of either is a replay, and revokes the grant. A client
//! that registered `private_key_jwt` signs every request. Tokens are opaque
//! (`lgat_…` access, `lgrt_…` refresh), stored only as SHA-256 hashes, and
//! work only on their grant's MCP endpoint. Every answer is `no-store`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::rejection::FormRejection;
use axum::extract::{Form, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::json;

use super::assertion::{self, AssertionError, ASSERTION_TYPE};
use super::clients::uses_refresh_tokens;
use super::crypto::{random_token, s256_challenge, sha256_hex};
use super::discovery::{issuer, resource_allows};
use super::principal::{grant_app, AccessError};
use super::store::{AuthRequest, ConsumeOutcome, Grant, McpOAuthStore, OAuthClient, RotateOutcome, TokenKind, TokenRow};
use super::OAuthShared;
use crate::api::middleware::AppState;
use crate::types::{Role, User};

/// The largest token or revocation request.
pub const FORM_MAX_BYTES: usize = 16 * 1024;
/// What every access token starts with, and every refresh token.
pub const ACCESS_PREFIX: &str = "lgat_";
pub const REFRESH_PREFIX: &str = "lgrt_";

#[derive(Debug, Serialize)]
struct TokenAnswer {
    access_token: String,
    token_type: &'static str,
    expires_in: i64,
    /// None for a client that registered without the refresh_token grant.
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    scope: String,
}

/// `POST /oauth/token`.
pub async fn token(State(shared): State<Arc<OAuthShared>>, form: Result<Form<Vec<(String, String)>>, FormRejection>) -> Response {
    let Some(store) = shared.state.db.mcp_oauth() else {
        return TokenError::new(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "OAuth is not available").into_response();
    };
    let answer = match Params::parse(form) {
        Ok(params) => match params.get("grant_type") {
            Some("authorization_code") => redeem_code(&shared, store, &params).await,
            Some("refresh_token") => refresh(&shared, store, &params).await,
            Some(_) => Err(TokenError::bad("unsupported_grant_type", "grant_type must be authorization_code or refresh_token")),
            None => Err(TokenError::request("grant_type is required")),
        },
        Err(e) => Err(e),
    };
    match answer {
        Ok(answer) => (StatusCode::OK, [(header::CACHE_CONTROL, "no-store"), (header::PRAGMA, "no-cache")], Json(answer)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// The client a token request comes from, authenticated as it registered:
/// a public client by its `client_id` (its PKCE verifier or refresh token
/// proves the rest), a `private_key_jwt` client by a valid assertion, which
/// may stand in for `client_id`.
async fn authenticate(shared: &OAuthShared, store: &dyn McpOAuthStore, params: &Params, now: DateTime<Utc>) -> Result<OAuthClient, TokenError> {
    let assertion = match params.get("client_assertion_type") {
        Some(ASSERTION_TYPE) => Some(params.require("client_assertion")?),
        Some(_) => return Err(TokenError::request(&format!("client_assertion_type must be {ASSERTION_TYPE}"))),
        None => None,
    };
    let client_id = match (params.get("client_id"), assertion) {
        (Some(id), _) => id.to_string(),
        (None, Some(assertion)) => assertion::claimed_client(assertion).ok_or_else(|| TokenError::unauthenticated("the client assertion names no client (sub)"))?,
        (None, None) => return Err(TokenError::request("client_id is required")),
    };
    let client = store.get_client(&client_id).await?.ok_or_else(TokenError::client)?;
    if assertion::required(&client) {
        let assertion = assertion.ok_or_else(|| TokenError::unauthenticated("this client signs in with private_key_jwt: send a client_assertion"))?;
        let state = &shared.state;
        let audiences = [issuer(state), format!("{}/oauth/token", issuer(state))];
        match assertion::verify(assertion, &client, &audiences, &shared.keys, shared.fetcher.as_ref(), store, now).await {
            Ok(()) => {}
            Err(AssertionError::Invalid(reason)) => return Err(TokenError::unauthenticated(&reason)),
            Err(AssertionError::Store(e)) => return Err(e.into()),
        }
    }
    Ok(client)
}

/// `grant_type=authorization_code`.
async fn redeem_code(shared: &OAuthShared, store: &dyn McpOAuthStore, params: &Params) -> Result<TokenAnswer, TokenError> {
    let state = &shared.state;
    let code = params.require("code")?;
    let redirect_uri = params.require("redirect_uri")?;
    let verifier = params.require("code_verifier")?;
    let now = Utc::now();
    let client = authenticate(shared, store, params, now).await?;
    let client_id = client.client_id.as_str();

    // The code is spent by its first presentation, right or wrong: a stolen
    // code cannot be tried against verifiers.
    let request = match store.consume_code(&sha256_hex(code), now).await? {
        ConsumeOutcome::Consumed(request) => request,
        ConsumeOutcome::AlreadyUsed(request) => {
            revoke_issued(store, &request, now).await?;
            return Err(TokenError::grant("the code was used before; the access it gave is revoked"));
        }
        ConsumeOutcome::Missing => return Err(TokenError::grant("the code is unknown or expired")),
    };
    if request.client_id != client_id {
        return Err(TokenError::grant("the code was issued to another client"));
    }
    if request.redirect_uri != redirect_uri {
        return Err(TokenError::grant("redirect_uri is not the one the code was issued for"));
    }
    if !valid_verifier(verifier) || s256_challenge(verifier) != request.code_challenge {
        return Err(TokenError::grant("code_verifier does not match the code_challenge"));
    }
    if let Some(resource) = params.get("resource") {
        if !resource_allows(state, request.surface, resource) {
            return Err(TokenError::bad("invalid_target", "resource is not the endpoint this sign-in was for"));
        }
    }
    let (Some(user_id), Some(org_id), Some(scopes)) = (request.user_id, request.org_id, request.granted_scopes) else {
        return Err(TokenError::grant("the code was never approved"));
    };

    // One grant per client, person, org, app and endpoint: signing in again
    // updates it instead of piling up grants.
    let grant = store
        .claim_grant(&Grant {
            id: uuid::Uuid::new_v4().to_string(),
            client_id: client_id.to_string(),
            user_id,
            org_id,
            app_id: request.app_id,
            surface: request.surface,
            scopes,
            created_at: now,
            last_used_at: None,
            revoked_at: None,
        })
        .await?;
    let (access, access_row) = mint(&grant.id, TokenKind::Access, now, access_ttl(state));
    store.insert_token(&access_row).await?;
    let refresh = if uses_refresh_tokens(&client) {
        let (refresh, refresh_row) = mint(&grant.id, TokenKind::Refresh, now, refresh_ttl(state));
        store.insert_token(&refresh_row).await?;
        Some(refresh)
    } else {
        None
    };
    store.touch_client(client_id, now).await?;
    Ok(answer(state, access, refresh, &grant.scopes))
}

/// `grant_type=refresh_token`.
async fn refresh(shared: &OAuthShared, store: &dyn McpOAuthStore, params: &Params) -> Result<TokenAnswer, TokenError> {
    let state = &shared.state;
    let token = params.require("refresh_token")?;
    let now = Utc::now();
    let client = authenticate(shared, store, params, now).await?;
    let client_id = client.client_id.as_str();
    if !uses_refresh_tokens(&client) {
        return Err(TokenError::bad("unauthorized_client", "this client registered without the refresh_token grant"));
    }
    let row = store
        .get_token(&sha256_hex(token))
        .await?
        .filter(|t| t.kind == TokenKind::Refresh)
        .ok_or_else(|| TokenError::grant("the refresh token is unknown"))?;
    let grant = store
        .get_grant(&row.grant_id)
        .await?
        .filter(|g| g.revoked_at.is_none())
        .ok_or_else(|| TokenError::grant("the app's access was revoked"))?;
    if grant.client_id != client_id {
        return Err(TokenError::grant("the refresh token was issued to another client"));
    }
    if let Some(scope) = params.get("scope") {
        if scope.split_whitespace().any(|s| !grant.scopes.iter().any(|g| g == s)) {
            return Err(TokenError::bad("invalid_scope", "scope asks for more than was granted"));
        }
    }
    // A sign-in whose new access token would be refused ends here, so the
    // client signs in again instead of refreshing in a loop.
    if live_member(state, &grant).await?.is_none() {
        store.revoke_grant(&grant.id, now).await?;
        return Err(TokenError::grant("the person who approved this app can no longer use LiteGen there"));
    }
    match grant_app(state, &grant).await {
        Ok(_) => {}
        Err(AccessError::Invalid(reason)) => {
            store.revoke_grant(&grant.id, now).await?;
            return Err(TokenError::grant(reason));
        }
        Err(AccessError::Store(e)) => return Err(e.into()),
    }

    let (access, access_row) = mint(&grant.id, TokenKind::Access, now, access_ttl(state));
    let (refresh, refresh_row) = mint(&grant.id, TokenKind::Refresh, now, refresh_ttl(state));
    match store.rotate_refresh(&row.token_hash, &access_row, &refresh_row, now).await? {
        RotateOutcome::Rotated => {
            store.touch_client(client_id, now).await?;
            Ok(answer(state, access, Some(refresh), &grant.scopes))
        }
        RotateOutcome::Reused { grant_id } => {
            store.revoke_grant(&grant_id, now).await?;
            Err(TokenError::grant("the refresh token was used before; the app's access is revoked"))
        }
        RotateOutcome::Missing => Err(TokenError::grant("the refresh token is expired or revoked")),
    }
}

/// The grant's person and their role in its org, while the grant still
/// speaks for them: unrevoked, the person active, and still a member.
pub async fn live_member(state: &AppState, grant: &Grant) -> Result<Option<(User, Role)>, sqlx::Error> {
    if grant.revoked_at.is_some() {
        return Ok(None);
    }
    let Some(user) = state.db.get_user_by_id(&grant.user_id).await?.filter(|u| u.is_active) else {
        return Ok(None);
    };
    Ok(state.db.get_membership(&grant.org_id, &grant.user_id).await?.map(|role| (user, role)))
}

/// Revoke the grant a replayed code's request led to, with its tokens.
async fn revoke_issued(store: &dyn McpOAuthStore, request: &AuthRequest, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    if let (Some(user_id), Some(org_id)) = (&request.user_id, &request.org_id) {
        if let Some(grant) = store.find_active_grant(&request.client_id, user_id, org_id, request.app_id.as_deref(), request.surface).await? {
            store.revoke_grant(&grant.id, now).await?;
        }
    }
    Ok(())
}

/// A new token for `grant_id` and the row that stores its hash.
fn mint(grant_id: &str, kind: TokenKind, now: DateTime<Utc>, ttl: Duration) -> (String, TokenRow) {
    let prefix = match kind {
        TokenKind::Access => ACCESS_PREFIX,
        TokenKind::Refresh => REFRESH_PREFIX,
    };
    let token = random_token(prefix, 32);
    let row = TokenRow {
        token_hash: sha256_hex(&token),
        grant_id: grant_id.to_string(),
        kind,
        created_at: now,
        expires_at: now + ttl,
        rotated_at: None,
        revoked_at: None,
    };
    (token, row)
}

fn answer(state: &AppState, access: String, refresh: Option<String>, scopes: &[String]) -> TokenAnswer {
    TokenAnswer {
        access_token: access,
        token_type: "Bearer",
        expires_in: access_ttl(state).num_seconds(),
        refresh_token: refresh,
        scope: scopes.join(" "),
    }
}

fn access_ttl(state: &AppState) -> Duration {
    Duration::seconds(state.router.config.mcp.oauth.access_token_ttl_secs.min(i64::MAX as u64) as i64)
}

fn refresh_ttl(state: &AppState) -> Duration {
    Duration::days(state.router.config.mcp.oauth.refresh_token_ttl_days.min(36_500) as i64)
}

/// A PKCE verifier: 43 to 128 unreserved characters (RFC 7636 §4.1).
fn valid_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len()) && verifier.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

/// `POST /oauth/revoke`: always 200, as RFC 7009 asks, so a client cannot
/// probe which tokens exist. A refresh token revokes its whole grant; an
/// access token, only itself.
pub async fn revoke(State(shared): State<Arc<OAuthShared>>, form: Result<Form<Vec<(String, String)>>, FormRejection>) -> Response {
    let token = form.ok().and_then(|Form(pairs)| pairs.into_iter().find(|(k, _)| k == "token").map(|(_, v)| v));
    if let (Some(store), Some(token)) = (shared.state.db.mcp_oauth(), token) {
        if let Err(e) = revoke_token(store, &token).await {
            tracing::error!(error = %e, "OAuth token revocation failed");
            return TokenError::new(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "try again").into_response();
        }
    }
    (StatusCode::OK, [(header::CACHE_CONTROL, "no-store")]).into_response()
}

async fn revoke_token(store: &dyn McpOAuthStore, token: &str) -> Result<(), sqlx::Error> {
    let Some(row) = store.get_token(&sha256_hex(token)).await? else { return Ok(()) };
    let now = Utc::now();
    match row.kind {
        TokenKind::Refresh => store.revoke_grant(&row.grant_id, now).await.map(|_| ()),
        TokenKind::Access => store.revoke_token(&row.token_hash, now).await,
    }
}

/// A token request's form fields; RFC 6749 §3.2 allows each once.
struct Params(HashMap<String, String>);

impl Params {
    fn parse(form: Result<Form<Vec<(String, String)>>, FormRejection>) -> Result<Params, TokenError> {
        let Ok(Form(pairs)) = form else {
            return Err(TokenError::request("send the fields as application/x-www-form-urlencoded"));
        };
        let mut fields = HashMap::new();
        for (key, value) in pairs {
            if fields.contains_key(&key) {
                return Err(TokenError::request(&format!("{key} is given more than once")));
            }
            fields.insert(key, value);
        }
        Ok(Params(fields))
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str).filter(|v| !v.is_empty())
    }

    fn require(&self, key: &str) -> Result<&str, TokenError> {
        self.get(key).ok_or_else(|| TokenError::request(&format!("{key} is required")))
    }
}

/// An RFC 6749 §5.2 error answer.
#[derive(Debug)]
struct TokenError {
    status: StatusCode,
    error: &'static str,
    description: String,
}

impl TokenError {
    fn new(status: StatusCode, error: &'static str, description: &str) -> Self {
        TokenError { status, error, description: description.to_string() }
    }

    fn bad(error: &'static str, description: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, error, description)
    }

    fn request(description: &str) -> Self {
        Self::bad("invalid_request", description)
    }

    fn grant(description: &str) -> Self {
        Self::bad("invalid_grant", description)
    }

    fn client() -> Self {
        Self::unauthenticated("LiteGen has no client with this client_id")
    }

    /// The client is unknown or failed to authenticate (RFC 6749 §5.2).
    fn unauthenticated(description: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "invalid_client", description)
    }
}

impl From<sqlx::Error> for TokenError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "OAuth token endpoint failed");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "the request could not be completed")
    }
}

impl IntoResponse for TokenError {
    fn into_response(self) -> Response {
        let body = json!({ "error": self.error, "error_description": self.description });
        (self.status, [(header::CACHE_CONTROL, "no-store"), (header::PRAGMA, "no-cache")], Json(body)).into_response()
    }
}
