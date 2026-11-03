//! Who an OAuth access token speaks for. A live token resolves to its
//! grant's person, in the grant's org and app, with their role there and
//! the grant's scopes: the same context a dashboard session would give them,
//! narrowed to what they approved and confined to the one endpoint and the
//! one org the grant is for.

use chrono::{Duration, Utc};

use super::crypto::sha256_hex;
use super::store::{Grant, TokenKind};
use super::token::live_member;
use crate::api::middleware::{AppState, KeyContext, Scope, UserContext, DEFAULT_APP_ID, DEFAULT_ORG_ID};
use crate::auth::permissions::permissions_for;
use crate::mcp::Surface;

/// `last_used_at` is written at most this often per grant.
const TOUCH_EVERY: Duration = Duration::minutes(1);

/// Why a token was refused.
#[derive(Debug)]
pub enum AccessError {
    /// Unknown, expired, revoked, for the other endpoint, or for someone who
    /// can no longer act: the client must sign in again (`invalid_token`).
    Invalid(&'static str),
    Store(sqlx::Error),
}

impl From<sqlx::Error> for AccessError {
    fn from(e: sqlx::Error) -> Self {
        AccessError::Store(e)
    }
}

/// The principal an access token stands for on `surface`.
pub async fn resolve_access_token(state: &AppState, token: &str, surface: Surface) -> Result<KeyContext, AccessError> {
    let store = state.db.mcp_oauth().filter(|_| super::available(state)).ok_or(AccessError::Invalid("OAuth is off"))?;
    let now = Utc::now();
    let row = store
        .get_token(&sha256_hex(token))
        .await?
        .filter(|t| t.kind == TokenKind::Access && t.revoked_at.is_none() && t.expires_at > now)
        .ok_or(AccessError::Invalid("the access token is unknown, expired or revoked"))?;
    let grant = store.get_grant(&row.grant_id).await?.ok_or(AccessError::Invalid("the app's access was revoked"))?;
    if grant.surface != surface {
        return Err(AccessError::Invalid("this sign-in is for the other MCP endpoint"));
    }
    let (user, role) = live_member(state, &grant)
        .await?
        .ok_or(AccessError::Invalid("the person who approved this app can no longer act in its organization"))?;
    let app_id = grant_app(state, &grant).await?;

    if grant.last_used_at.is_none_or(|at| now - at >= TOUCH_EVERY) {
        if let Err(e) = store.touch_grant(&grant.id, now).await {
            tracing::debug!(error = %e, "recording an OAuth grant's use failed");
        }
    }

    Ok(KeyContext {
        key_id: None,
        scopes: key_scopes(&grant.scopes),
        rpm_limit: None,
        quota_remaining: None,
        webhook_url: None,
        user: Some(UserContext { user_id: user.id, email: user.email, role: user.role }),
        permissions: permissions_for(role).to_vec(),
        session_id: None,
        org_id: Some(grant.org_id),
        app_id: Some(app_id),
        oauth_grant_id: Some(grant.id),
    })
}

/// The app a grant acts in: the one approved, which must still be the org's;
/// else the org's first app, as a dashboard session picks it.
async fn grant_app(state: &AppState, grant: &Grant) -> Result<String, AccessError> {
    if let Some(app_id) = &grant.app_id {
        return match state.db.get_application(app_id).await? {
            Some(app) if app.org_id == grant.org_id => Ok(app.id),
            _ => Err(AccessError::Invalid("the app this sign-in was approved for no longer exists")),
        };
    }
    if grant.org_id == DEFAULT_ORG_ID {
        return Ok(DEFAULT_APP_ID.to_string());
    }
    state
        .db
        .list_apps_for_org(&grant.org_id)
        .await?
        .into_iter()
        .next()
        .map(|app| app.id)
        .ok_or(AccessError::Invalid("the organization has no app to act in"))
}

/// A grant's OAuth scopes as key scopes: `ai:generate` → `generate`,
/// `ai:read` and `admin:read` → `read`, `admin:write` → `admin`.
pub fn key_scopes(scopes: &[String]) -> Vec<Scope> {
    let mut out = Vec::new();
    for scope in scopes {
        let mapped = match scope.as_str() {
            "ai:generate" => Scope::Generate,
            "ai:read" | "admin:read" => Scope::Read,
            "admin:write" => Scope::Admin,
            _ => continue,
        };
        if !out.contains(&mapped) {
            out.push(mapped);
        }
    }
    out
}
