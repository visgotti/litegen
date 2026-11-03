//! [`McpOAuthStore`] for the SQLite and Postgres backends: one body, expanded
//! twice by `impl_mcp_oauth_store!`, with `?` placeholders passed through
//! `$sql` (identity for SQLite, `?` → `$n` for Postgres), as
//! `management::sql` does.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::store::*;
use crate::db::{PostgresDatabase, SqliteDatabase};
use crate::management::sql::{identity_sql, pg_sql};
use crate::mcp::Surface;

#[derive(sqlx::FromRow)]
struct ClientRow {
    client_id: String,
    registration: String,
    client_name: String,
    redirect_uris: String,
    logo_uri: Option<String>,
    client_uri: Option<String>,
    metadata_json: String,
    created_at: DateTime<Utc>,
    refreshed_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
}

const CLIENT_COLS: &str =
    "client_id, registration, client_name, redirect_uris, logo_uri, client_uri, metadata_json, created_at, refreshed_at, last_used_at";

fn client_from_row(r: ClientRow) -> Option<OAuthClient> {
    Some(OAuthClient {
        client_id: r.client_id,
        registration: Registration::parse(&r.registration)?,
        client_name: r.client_name,
        redirect_uris: serde_json::from_str(&r.redirect_uris).ok()?,
        logo_uri: r.logo_uri,
        client_uri: r.client_uri,
        metadata: serde_json::from_str(&r.metadata_json).unwrap_or(serde_json::Value::Null),
        created_at: r.created_at,
        refreshed_at: r.refreshed_at,
        last_used_at: r.last_used_at,
    })
}

#[derive(sqlx::FromRow)]
struct RequestRow {
    id: String,
    client_id: String,
    redirect_uri: String,
    state: Option<String>,
    code_challenge: String,
    scopes: String,
    surface: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    user_id: Option<String>,
    org_id: Option<String>,
    app_id: Option<String>,
    granted_scopes: Option<String>,
    code_hash: Option<String>,
    decided_at: Option<DateTime<Utc>>,
    consumed_at: Option<DateTime<Utc>>,
}

const REQUEST_COLS: &str = "id, client_id, redirect_uri, state, code_challenge, scopes, surface, created_at, expires_at, \
    user_id, org_id, app_id, granted_scopes, code_hash, decided_at, consumed_at";

fn request_from_row(r: RequestRow) -> Option<AuthRequest> {
    Some(AuthRequest {
        id: r.id,
        client_id: r.client_id,
        redirect_uri: r.redirect_uri,
        state: r.state,
        code_challenge: r.code_challenge,
        scopes: split(&r.scopes),
        surface: Surface::parse(&r.surface)?,
        created_at: r.created_at,
        expires_at: r.expires_at,
        user_id: r.user_id,
        org_id: r.org_id,
        app_id: r.app_id,
        granted_scopes: r.granted_scopes.as_deref().map(split),
        code_hash: r.code_hash,
        decided_at: r.decided_at,
        consumed_at: r.consumed_at,
    })
}

