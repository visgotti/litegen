//! Connected apps: the OAuth grants a person has given, listed and revoked
//! from the dashboard, the API or the admin MCP endpoint. Revoking a grant
//! stops its access and refresh tokens at once.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::handlers::orgs::log_org_audit;
use crate::api::middleware::{is_platform_admin, within_grant, AppState, KeyContext};

/// An app a person signed in to LiteGen with, and what it may do.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectedApp {
    /// The grant's id: revoke the app with it.
    pub id: String,
    pub client_id: String,
    pub client_name: String,
    /// The endpoint it may use: `ai` (`/mcp`) or `admin` (`/admin/mcp`).
    pub surface: String,
    pub scopes: Vec<String>,
    pub org_id: String,
    pub app_id: Option<String>,
    pub created_at: DateTime<Utc>,
    /// When it last called LiteGen (recorded at most once a minute).
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RevokedGrant {
    pub revoked: bool,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct GrantsQuery {
    /// Platform admin only: whose connected apps to list.
    pub user_id: Option<String>,
}

/// GET /v1/oauth/grants — The apps a person has connected through OAuth.
#[utoipa::path(
    get,
    path = "/v1/oauth/grants",
    params(GrantsQuery),
    responses(
        (status = 200, description = "The person's connected apps, newest first", body = [ConnectedApp]),
        (status = 400, description = "The platform admin named no `user_id`", body = crate::types::ErrorResponse),
        (status = 403, description = "Not a person (an API key), or `user_id` from anyone but the platform admin", body = crate::types::ErrorResponse),
    ),
    tag = "OAuth"
)]
pub async fn list_grants(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Query(query): Query<GrantsQuery>,
) -> Result<Json<Vec<ConnectedApp>>, GrantsError> {
    let store = state.db.mcp_oauth().ok_or(GrantsError::Unavailable)?;
    let user_id = match (&ctx.user, is_platform_admin(&ctx)) {
        (Some(user), _) if query.user_id.as_deref().is_none_or(|u| u == user.user_id) => user.user_id.clone(),
        (Some(_), _) => return Err(GrantsError::Forbidden("only the platform admin can list another person's connected apps")),
        (None, true) => query.user_id.ok_or(GrantsError::UserIdRequired)?,
        (None, false) => return Err(GrantsError::Forbidden("connected apps belong to people: sign in, or use the master key with user_id")),
    };
    let mut apps = Vec::new();
    // An app connected through OAuth sees only the grants of its own org.
    for grant in store.list_grants_for_user(&user_id).await?.into_iter().filter(|g| within_grant(&ctx, &g.org_id)) {
        let client_name = store.get_client(&grant.client_id).await?.map(|c| c.client_name).unwrap_or_else(|| grant.client_id.clone());
        apps.push(ConnectedApp {
            id: grant.id,
            client_id: grant.client_id,
            client_name,
            surface: grant.surface.as_str().to_string(),
            scopes: grant.scopes,
            org_id: grant.org_id,
            app_id: grant.app_id,
            created_at: grant.created_at,
            last_used_at: grant.last_used_at,
        });
    }
    Ok(Json(apps))
}

/// DELETE /v1/oauth/grants/{id} — Disconnect an app.
#[utoipa::path(
    delete,
    path = "/v1/oauth/grants/{id}",
    params(("id" = String, Path, description = "The connected app's grant id")),
    responses(
        (status = 200, description = "Revoked: its tokens stop working at once", body = RevokedGrant),
        (status = 404, description = "No such connected app of yours", body = crate::types::ErrorResponse),
    ),
    tag = "OAuth"
)]
pub async fn revoke_grant(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
) -> Result<Json<RevokedGrant>, GrantsError> {
    let store = state.db.mcp_oauth().ok_or(GrantsError::Unavailable)?;
    let grant = store.get_grant(&id).await?.filter(|g| g.revoked_at.is_none()).ok_or(GrantsError::NotFound)?;
    let own = ctx.user.as_ref().is_some_and(|u| u.user_id == grant.user_id) && within_grant(&ctx, &grant.org_id);
    // Someone else's grant answers as if it did not exist.
    if !(own || is_platform_admin(&ctx)) {
        return Err(GrantsError::NotFound);
    }
    store.revoke_grant(&id, Utc::now()).await?;
    log_org_audit(&state, &ctx, &grant.org_id, "oauth.revoke", "oauth_client", &grant.client_id);
    Ok(Json(RevokedGrant { revoked: true }))
}

/// Why the connected-apps API refused.
#[derive(Debug)]
pub enum GrantsError {
    Forbidden(&'static str),
    UserIdRequired,
    NotFound,
    Unavailable,
    Store(sqlx::Error),
}

impl From<sqlx::Error> for GrantsError {
    fn from(e: sqlx::Error) -> Self {
        GrantsError::Store(e)
    }
}

impl IntoResponse for GrantsError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            GrantsError::Forbidden(message) => (StatusCode::FORBIDDEN, "forbidden", message),
            GrantsError::UserIdRequired => (StatusCode::BAD_REQUEST, "user_id_required", "name the person whose connected apps to list: user_id"),
            GrantsError::NotFound => (StatusCode::NOT_FOUND, "not_found", "no such connected app"),
            GrantsError::Unavailable => (StatusCode::NOT_FOUND, "not_found", "OAuth is not available on this server"),
            GrantsError::Store(e) => {
                tracing::error!(error = %e, "connected apps API failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "internal server error")
            }
        };
        (status, Json(json!({ "error": { "code": code, "message": message, "type": "request_error" } }))).into_response()
    }
}