#[derive(sqlx::FromRow)]
struct GrantRow {
    id: String,
    client_id: String,
    user_id: String,
    org_id: String,
    app_id: Option<String>,
    surface: String,
    scopes: String,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

const GRANT_COLS: &str = "id, client_id, user_id, org_id, app_id, surface, scopes, created_at, last_used_at, revoked_at";

fn grant_from_row(r: GrantRow) -> Option<Grant> {
    Some(Grant {
        id: r.id,
        client_id: r.client_id,
        user_id: r.user_id,
        org_id: r.org_id,
        app_id: r.app_id,
        surface: Surface::parse(&r.surface)?,
        scopes: split(&r.scopes),
        created_at: r.created_at,
        last_used_at: r.last_used_at,
        revoked_at: r.revoked_at,
    })
}

#[derive(sqlx::FromRow)]
struct TokenDbRow {
    token_hash: String,
    grant_id: String,
    kind: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    rotated_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

const TOKEN_COLS: &str = "token_hash, grant_id, kind, created_at, expires_at, rotated_at, revoked_at";

fn token_from_row(r: TokenDbRow) -> Option<TokenRow> {
    Some(TokenRow {
        token_hash: r.token_hash,
        grant_id: r.grant_id,
        kind: TokenKind::parse(&r.kind)?,
        created_at: r.created_at,
        expires_at: r.expires_at,
        rotated_at: r.rotated_at,
        revoked_at: r.revoked_at,
    })
}

/// Scopes are stored space-separated, as OAuth writes them.
fn join(scopes: &[String]) -> String {
    scopes.join(" ")
}

fn split(scopes: &str) -> Vec<String> {
    scopes.split_whitespace().map(String::from).collect()
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    e.as_database_error().is_some_and(|d| d.is_unique_violation())
}

/// Grants whose org, or whose named app, no longer exists; `?` is a cutoff
/// on when the grant was made.
const ORPHANED_GRANTS: &str = "SELECT g.id FROM mcp_oauth_grants g WHERE g.created_at < ? \
     AND (NOT EXISTS (SELECT 1 FROM organizations o WHERE o.id = g.org_id) \
     OR (g.app_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM applications a WHERE a.id = g.app_id)))";

/// No sign-in under way for the client, and no grant refers to it.
const CLIENT_UNREFERENCED: &str = "NOT EXISTS (SELECT 1 FROM mcp_oauth_requests r WHERE r.client_id = mcp_oauth_clients.client_id) \
     AND NOT EXISTS (SELECT 1 FROM mcp_oauth_grants g WHERE g.client_id = mcp_oauth_clients.client_id)";

macro_rules! impl_mcp_oauth_store {
    ($ty:ty, $sql:path) => {
        #[async_trait]
        impl McpOAuthStore for $ty {
            // ─── Clients ────────────────────────────────────────────────
            async fn upsert_client(&self, c: &OAuthClient) -> Result<(), sqlx::Error> {
                let sql = $sql(&format!(
                    "INSERT INTO mcp_oauth_clients ({CLIENT_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT (client_id) DO UPDATE SET registration = excluded.registration, \
                     client_name = excluded.client_name, redirect_uris = excluded.redirect_uris, \
                     logo_uri = excluded.logo_uri, client_uri = excluded.client_uri, \
                     metadata_json = excluded.metadata_json, refreshed_at = excluded.refreshed_at"
                ));
                sqlx::query(&sql)
                    .bind(&c.client_id)
                    .bind(c.registration.as_str())
                    .bind(&c.client_name)
                    .bind(serde_json::to_string(&c.redirect_uris).unwrap_or_else(|_| "[]".into()))
                    .bind(&c.logo_uri)
                    .bind(&c.client_uri)
                    .bind(c.metadata.to_string())
                    .bind(c.created_at)
                    .bind(c.refreshed_at)
                    .bind(c.last_used_at)
                    .execute(self.pool())
                    .await?;
                Ok(())
            }

            async fn get_client(&self, client_id: &str) -> Result<Option<OAuthClient>, sqlx::Error> {
                let sql = $sql(&format!("SELECT {CLIENT_COLS} FROM mcp_oauth_clients WHERE client_id = ?"));
                let row = sqlx::query_as::<_, ClientRow>(&sql).bind(client_id).fetch_optional(self.pool()).await?;
                Ok(row.and_then(client_from_row))
            }

            async fn touch_client(&self, client_id: &str, at: DateTime<Utc>) -> Result<(), sqlx::Error> {
                let sql = $sql("UPDATE mcp_oauth_clients SET last_used_at = ? WHERE client_id = ?");
                sqlx::query(&sql).bind(at).bind(client_id).execute(self.pool()).await?;
                Ok(())
            }

            // ─── Requests and codes ─────────────────────────────────────
            async fn insert_request(&self, r: &AuthRequest) -> Result<(), sqlx::Error> {
                let sql = $sql(&format!(
                    "INSERT INTO mcp_oauth_requests ({REQUEST_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                ));
                sqlx::query(&sql)
                    .bind(&r.id)
                    .bind(&r.client_id)
                    .bind(&r.redirect_uri)
                    .bind(&r.state)
                    .bind(&r.code_challenge)
                    .bind(join(&r.scopes))
                    .bind(r.surface.as_str())
                    .bind(r.created_at)
                    .bind(r.expires_at)
                    .bind(&r.user_id)
                    .bind(&r.org_id)
                    .bind(&r.app_id)
                    .bind(r.granted_scopes.as_deref().map(join))
                    .bind(&r.code_hash)
                    .bind(r.decided_at)
                    .bind(r.consumed_at)
                    .execute(self.pool())
                    .await?;
                Ok(())
            }

            async fn get_request(&self, id: &str) -> Result<Option<AuthRequest>, sqlx::Error> {
                let sql = $sql(&format!("SELECT {REQUEST_COLS} FROM mcp_oauth_requests WHERE id = ?"));
                let row = sqlx::query_as::<_, RequestRow>(&sql).bind(id).fetch_optional(self.pool()).await?;
                Ok(row.and_then(request_from_row))
            }

            async fn decide_request(&self, id: &str, decision: Decision) -> Result<bool, sqlx::Error> {
                let result = match decision {
                    Decision::Approve { user_id, org_id, app_id, scopes, code_hash, code_expires_at, at } => {
                        let sql = $sql(
                            "UPDATE mcp_oauth_requests SET user_id = ?, org_id = ?, app_id = ?, granted_scopes = ?, \
                             code_hash = ?, expires_at = ?, decided_at = ? \
                             WHERE id = ? AND decided_at IS NULL AND expires_at > ?",
                        );
                        sqlx::query(&sql)
                            .bind(user_id)
                            .bind(org_id)
                            .bind(app_id)
                            .bind(join(&scopes))
                            .bind(code_hash)
                            .bind(code_expires_at)
                            .bind(at)
                            .bind(id)
                            .bind(at)
                            .execute(self.pool())
                            .await?
                    }
                    Decision::Deny { at } => {
                        let sql = $sql(
                            "UPDATE mcp_oauth_requests SET decided_at = ? WHERE id = ? AND decided_at IS NULL AND expires_at > ?",
                        );
                        sqlx::query(&sql).bind(at).bind(id).bind(at).execute(self.pool()).await?
                    }
                };
                Ok(result.rows_affected() == 1)
            }

            async fn consume_code(&self, code_hash: &str, now: DateTime<Utc>) -> Result<ConsumeOutcome, sqlx::Error> {
                let sql = $sql(
                    "UPDATE mcp_oauth_requests SET consumed_at = ? WHERE code_hash = ? AND consumed_at IS NULL AND expires_at > ?",
                );
                let claimed = sqlx::query(&sql).bind(now).bind(code_hash).bind(now).execute(self.pool()).await?.rows_affected() == 1;
                let sql = $sql(&format!("SELECT {REQUEST_COLS} FROM mcp_oauth_requests WHERE code_hash = ?"));
                let row = sqlx::query_as::<_, RequestRow>(&sql).bind(code_hash).fetch_optional(self.pool()).await?;
                Ok(match row.and_then(request_from_row) {
                    Some(r) if claimed => ConsumeOutcome::Consumed(r),
                    Some(r) if r.consumed_at.is_some() => ConsumeOutcome::AlreadyUsed(r),
                    _ => ConsumeOutcome::Missing,
                })
            }

            // ─── Grants ─────────────────────────────────────────────────
            async fn insert_grant(&self, g: &Grant) -> Result<(), sqlx::Error> {
                let sql = $sql(&format!("INSERT INTO mcp_oauth_grants ({GRANT_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"));
                sqlx::query(&sql)
                    .bind(&g.id)
                    .bind(&g.client_id)
                    .bind(&g.user_id)
                    .bind(&g.org_id)
                    .bind(&g.app_id)
                    .bind(g.surface.as_str())
                    .bind(join(&g.scopes))
                    .bind(g.created_at)
                    .bind(g.last_used_at)
                    .bind(g.revoked_at)
                    .execute(self.pool())
                    .await?;
                Ok(())
            }

            async fn claim_grant(&self, g: &Grant) -> Result<Grant, sqlx::Error> {
                // When two first sign-ins race, the unique index fails the
                // later insert, and that claim takes the grant that won.
                for _ in 0..3 {
                    if let Some(live) = self.find_active_grant(&g.client_id, &g.user_id, &g.org_id, g.app_id.as_deref(), g.surface).await? {
                        self.update_grant_scopes(&live.id, &g.scopes).await?;
                        return Ok(Grant { scopes: g.scopes.clone(), ..live });
                    }
                    match self.insert_grant(g).await {
                        Ok(()) => return Ok(g.clone()),
                        Err(e) if is_unique_violation(&e) => continue,
                        Err(e) => return Err(e),
                    }
                }
                Err(sqlx::Error::Protocol("the grant kept changing while it was claimed".into()))
            }

            async fn get_grant(&self, id: &str) -> Result<Option<Grant>, sqlx::Error> {
                let sql = $sql(&format!("SELECT {GRANT_COLS} FROM mcp_oauth_grants WHERE id = ?"));
                let row = sqlx::query_as::<_, GrantRow>(&sql).bind(id).fetch_optional(self.pool()).await?;
                Ok(row.and_then(grant_from_row))
            }

            async fn find_active_grant(
                &self,
                client_id: &str,
                user_id: &str,
                org_id: &str,
                app_id: Option<&str>,
                surface: Surface,
            ) -> Result<Option<Grant>, sqlx::Error> {
                let sql = $sql(&format!(
                    "SELECT {GRANT_COLS} FROM mcp_oauth_grants WHERE client_id = ? AND user_id = ? AND org_id = ? \
                     AND COALESCE(app_id, '') = COALESCE(?, '') AND surface = ? AND revoked_at IS NULL \
                     ORDER BY created_at DESC LIMIT 1"
                ));
                let row = sqlx::query_as::<_, GrantRow>(&sql)
                    .bind(client_id)
                    .bind(user_id)
                    .bind(org_id)
                    .bind(app_id)
                    .bind(surface.as_str())
                    .fetch_optional(self.pool())
                    .await?;
                Ok(row.and_then(grant_from_row))
            }

            async fn update_grant_scopes(&self, id: &str, scopes: &[String]) -> Result<(), sqlx::Error> {
                let sql = $sql("UPDATE mcp_oauth_grants SET scopes = ? WHERE id = ?");
                sqlx::query(&sql).bind(join(scopes)).bind(id).execute(self.pool()).await?;
                Ok(())
            }

            async fn list_grants_for_user(&self, user_id: &str) -> Result<Vec<Grant>, sqlx::Error> {
                let sql = $sql(&format!(
                    "SELECT {GRANT_COLS} FROM mcp_oauth_grants WHERE user_id = ? AND revoked_at IS NULL ORDER BY created_at DESC"
                ));
                let rows = sqlx::query_as::<_, GrantRow>(&sql).bind(user_id).fetch_all(self.pool()).await?;
                Ok(rows.into_iter().filter_map(grant_from_row).collect())
            }

            async fn revoke_grant(&self, id: &str, at: DateTime<Utc>) -> Result<bool, sqlx::Error> {
                let mut tx = self.pool().begin().await?;
                let sql = $sql("UPDATE mcp_oauth_grants SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL");
                let revoked = sqlx::query(&sql).bind(at).bind(id).execute(&mut *tx).await?.rows_affected() == 1;
                let sql = $sql("UPDATE mcp_oauth_tokens SET revoked_at = ? WHERE grant_id = ? AND revoked_at IS NULL");
                sqlx::query(&sql).bind(at).bind(id).execute(&mut *tx).await?;
                tx.commit().await?;
                Ok(revoked)
            }

            async fn touch_grant(&self, id: &str, at: DateTime<Utc>) -> Result<(), sqlx::Error> {
                let sql = $sql("UPDATE mcp_oauth_grants SET last_used_at = ? WHERE id = ?");
                sqlx::query(&sql).bind(at).bind(id).execute(self.pool()).await?;
                Ok(())
            }

            // ─── Tokens ─────────────────────────────────────────────────
            async fn insert_token(&self, t: &TokenRow) -> Result<(), sqlx::Error> {
                let sql = $sql(&format!("INSERT INTO mcp_oauth_tokens ({TOKEN_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?)"));
                sqlx::query(&sql)
                    .bind(&t.token_hash)
                    .bind(&t.grant_id)
                    .bind(t.kind.as_str())
                    .bind(t.created_at)
                    .bind(t.expires_at)
                    .bind(t.rotated_at)
                    .bind(t.revoked_at)
                    .execute(self.pool())
                    .await?;
                Ok(())
            }

            async fn get_token(&self, token_hash: &str) -> Result<Option<TokenRow>, sqlx::Error> {
                let sql = $sql(&format!("SELECT {TOKEN_COLS} FROM mcp_oauth_tokens WHERE token_hash = ?"));
                let row = sqlx::query_as::<_, TokenDbRow>(&sql).bind(token_hash).fetch_optional(self.pool()).await?;
                Ok(row.and_then(token_from_row))
            }

            async fn rotate_refresh(
                &self,
                old_hash: &str,
                new_access: &TokenRow,
                new_refresh: &TokenRow,
                at: DateTime<Utc>,
            ) -> Result<RotateOutcome, sqlx::Error> {
                let mut tx = self.pool().begin().await?;
                let sql = $sql(
                    "UPDATE mcp_oauth_tokens SET rotated_at = ? WHERE token_hash = ? AND kind = 'refresh' \
                     AND rotated_at IS NULL AND revoked_at IS NULL AND expires_at > ?",
                );
                let claimed = sqlx::query(&sql).bind(at).bind(old_hash).bind(at).execute(&mut *tx).await?.rows_affected() == 1;
                if !claimed {
                    let sql = $sql(&format!("SELECT {TOKEN_COLS} FROM mcp_oauth_tokens WHERE token_hash = ?"));
                    let row = sqlx::query_as::<_, TokenDbRow>(&sql).bind(old_hash).fetch_optional(&mut *tx).await?;
                    tx.rollback().await?;
                    return Ok(match row.and_then(token_from_row) {
                        Some(t) if t.kind == TokenKind::Refresh && t.rotated_at.is_some() => RotateOutcome::Reused { grant_id: t.grant_id },
                        _ => RotateOutcome::Missing,
                    });
                }
                let sql = $sql(&format!("INSERT INTO mcp_oauth_tokens ({TOKEN_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?)"));
                for t in [new_access, new_refresh] {
                    sqlx::query(&sql)
                        .bind(&t.token_hash)
                        .bind(&t.grant_id)
                        .bind(t.kind.as_str())
                        .bind(t.created_at)
                        .bind(t.expires_at)
                        .bind(t.rotated_at)
                        .bind(t.revoked_at)
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
                Ok(RotateOutcome::Rotated)
            }

            async fn revoke_token(&self, token_hash: &str, at: DateTime<Utc>) -> Result<(), sqlx::Error> {
                let sql = $sql("UPDATE mcp_oauth_tokens SET revoked_at = ? WHERE token_hash = ? AND revoked_at IS NULL");
                sqlx::query(&sql).bind(at).bind(token_hash).execute(self.pool()).await?;
                Ok(())
            }

            async fn record_assertion(&self, client_id: &str, jti: &str, expires_at: DateTime<Utc>) -> Result<bool, sqlx::Error> {
                let sql = $sql("INSERT INTO mcp_oauth_assertions (client_id, jti, expires_at) VALUES (?, ?, ?) ON CONFLICT DO NOTHING");
                let inserted = sqlx::query(&sql).bind(client_id).bind(jti).bind(expires_at).execute(self.pool()).await?.rows_affected();
                Ok(inserted == 1)
            }

            async fn purge_expired(&self, now: DateTime<Utc>) -> Result<u64, sqlx::Error> {
                let spent = now - KEEP_SPENT;
                let mut removed = 0;
                // Grants whose org or app was deleted: refused already, now gone.
                for table in ["mcp_oauth_tokens WHERE grant_id", "mcp_oauth_grants WHERE id"] {
                    let sql = $sql(&format!("DELETE FROM {table} IN ({ORPHANED_GRANTS})"));
                    removed += sqlx::query(&sql).bind(spent).execute(self.pool()).await?.rows_affected();
                }
                let sql = $sql("DELETE FROM mcp_oauth_requests WHERE expires_at < ?");
                removed += sqlx::query(&sql).bind(spent).execute(self.pool()).await?.rows_affected();
                let sql = $sql("DELETE FROM mcp_oauth_tokens WHERE expires_at < ? OR revoked_at < ?");
                removed += sqlx::query(&sql).bind(spent).bind(spent).execute(self.pool()).await?.rows_affected();
                let sql = $sql(
                    "DELETE FROM mcp_oauth_grants WHERE revoked_at < ? \
                     AND NOT EXISTS (SELECT 1 FROM mcp_oauth_tokens t WHERE t.grant_id = mcp_oauth_grants.id)",
                );
                removed += sqlx::query(&sql).bind(spent).execute(self.pool()).await?.rows_affected();

                // Registrations: never used, or idle with no grant left.
                let sql = $sql(&format!(
                    "DELETE FROM mcp_oauth_clients WHERE registration = 'dcr' AND last_used_at IS NULL AND created_at < ? AND {CLIENT_UNREFERENCED}"
                ));
                removed += sqlx::query(&sql).bind(now - UNUSED_CLIENT).execute(self.pool()).await?.rows_affected();
                let sql = $sql(&format!(
                    "DELETE FROM mcp_oauth_clients WHERE registration = 'dcr' AND last_used_at < ? AND {CLIENT_UNREFERENCED}"
                ));
                removed += sqlx::query(&sql).bind(now - IDLE_CLIENT).execute(self.pool()).await?.rows_affected();
                // Copies of metadata documents: fetched again when needed.
                let sql = $sql(&format!(
                    "DELETE FROM mcp_oauth_clients WHERE registration = 'cimd' AND refreshed_at < ? AND {CLIENT_UNREFERENCED}"
                ));
                removed += sqlx::query(&sql).bind(spent).execute(self.pool()).await?.rows_affected();

                let sql = $sql("DELETE FROM mcp_oauth_assertions WHERE expires_at < ?");
                removed += sqlx::query(&sql).bind(spent).execute(self.pool()).await?.rows_affected();
                Ok(removed)
            }
        }
    };
}

impl_mcp_oauth_store!(SqliteDatabase, identity_sql);
impl_mcp_oauth_store!(PostgresDatabase, pg_sql);
