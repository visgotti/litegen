use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::QueryBuilder;
use tracing::info;
use uuid::Uuid;

use crate::types::*;

use super::sqlite::{
    api_key_from_row, application_from_row, audit_log_from_row, build_proxy_stats,
    generation_from_row, invitation_from_row, org_with_role_from_row, organization_from_row,
    org_member_from_row,
    password_reset_from_row, provider_credential_from_row, request_log_from_row, session_from_row,
    user_from_row, webhook_delivery_from_row, ApiKeyRow, ApplicationRow, AuditLogRow, REQUEST_LOG_COLS,
    GenerationRow, InvitationRow, ModelUsageRow, OrgMemberRow, OrgWithRoleRow, OrganizationRow,
    PasswordResetRow, ProviderCredentialRow, ProviderUsageRow, RequestLogRow, SessionRow, UserRow,
    WebhookDeliveryRow,
};
use super::sqlite::{
    api_key_owner_from_row, generation_access_from_row, key_cols_of_k, session_user_from_row, ApiKeyOwnerRow,
    GenerationAccessRow, SessionUserRow, GENERATION_ACCESS_COLS, SESSION_USER_COLS,
};
use super::trait_def::DatabaseStore;

/// Full column list for api_keys selects (Postgres).
const API_KEY_COLS: &str = "id, name, key_hash, key_prefix, created_at, expires_at, is_active, \
    token_quota, tokens_used, rpm_limit, scopes, webhook_url, owner_user_id, org_id, app_id, public_id";

/// Full column list for generations selects (Postgres).
const GENERATION_COLS: &str = "id, key_id, model, provider, media_type, status, progress, \
    provider_job_id, result_url, error_message, cost_usd, created_at, completed_at, metadata, \
    org_id, app_id, files_deleted_at, files_deleted_reason";

/// PostgreSQL-backed database implementation.
pub struct PostgresDatabase {
    pool: PgPool,
    /// Tests only: how far "the database's clock" is set from the server's,
    /// standing in for a replica whose own clock is off.
    #[cfg(test)]
    clock_offset: chrono::Duration,
}

/// How long a statement on one of litegen's pooled Postgres connections may
/// run, and wait for a lock ([`PostgresDatabase::connect`]). Without them a
/// statement blocked behind another transaction's lock waits for as long as
/// that transaction runs, holding a pool connection (and in the poller, a
/// slot) all the while.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StatementLimits {
    /// `statement_timeout`.
    pub statement: std::time::Duration,
    /// `lock_timeout`.
    pub lock: std::time::Duration,
}

impl StatementLimits {
    /// 5 minutes leaves room for the longest statements litegen runs, the
    /// bulk deletes when an org or an app with many rows is deleted. 30
    /// seconds is as long as the poller gives a vendor to answer; only a long
    /// delete, a migration or a transaction left open holds a lock that long.
    /// The migrations run without either ([`PostgresDatabase::connect`]).
    pub(crate) const DEFAULT: Self = Self {
        statement: std::time::Duration::from_secs(5 * 60),
        lock: std::time::Duration::from_secs(30),
    };

    /// Set the limits on `conn`, each only where nothing else has set it: a
    /// limit of the URL's `options`, of `PGOPTIONS`, or of the role, the
    /// database or the server's configuration wins, a `0` too
    /// (`pg_settings.source` is `default` only when none set it).
    async fn apply(self, conn: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
        sqlx::query(
            "SELECT set_config(name, CASE name WHEN 'statement_timeout' THEN $1 ELSE $2 END, false) \
             FROM pg_settings WHERE name IN ('statement_timeout', 'lock_timeout') AND source = 'default'",
        )
        .bind(format!("{}ms", self.statement.as_millis()))
        .bind(format!("{}ms", self.lock.as_millis()))
        .execute(conn)
        .await?;
        Ok(())
    }
}

/// No statement limit for the rest of `tx`: deleting an org or an app, whose
/// statements take as long as its rows are many, and which a limit would
/// otherwise leave undeletable. `SET LOCAL` ends with the transaction, so the
/// pooled connection keeps its [`StatementLimits`]; the lock limit stays.
async fn lift_statement_timeout(tx: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query("SET LOCAL statement_timeout = 0").execute(tx).await?;
    Ok(())
}

/// Record, in `tx`, each file litegen kept for the generations of org
/// (`scope` "org") or app ("app") `id` whose files aren't deleted yet, for
/// `proxy::tenant_files` to delete from litegen's store once their rows are
/// gone (migration 031), as SQLite's does. The generations are read FOR
/// UPDATE: a job finishing meanwhile waits for this transaction, and then
/// finds its row gone, so no generation's files change between this read and
/// its delete.
async fn record_tenant_files(tx: &mut sqlx::PgConnection, scope: &str, id: &str) -> Result<(), sqlx::Error> {
    use futures::TryStreamExt;
    let column = if scope == "org" { "org_id" } else { "app_id" };
    let sql = format!("SELECT id, result_url, metadata FROM generations WHERE {column} = $1 AND files_deleted_at IS NULL FOR UPDATE");
    // Read as a stream: only the URLs are kept, never every row's metadata.
    let mut files: Vec<(String, String)> = Vec::new();
    {
        let mut rows = sqlx::query_as::<_, (String, Option<String>, Option<String>)>(&sql).bind(id).fetch(&mut *tx);
        while let Some((generation, result_url, metadata)) = rows.try_next().await? {
            for url in super::trait_def::kept_file_urls(result_url.as_deref(), metadata.as_deref()) {
                files.push((generation.clone(), url));
            }
        }
    }
    insert_tenant_files(tx, scope, id, &files).await
}

/// Insert `files` (generation, URL) as file deletions of `scope` `tenant`,
/// due now, as SQLite's does.
async fn insert_tenant_files(conn: &mut sqlx::PgConnection, scope: &str, tenant: &str, files: &[(String, String)]) -> Result<(), sqlx::Error> {
    for chunk in files.chunks(super::sqlite::TENANT_FILES_PER_INSERT) {
        let mut insert = QueryBuilder::new("INSERT INTO tenant_file_deletions (scope, tenant_id, generation_id, url) ");
        insert.push_values(chunk, |mut row, (generation, url)| {
            row.push_bind(scope).push_bind(tenant).push_bind(generation.as_str()).push_bind(url.as_str());
        });
        insert.build().execute(&mut *conn).await?;
    }
    Ok(())
}

/// How many times an org's or app's delete runs, at most, when rows written
/// meanwhile get in its way ([`a_row_arrived`]).
const TENANT_DELETE_TRIES: u32 = 5;

/// Whether `e` is a tenant delete that a row written while it ran got in the
/// way of: a request in flight committed a row of the org or app (its log
/// row, say) after the delete's child deletes and before the parent's, which
/// then hit it (23503), or the two deadlocked (40P01). Run again from the
/// start, the delete takes that row too.
fn a_row_arrived(e: &sqlx::Error) -> bool {
    matches!(e.as_database_error().and_then(|d| d.code()).as_deref(), Some("23503") | Some("40P01"))
}

impl PostgresDatabase {
    /// One try of `delete_organization`: its rows in one transaction.
    async fn delete_organization_once(&self, id: &str) -> Result<bool, sqlx::Error> {
        // Cascade in one transaction, in FK-dependency order. On Postgres the
        // tenant columns DO carry FKs to organizations/applications without
        // ON DELETE CASCADE, so a bare `DELETE FROM organizations` fails once the
        // org has its auto-created default app + owner membership. Delete the
        // children first (generations references api_keys → before api_keys).
        let mut tx = self.pool.begin().await?;
        lift_statement_timeout(&mut tx).await?;
        for stmt in [
            "DELETE FROM webhook_deliveries WHERE org_id = $1",
            "DELETE FROM webhook_outbox WHERE org_id = $1",
            "DELETE FROM request_artifacts WHERE org_id = $1",
            // Its requests' spend tags, before the rows they hang off.
            "DELETE FROM request_log_tags WHERE request_id IN (SELECT id FROM request_logs WHERE org_id = $1)",
            "DELETE FROM request_logs WHERE org_id = $1",
        ] {
            sqlx::query(stmt).bind(id).execute(&mut *tx).await?;
        }
        // The files its generations kept, recorded before their rows go
        // (migration 031): deleted from litegen's store after the commit,
        // until they're gone.
        record_tenant_files(&mut tx, "org", id).await?;
        for stmt in [
            "DELETE FROM generations WHERE org_id = $1",
            "DELETE FROM api_keys WHERE org_id = $1",
            "DELETE FROM audit_log WHERE org_id = $1",
            "DELETE FROM invitations WHERE org_id = $1",
            "DELETE FROM org_provider_credentials WHERE org_id = $1",
            "DELETE FROM org_allowed_models WHERE org_id = $1",
            // Migrations 042/043's triggers delete unused reset links here (see 043's header).
            "DELETE FROM organization_members WHERE org_id = $1",
        ] {
            sqlx::query(stmt).bind(id).execute(&mut *tx).await?;
        }
        for stmt in [
            "DELETE FROM app_model_access_ids WHERE app_id IN (SELECT id FROM applications WHERE org_id = $1)",
            "DELETE FROM app_model_access WHERE app_id IN (SELECT id FROM applications WHERE org_id = $1)",
            "DELETE FROM app_storage_credentials WHERE app_id IN (SELECT id FROM applications WHERE org_id = $1)",
            "DELETE FROM app_file_access WHERE app_id IN (SELECT id FROM applications WHERE org_id = $1)",
        ] {
            sqlx::query(stmt).bind(id).execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM applications WHERE org_id = $1").bind(id).execute(&mut *tx).await?;
        let result = sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    /// One try of `delete_application`: its rows in one transaction.
    async fn delete_application_once(&self, id: &str) -> Result<bool, sqlx::Error> {
        // Cascade in one transaction, in FK-dependency order. The app_id columns
        // on api_keys/generations/request_logs/request_artifacts/webhook_deliveries
        // and app_storage_credentials reference applications WITHOUT ON DELETE
        // CASCADE, so a bare `DELETE FROM applications` fails the FK check once the
        // app has issued a key or run a generation, leaving used apps permanently
        // undeletable. Delete the app-scoped children first (generations
        // references api_keys → before api_keys), mirroring delete_organization.
        let mut tx = self.pool.begin().await?;
        lift_statement_timeout(&mut tx).await?;
        for stmt in [
            "DELETE FROM webhook_deliveries WHERE app_id = $1",
            "DELETE FROM webhook_outbox WHERE app_id = $1",
            "DELETE FROM request_artifacts WHERE app_id = $1",
            // Its requests' spend tags, before the rows they hang off.
            "DELETE FROM request_log_tags WHERE request_id IN (SELECT id FROM request_logs WHERE app_id = $1)",
            "DELETE FROM request_logs WHERE app_id = $1",
        ] {
            sqlx::query(stmt).bind(id).execute(&mut *tx).await?;
        }
        // The files its generations kept, recorded before their rows go, as
        // delete_organization does.
        record_tenant_files(&mut tx, "app", id).await?;
        for stmt in [
            "DELETE FROM generations WHERE app_id = $1",
            "DELETE FROM api_keys WHERE app_id = $1",
            "DELETE FROM app_model_access_ids WHERE app_id = $1",
            "DELETE FROM app_model_access WHERE app_id = $1",
            "DELETE FROM app_storage_credentials WHERE app_id = $1",
            "DELETE FROM app_file_access WHERE app_id = $1",
        ] {
            sqlx::query(stmt).bind(id).execute(&mut *tx).await?;
        }
        let result = sqlx::query("DELETE FROM applications WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }
}

impl PostgresDatabase {
    /// Connect to PostgreSQL and run migrations.
    ///
    /// The migrations run first, on a connection of their own without
    /// [`StatementLimits`], so a long one (an index built on a large table)
    /// is never cut. Every connection of the pool then has the limits.
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        Self::connect_with_limits(database_url, StatementLimits::DEFAULT).await
    }

    /// [`connect`](Self::connect), with `limits` (tests shorten them).
    pub(crate) async fn connect_with_limits(database_url: &str, limits: StatementLimits) -> Result<Self, sqlx::Error> {
        let options: sqlx::postgres::PgConnectOptions = database_url.parse()?;
        // A pool of one connection, not a bare one: a migration run on a
        // `&mut PgConnection` makes a future that `tokio::spawn` refuses.
        let migrating = PgPoolOptions::new().max_connections(1).connect_with(options.clone()).await?;
        sqlx::migrate!("./migrations/postgres").run(&migrating).await?;
        migrating.close().await;

        let pool = PgPoolOptions::new()
            .max_connections(10)
            .after_connect(move |conn, _meta| Box::pin(async move { limits.apply(conn).await }))
            .connect_with(options)
            .await?;
        info!("PostgreSQL database connected and migrations applied");

        Ok(Self {
            pool,
            #[cfg(test)]
            clock_offset: chrono::Duration::zero(),
        })
    }

    /// Tests only: this database's clock, `offset` from the server's.
    #[cfg(test)]
    pub(crate) fn with_clock_offset(mut self, offset: chrono::Duration) -> Self {
        self.clock_offset = offset;
        self
    }

    /// Seconds added to `now()` wherever the database's clock is read: the
    /// tests' offset, 0 outside tests. The SQL is the same in both.
    #[cfg(test)]
    fn clock_offset_secs(&self) -> f64 {
        self.clock_offset.num_milliseconds() as f64 / 1000.0
    }

    #[cfg(not(test))]
    fn clock_offset_secs(&self) -> f64 {
        0.0
    }

    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Write `after` as generation `id`'s metadata only if it is still `before`
    /// and the row is in flight: one compare-and-swap step. `true` when it took.
    async fn swap_metadata(&self, id: &str, before: Option<&str>, after: &serde_json::Value) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE generations SET metadata = $1 \
                 WHERE id = $2 AND status IN ('pending', 'processing') AND metadata IS NOT DISTINCT FROM $3",
        )
        .bind(serde_json::to_string(after).unwrap_or_else(|_| "{}".into()))
        .bind(id)
        .bind(before)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// A row's status and its `metadata` exactly as stored: the "before" a
    /// compare-and-swap on the column compares against. Also the database's
    /// clock as it read them, which leases and stage claims are stamped and
    /// judged with, at no extra round trip.
    async fn status_and_metadata(&self, id: &str) -> Result<Option<(String, Option<String>, chrono::DateTime<chrono::Utc>)>, sqlx::Error> {
        sqlx::query_as("SELECT status, metadata, now() + make_interval(secs => $2) FROM generations WHERE id = $1")
            .bind(id)
            .bind(self.clock_offset_secs())
            .fetch_optional(&self.pool)
            .await
    }
}

/// Attempts a compare-and-swap on `generations.metadata` makes before giving
/// up. Each failed attempt means another writer changed the column between
/// the read and the write, which only a burst of writers can sustain.
const METADATA_CAS_ATTEMPTS: usize = 32;

fn parse_metadata(text: Option<&str>) -> Option<serde_json::Value> {
    text.and_then(|t| serde_json::from_str(t).ok())
}

fn is_in_flight(status: &str) -> bool {
    status == "pending" || status == "processing"
}

fn metadata_cas_exhausted(id: &str) -> sqlx::Error {
    sqlx::Error::Protocol(format!(
        "metadata of generation {id} kept changing under {METADATA_CAS_ATTEMPTS} write attempts"
    ))
}

/// The query that finds the sign-in `from` live (review round 6, finding
/// 2), with its id bound as `$param`; none for the master key, which always
/// is. Under READ COMMITTED a plain read would see a snapshot, so it takes a
/// share lock on the sender's row: a reset, a deactivation or a password
/// change of that account (each an `UPDATE` of the row, its first write)
/// that is under way makes the query wait for its commit, then re-check the
/// row, and fail; one that starts after waits for this transaction's
/// commit, and its cleanup then sees what was stored. A session's row is
/// locked too, after the account's (the order those changes write them in,
/// so the two never deadlock): its own password change re-stamps the session
/// it keeps, and the re-check then reads that new row, not the one from
/// before the wait (review round 7, finding 5).
fn live_sign_in_query(from: crate::db::SignIn<'_>, param: usize) -> Option<(String, &str)> {
    use crate::db::SignIn;
    match from {
        SignIn::MasterKey => None,
        SignIn::Session(id) => Some((
            format!(
                "SELECT 1 FROM sessions s JOIN users u ON u.id = s.user_id \
                 WHERE s.id = ${param} AND u.is_active AND s.sign_in_epoch = u.sign_in_epoch FOR SHARE OF u, s"
            ),
            id,
        )),
        SignIn::Grant(id) => Some((
            format!(
                "SELECT 1 FROM mcp_oauth_grants g JOIN users u ON u.id = g.user_id \
                 WHERE g.id = ${param} AND g.revoked_at IS NULL AND u.is_active AND g.sign_in_epoch = u.sign_in_epoch FOR SHARE OF u"
            ),
            id,
        )),
    }
}

/// The clause that makes an insert (`INSERT … SELECT <values>`) land only
/// while the sign-in `from` is live ([`live_sign_in_query`]), with its id
/// bound as `$param`; nothing for the master key.
fn live_sign_in_guard(from: crate::db::SignIn<'_>, param: usize) -> (String, Option<&str>) {
    match live_sign_in_query(from, param) {
        Some((query, id)) => (format!(" WHERE EXISTS ({query})"), Some(id)),
        None => (String::new(), None),
    }
}

/// Delete the pending invitations, in `org_id` or every org, that go to
/// `email` or that `invited_by` sent, through `executor` (the pool, or a
/// deactivation's or a confirm's transaction), and return them: the one
/// statement behind every revoke (review round 6, finding 6).
async fn revoke_pending_invitations_in<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
    executor: E,
    org_id: Option<&str>,
    email: Option<&str>,
    invited_by: Option<&str>,
) -> Result<Vec<Invitation>, sqlx::Error> {
    let rows: Vec<InvitationRow> = sqlx::query_as(
        "DELETE FROM invitations WHERE used_at IS NULL AND ($1::text IS NULL OR org_id = $1) AND (email = $2 OR invited_by = $3) \
         RETURNING id, email, role, token, invited_by, org_id, expires_at, used_at, created_at",
    )
    .bind(org_id)
    .bind(email)
    .bind(invited_by)
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(invitation_from_row).collect())
}

/// Ends every sign-in `user_id` has, in `tx`: its sessions (all but
/// `keep_session`), its OAuth grants with their tokens, and its OAuth codes
/// not yet redeemed, and moves its sign-in epoch on, so a sign-in begun
/// before and landing after is refused too. Deactivating an account,
/// confirming a reset and changing one's own password do it in the
/// transaction that makes the change (review round 3b, S1 and S2; round 4,
/// findings 2 and 6), so either all of it lands or none. `keep_session` is
/// re-stamped with the new epoch.
async fn end_sign_ins(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, user_id: &str, keep_session: Option<&str>) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET sign_in_epoch = sign_in_epoch + 1 WHERE id = $1").bind(user_id).execute(&mut **tx).await?;
    match keep_session {
        Some(keep) => {
            sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND id <> $2").bind(user_id).bind(keep).execute(&mut **tx).await?;
            sqlx::query("UPDATE sessions SET sign_in_epoch = (SELECT sign_in_epoch FROM users WHERE id = $1) WHERE id = $2 AND user_id = $1")
                .bind(user_id)
                .bind(keep)
                .execute(&mut **tx)
                .await?;
        }
        None => {
            sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(user_id).execute(&mut **tx).await?;
        }
    }
    sqlx::query("UPDATE mcp_oauth_tokens SET revoked_at = NOW() WHERE revoked_at IS NULL AND grant_id IN (SELECT id FROM mcp_oauth_grants WHERE user_id = $1)")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE mcp_oauth_grants SET revoked_at = NOW() WHERE user_id = $1 AND revoked_at IS NULL").bind(user_id).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM mcp_oauth_requests WHERE user_id = $1 AND consumed_at IS NULL").bind(user_id).execute(&mut **tx).await?;
    // And the unused links it issued, from a stolen session say (review
    // round 5, finding 4).
    sqlx::query("DELETE FROM password_resets WHERE issued_by = $1 AND used_at IS NULL").bind(user_id).execute(&mut **tx).await?;
    Ok(())
}

/// The account `id` through `executor`: the pool (`get_user_by_id`) or a
/// transaction (`update_user_with_revoked`'s read-back before its commit).
async fn fetch_user<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(executor: E, id: &str) -> Result<Option<User>, sqlx::Error> {
    let row = sqlx::query_as::<_, UserRow>(
        "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
         created_at, updated_at, last_login_at, is_active, sign_in_epoch \
         FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    Ok(row.map(user_from_row))
}

/// Insert `user` through `executor`: the pool (`create_user`) or the first
/// signup's transaction (`create_first_owner`).
async fn insert_user<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(executor: E, user: &User) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO users
            (id, email, password_hash, role, oauth_github_id, oauth_google_id,
             created_at, updated_at, last_login_at, is_active)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(&user.id)
    .bind(&user.email)
    .bind(&user.password_hash)
    .bind(user.role.as_str())
    .bind(&user.oauth_github_id)
    .bind(&user.oauth_google_id)
    .bind(user.created_at)
    .bind(user.updated_at)
    .bind(user.last_login_at)
    .bind(user.is_active)
    .execute(executor)
    .await?;
    Ok(())
}

/// Insert `o` through `executor`: the pool (`create_organization`) or a
/// transaction (`create_org_with_owner`).
async fn insert_organization<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(executor: E, o: &Organization) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO organizations (id, name, slug, plan, status, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&o.id)
    .bind(&o.name)
    .bind(&o.slug)
    .bind(&o.plan)
    .bind(&o.status)
    .bind(o.created_at)
    .bind(o.updated_at)
    .execute(executor)
    .await?;
    Ok(())
}

/// Insert `a` through `executor`: the pool (`create_application`) or a
/// transaction (`create_org_with_owner`).
async fn insert_application<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(executor: E, a: &Application) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO applications (id, org_id, name, slug, status, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&a.id)
    .bind(&a.org_id)
    .bind(&a.name)
    .bind(&a.slug)
    .bind(&a.status)
    .bind(a.created_at)
    .bind(a.updated_at)
    .execute(executor)
    .await?;
    Ok(())
}

/// Claim an `Idempotency-Key` ([`DatabaseStore::idempotency_begin_held`]),
/// the record held by `held_by` for `lease`. A record in progress is free once
/// its lease lapsed; one with no lease (an older replica's, or one from before
/// migration 030) once it is older than `IDEMPOTENCY_IN_PROGRESS_STALE`.
async fn begin_idempotency(
    db: &PostgresDatabase,
    scope: &str,
    key: &str,
    request_hash: &str,
    held_by: &str,
    lease: chrono::Duration,
) -> Result<crate::db::IdempotencyBegin, sqlx::Error> {
    use crate::db::IdempotencyBegin;
    // Every time here is the database's clock (`now()`), never this
    // replica's: a replica whose clock runs fast would otherwise judge
    // another's in-flight run stale and run the request again. No extra
    // round trip: the purge and the insert use `now()` themselves, and the
    // record is read with it.
    let offset = db.clock_offset_secs();
    // Opportunistic purge: the index on created_at keeps this cheap.
    sqlx::query("DELETE FROM idempotency_keys WHERE created_at < now() + make_interval(secs => $1)")
        .bind(offset - crate::db::IDEMPOTENCY_TTL.num_milliseconds() as f64 / 1000.0)
        .execute(&db.pool)
        .await?;
    for _ in 0..4 {
        let inserted = sqlx::query(
            "INSERT INTO idempotency_keys (scope, idem_key, request_hash, state, created_at, held_by, held_until) \
             VALUES ($1, $2, $3, 'in_progress', now() + make_interval(secs => $4), $5, now() + make_interval(secs => $4 + $6)) \
             ON CONFLICT (scope, idem_key) DO NOTHING",
        )
        .bind(scope)
        .bind(key)
        .bind(request_hash)
        .bind(offset)
        .bind(held_by)
        .bind(lease.num_milliseconds() as f64 / 1000.0)
        .execute(&db.pool)
        .await?;
        if inserted.rows_affected() > 0 {
            return Ok(IdempotencyBegin::Started);
        }
        #[allow(clippy::type_complexity)]
        let row: Option<(String, String, Option<i64>, Option<String>, Option<Vec<u8>>, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>, Option<bool>)> = sqlx::query_as(
            "SELECT request_hash, state, CAST(response_status AS BIGINT), response_headers, response_body, created_at, \
                    now() + make_interval(secs => $3), held_until < now() + make_interval(secs => $3) \
             FROM idempotency_keys WHERE scope = $1 AND idem_key = $2",
        )
        .bind(scope)
        .bind(key)
        .bind(offset)
        .fetch_optional(&db.pool)
        .await?;
        let Some((hash, state, status, headers, body, created_at, now, lapsed)) = row else { continue };
        // In progress: a leased record until its lease lapses, one with
        // no lease (NULL) for the stale window.
        let abandoned = match lapsed {
            Some(lapsed) => lapsed,
            None => now - created_at > crate::db::IDEMPOTENCY_IN_PROGRESS_STALE,
        };
        let expired = now - created_at > crate::db::IDEMPOTENCY_TTL || (state == "in_progress" && abandoned);
        if expired {
            // Only the record we looked at: a fresh one a racer just made stays.
            sqlx::query("DELETE FROM idempotency_keys WHERE scope = $1 AND idem_key = $2 AND created_at = $3")
                .bind(scope)
                .bind(key)
                .bind(created_at)
                .execute(&db.pool)
                .await?;
            continue;
        }
        if hash != request_hash {
            return Ok(IdempotencyBegin::Conflict);
        }
        if state != "completed" {
            return Ok(IdempotencyBegin::InProgress);
        }
        let headers: Vec<(String, String)> = headers
            .and_then(|h| serde_json::from_str(&h).ok())
            .unwrap_or_default();
        return Ok(IdempotencyBegin::Replay(crate::db::StoredResponse {
            status: status.unwrap_or(200).clamp(100, 599) as u16,
            headers,
            body: body.unwrap_or_default(),
        }));
    }
    Err(sqlx::Error::Protocol(format!("idempotency key {key} kept changing under four claim attempts")))
}

#[async_trait]
impl DatabaseStore for PostgresDatabase {
    fn management(&self) -> Option<&dyn crate::management::store::ManagementStore> {
        Some(self)
    }

    fn mcp_oauth(&self) -> Option<&dyn crate::mcp::oauth::store::McpOAuthStore> {
        Some(self)
    }

    fn reservations(&self) -> Option<&dyn crate::management::reservations::ReservationStore> {
        Some(self)
    }

    // ─── Generations ────────────────────────────────────────────────────────────

    async fn insert_generation(
        &self,
        id: &str,
        key_id: Option<&Uuid>,
        model: &str,
        provider: &str,
        media_type: &str,
        provider_job_id: Option<&str>,
        cost_usd: f64,
        org_id: Option<&str>,
        app_id: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO generations
                (id, key_id, model, provider, media_type, status, progress,
                 provider_job_id, cost_usd, org_id, app_id, created_at)
            VALUES ($1, $2, $3, $4, $5, 'pending', 0, $6, $7, COALESCE($8, $9), COALESCE($10, $11), NOW())
            "#,
        )
        .bind(id)
        .bind(key_id.map(|u| u.to_string()))
        .bind(model)
        .bind(provider)
        .bind(media_type)
        .bind(provider_job_id)
        .bind(cost_usd)
        .bind(org_id)
        .bind(crate::api::middleware::DEFAULT_ORG_ID)
        .bind(app_id)
        .bind(crate::api::middleware::DEFAULT_APP_ID)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_generation_status(
        &self,
        id: &str,
        status: &str,
        progress: i32,
        result_url: Option<&str>,
        error: Option<&str>,
        completed_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE generations
            SET status = $1, progress = $2, result_url = $3,
                error_message = $4, completed_at = $5
            WHERE id = $6
            "#,
        )
        .bind(status)
        .bind(progress)
        .bind(result_url)
        .bind(error)
        .bind(completed_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_generation_metadata(
        &self,
        id: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE generations SET metadata = $1 WHERE id = $2")
            .bind(serde_json::to_string(metadata).unwrap_or_else(|_| "null".into()))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn idempotency_begin_held(
        &self,
        scope: &str,
        key: &str,
        request_hash: &str,
        held_by: &str,
        lease: chrono::Duration,
    ) -> Result<crate::db::IdempotencyBegin, sqlx::Error> {
        begin_idempotency(self, scope, key, request_hash, held_by, lease).await
    }

    async fn idempotency_complete_held(&self, scope: &str, key: &str, held_by: &str, response: &crate::db::StoredResponse) -> Result<(), sqlx::Error> {
        // Only the record this request holds: after its lease lapsed, a
        // retry's record has another holder.
        sqlx::query(
            "UPDATE idempotency_keys SET state = 'completed', response_status = $1, response_headers = $2, response_body = $3 \
             WHERE scope = $4 AND idem_key = $5 AND held_by = $6",
        )
        .bind(i32::from(response.status))
        .bind(serde_json::to_string(&response.headers).unwrap_or_else(|_| "[]".into()))
        .bind(&response.body)
        .bind(scope)
        .bind(key)
        .bind(held_by)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn idempotency_release_held(&self, scope: &str, key: &str, held_by: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM idempotency_keys WHERE scope = $1 AND idem_key = $2 AND state = 'in_progress' AND held_by = $3")
            .bind(scope)
            .bind(key)
            .bind(held_by)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn merge_generation_metadata_if_active(
        &self,
        id: &str,
        patch: &serde_json::Value,
    ) -> Result<crate::db::GenerationWrite, sqlx::Error> {
        for _ in 0..METADATA_CAS_ATTEMPTS {
            let Some((status, before, _)) = self.status_and_metadata(id).await? else {
                return Ok(crate::db::GenerationWrite::Missing);
            };
            if !is_in_flight(&status) {
                return Ok(crate::db::GenerationWrite::AlreadyTerminal);
            }
            let merged = crate::db::merge_metadata(parse_metadata(before.as_deref()).as_ref(), patch);
            let res = sqlx::query(
                "UPDATE generations SET metadata = $1 \
                 WHERE id = $2 AND status IN ('pending', 'processing') AND metadata IS NOT DISTINCT FROM $3",
            )
            .bind(serde_json::to_string(&merged).unwrap_or_else(|_| "{}".into()))
            .bind(id)
            .bind(before.as_deref())
            .execute(&self.pool)
            .await?;
            if res.rows_affected() > 0 {
                return Ok(crate::db::GenerationWrite::Written);
            }
        }
        Err(metadata_cas_exhausted(id))
    }

    async fn claim_generation_stage(&self, id: &str, stage: &str, claimant: &str) -> Result<bool, sqlx::Error> {
        for _ in 0..METADATA_CAS_ATTEMPTS {
            let Some((status, before, now)) = self.status_and_metadata(id).await? else { return Ok(false) };
            if !is_in_flight(&status) {
                return Ok(false);
            }
            let meta = parse_metadata(before.as_deref());
            let claims = meta.as_ref().and_then(|m| m.get("stage_claims"));
            if claims.and_then(|c| c.get(stage)).is_some() {
                return Ok(false);
            }
            let mut claims = claims.filter(|c| c.is_object()).cloned().unwrap_or_else(|| serde_json::json!({}));
            claims[stage] = crate::db::stage_claim_record(claimant, now);
            let merged = crate::db::merge_metadata(meta.as_ref(), &serde_json::json!({ "stage_claims": claims }));
            let res = sqlx::query(
                "UPDATE generations SET metadata = $1 \
                 WHERE id = $2 AND status IN ('pending', 'processing') AND metadata IS NOT DISTINCT FROM $3",
            )
            .bind(serde_json::to_string(&merged).unwrap_or_else(|_| "{}".into()))
            .bind(id)
            .bind(before.as_deref())
            .execute(&self.pool)
            .await?;
            if res.rows_affected() > 0 {
                return Ok(true);
            }
        }
        Err(metadata_cas_exhausted(id))
    }

    async fn lease_generation_work(&self, id: &str, work: &str, claimant: &str, ttl: chrono::Duration) -> Result<bool, sqlx::Error> {
        for _ in 0..METADATA_CAS_ATTEMPTS {
            let Some((status, before, now)) = self.status_and_metadata(id).await? else { return Ok(true) };
            if !is_in_flight(&status) {
                return Ok(false);
            }
            let meta = parse_metadata(before.as_deref());
            if !crate::db::lease_is_free(meta.as_ref(), work, claimant, ttl, now) {
                return Ok(false);
            }
            let merged = crate::db::merge_metadata(meta.as_ref(), &crate::db::leases_patch(meta.as_ref(), work, Some(claimant), now));
            if self.swap_metadata(id, before.as_deref(), &merged).await? {
                return Ok(true);
            }
        }
        Err(metadata_cas_exhausted(id))
    }

    async fn release_generation_work(&self, id: &str, work: &str, claimant: &str) -> Result<(), sqlx::Error> {
        for _ in 0..METADATA_CAS_ATTEMPTS {
            let Some((status, before, now)) = self.status_and_metadata(id).await? else { return Ok(()) };
            let meta = parse_metadata(before.as_deref());
            let held = meta.as_ref().and_then(|m| m.get("leases")).and_then(|l| l.get(work)).and_then(|l| l.get("by")).and_then(|b| b.as_str());
            if !is_in_flight(&status) || held != Some(claimant) {
                return Ok(());
            }
            let merged = crate::db::merge_metadata(meta.as_ref(), &crate::db::leases_patch(meta.as_ref(), work, None, now));
            if self.swap_metadata(id, before.as_deref(), &merged).await? {
                return Ok(());
            }
        }
        Err(metadata_cas_exhausted(id))
    }

    async fn update_generation_if_active(
        &self,
        id: &str,
        status: &str,
        progress: i32,
        result_url: Option<&str>,
        error: Option<&str>,
        completed_at: Option<chrono::DateTime<chrono::Utc>>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<crate::db::GenerationWrite, sqlx::Error> {
        // A metadata patch is MERGED into the column (`merge_metadata`), so it
        // goes through the same compare-and-swap as every other metadata write;
        // status, result and the merged metadata still land in ONE statement.
        if let Some(patch) = metadata {
            for _ in 0..METADATA_CAS_ATTEMPTS {
                let Some((current, before, _)) = self.status_and_metadata(id).await? else {
                    return Ok(crate::db::GenerationWrite::Missing);
                };
                if !is_in_flight(&current) {
                    return Ok(crate::db::GenerationWrite::AlreadyTerminal);
                }
                let merged = crate::db::merge_metadata(parse_metadata(before.as_deref()).as_ref(), patch);
                let res = sqlx::query(
                    r#"
                    UPDATE generations
                    SET status = $1, progress = $2, result_url = $3,
                        error_message = $4, completed_at = $5, metadata = $6
                    WHERE id = $7 AND status IN ('pending', 'processing') AND metadata IS NOT DISTINCT FROM $8
                    "#,
                )
                .bind(status)
                .bind(progress)
                .bind(result_url)
                .bind(error)
                .bind(completed_at)
                .bind(serde_json::to_string(&merged).unwrap_or_else(|_| "{}".into()))
                .bind(id)
                .bind(before.as_deref())
                .execute(&self.pool)
                .await?;
                if res.rows_affected() > 0 {
                    return Ok(crate::db::GenerationWrite::Written);
                }
            }
            return Err(metadata_cas_exhausted(id));
        }
        // `COALESCE($6, metadata)` leaves the column alone when no metadata is
        // supplied, so status and assets still land in ONE statement.
        let meta = metadata.map(|m| serde_json::to_string(m).unwrap_or_else(|_| "null".into()));
        let res = sqlx::query(
            r#"
            UPDATE generations
            SET status = $1, progress = $2, result_url = $3,
                error_message = $4, completed_at = $5, metadata = COALESCE($6, metadata)
            WHERE id = $7 AND status IN ('pending', 'processing')
            "#,
        )
        .bind(status)
        .bind(progress)
        .bind(result_url)
        .bind(error)
        .bind(completed_at)
        .bind(meta)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() > 0 {
            return Ok(crate::db::GenerationWrite::Written);
        }
        // See the SQLite twin: zero rows means "another observer got there
        // first" OR "the spawned insert has not landed yet", and the caller
        // must treat those differently.
        let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM generations WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(if exists.is_some() {
            crate::db::GenerationWrite::AlreadyTerminal
        } else {
            crate::db::GenerationWrite::Missing
        })
    }

    async fn get_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error> {
        let sql = format!("SELECT {} FROM generations WHERE id = $1", GENERATION_COLS);
        let row = sqlx::query_as::<_, GenerationRow>(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(generation_from_row))
    }

    async fn list_generations(
        &self,
        key_id: Option<&Uuid>,
        page: u32,
        per_page: u32,
    ) -> Result<Vec<Generation>, sqlx::Error> {
        let offset = crate::api::pagination::offset(page, per_page);
        let rows = if let Some(kid) = key_id {
            let sql = format!(
                "SELECT {} FROM generations WHERE key_id = $1 OR key_id IS NULL \
                 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
                GENERATION_COLS
            );
            sqlx::query_as::<_, GenerationRow>(&sql)
                .bind(kid.to_string())
                .bind(per_page as i64)
                .bind(offset as i64)
                .fetch_all(&self.pool)
                .await?
        } else {
            let sql = format!(
                "SELECT {} FROM generations ORDER BY created_at DESC LIMIT $1 OFFSET $2",
                GENERATION_COLS
            );
            sqlx::query_as::<_, GenerationRow>(&sql)
                .bind(per_page as i64)
                .bind(offset as i64)
                .fetch_all(&self.pool)
                .await?
        };
        Ok(rows.into_iter().map(generation_from_row).collect())
    }

    async fn count_generations(&self, key_id: Option<&Uuid>) -> Result<i64, sqlx::Error> {
        if let Some(kid) = key_id {
            let row: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM generations WHERE key_id = $1 OR key_id IS NULL",
            )
            .bind(kid.to_string())
            .fetch_one(&self.pool)
            .await?;
            Ok(row.0)
        } else {
            let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM generations")
                .fetch_one(&self.pool)
                .await?;
            Ok(row.0)
        }
    }

    async fn cancel_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error> {
        let sql = format!(
            "UPDATE generations \
             SET status = 'cancelled', completed_at = NOW() \
             WHERE id = $1 AND status IN ('pending', 'processing') \
             RETURNING {}",
            GENERATION_COLS
        );
        let row = sqlx::query_as::<_, GenerationRow>(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(generation_from_row))
    }

    async fn list_active_generations(&self, limit: u32) -> Result<Vec<Generation>, sqlx::Error> {
        let sql = format!(
            "SELECT {} FROM generations WHERE status IN ('pending', 'processing') \
             ORDER BY created_at ASC LIMIT $1",
            GENERATION_COLS
        );
        let rows = sqlx::query_as::<_, GenerationRow>(&sql)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(generation_from_row).collect())
    }

    async fn claim_active_generations(
        &self,
        limit: u32,
        claimant: &str,
        hold: chrono::Duration,
    ) -> Result<Vec<Generation>, sqlx::Error> {
        // One statement on the database's clock (`now()`, never a time bound
        // from the app). `SKIP LOCKED` hands rows another claimer is taking
        // right now to nobody, so concurrent replicas never wait on or share
        // a row. The picking query is a MATERIALIZED CTE so it runs exactly
        // once: inside `id IN (SELECT ... LIMIT n)` the planner may rerun it
        // per row (a nested loop) and claim more than `limit` rows. Its column
        // is not called `id`, so the unqualified RETURNING columns stay
        // unambiguous. The rows polled longest ago go first (a row never
        // polled counts as claimed at its creation), so a replica with more
        // active rows than `limit` takes them in turn instead of re-taking
        // the oldest every tick.
        let sql = format!(
            "WITH picked AS MATERIALIZED ( \
                 SELECT id AS picked_id FROM generations \
                 WHERE status IN ('pending', 'processing') \
                   AND (poll_claimed_until IS NULL \
                        OR poll_claimed_until < now() \
                        OR poll_claimed_by = $1) \
                 ORDER BY COALESCE(poll_claimed_until, created_at + make_interval(secs => $2)) ASC, \
                          created_at ASC \
                 LIMIT $3 \
                 FOR UPDATE SKIP LOCKED) \
             UPDATE generations g \
             SET poll_claimed_by = $1, \
                 poll_claimed_until = now() + make_interval(secs => $2) \
             FROM picked \
             WHERE g.id = picked.picked_id \
             RETURNING {}",
            GENERATION_COLS
        );
        let rows = sqlx::query_as::<_, GenerationRow>(&sql)
            .bind(claimant)
            .bind(hold.num_milliseconds() as f64 / 1000.0)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(generation_from_row).collect())
    }

    async fn release_poll_claims(&self, claimant: &str) -> Result<u64, sqlx::Error> {
        // Only active rows: a claim is never cleared when its row turns
        // terminal, so without the status clause every finished row this
        // process ever polled would be rewritten and counted, after a
        // sequential scan. With it Postgres can use `idx_generations_status`.
        let freed = sqlx::query(
            "UPDATE generations SET poll_claimed_by = NULL, poll_claimed_until = NULL \
             WHERE poll_claimed_by = $1 AND status IN ('pending', 'processing')",
        )
            .bind(claimant)
            .execute(&self.pool)
            .await?;
        Ok(freed.rows_affected())
    }

    async fn renew_poll_claims(&self, claimant: &str, ids: &[String], hold: chrono::Duration) -> Result<u64, sqlx::Error> {
        if ids.is_empty() {
            return Ok(0);
        }
        // On the database's clock, as the claim is.
        let renewed = sqlx::query(
            "UPDATE generations SET poll_claimed_until = now() + make_interval(secs => $3) \
             WHERE poll_claimed_by = $1 AND id = ANY($2) AND status IN ('pending', 'processing')",
        )
            .bind(claimant)
            .bind(ids.to_vec())
            .bind(hold.num_milliseconds() as f64 / 1000.0)
            .execute(&self.pool)
            .await?;
        Ok(renewed.rows_affected())
    }

    async fn note_poll_skip_warning(&self, id: &str, mark: i64) -> Result<bool, sqlx::Error> {
        // One guarded statement: of pollers racing at one mark, the row lock
        // lets one change it; the others see the mark and change nothing.
        let noted = sqlx::query(
            "UPDATE generations SET poll_skip_warned_mark = $2 \
             WHERE id = $1 AND status IN ('pending', 'processing') \
               AND (poll_skip_warned_mark IS NULL OR poll_skip_warned_mark < $2)",
        )
            .bind(id)
            .bind(mark)
            .execute(&self.pool)
            .await?;
        Ok(noted.rows_affected() > 0)
    }

    async fn clock_now(&self) -> Result<chrono::DateTime<chrono::Utc>, sqlx::Error> {
        sqlx::query_scalar("SELECT now() + make_interval(secs => $1)")
            .bind(self.clock_offset_secs())
            .fetch_one(&self.pool)
            .await
    }

    async fn log_request(
        &self,
        id: &str,
        model: &str,
        provider: &str,
        status: &str,
        media_type: &str,
        cost_usd: f64,
        latency_ms: i64,
        error: Option<&str>,
        metadata: Option<&serde_json::Value>,
        org_id: Option<&str>,
        app_id: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let meta_str = metadata.map(|m| m.to_string());
        sqlx::query(
            r#"
            INSERT INTO request_logs (id, model, provider, status, media_type, cost_usd, latency_ms, error, metadata, org_id, app_id, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, $11), COALESCE($12, $13), NOW())
            "#,
        )
        .bind(id)
        .bind(model)
        .bind(provider)
        .bind(status)
        .bind(media_type)
        .bind(cost_usd)
        .bind(latency_ms)
        .bind(error)
        .bind(meta_str.as_deref())
        .bind(org_id)
        .bind(crate::api::middleware::DEFAULT_ORG_ID)
        .bind(app_id)
        .bind(crate::api::middleware::DEFAULT_APP_ID)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_request_log_status(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        // See the SQLite twin: an async row that settles is re-stamped with its
        // end-to-end latency so `/v1/stats` percentiles keep one meaning; a row
        // that was never `pending` keeps the latency its handler measured.
        sqlx::query(
            r#"
            UPDATE request_logs
            SET status = $1, error = $2,
                latency_ms = CASE WHEN status = 'pending'
                    THEN GREATEST(0, (EXTRACT(EPOCH FROM (NOW() - created_at)) * 1000)::BIGINT)
                    ELSE latency_ms END
            WHERE id = $3
            "#,
        )
            .bind(status)
            .bind(error)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn claim_generation_refund(&self, id: &str, cost_usd: f64) -> Result<bool, sqlx::Error> {
        let claimed = sqlx::query(
            "UPDATE generations SET cost_usd = 0 \
             WHERE id = $1 AND cost_usd = $2 AND cost_usd > 0 AND status IN ('failed', 'cancelled')",
        )
        .bind(id)
        .bind(cost_usd)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;
        if claimed {
            sqlx::query("UPDATE request_logs SET cost_usd = 0 WHERE id = $1").bind(id).execute(&self.pool).await?;
        }
        Ok(claimed)
    }

    async fn get_request_logs(
        &self,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
        let offset = crate::api::pagination::offset(page, per_page);

        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM request_logs")
            .fetch_one(&self.pool)
            .await?;

        let rows = sqlx::query_as::<_, RequestLogRow>(
            r#"
            SELECT id, model, provider, status, media_type, cost_usd, latency_ms, error, metadata, created_at, cache_hit
            FROM request_logs
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(per_page as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await?;

        let logs = rows.into_iter().map(request_log_from_row).collect();
        Ok((logs, total.0 as u64))
    }

    async fn get_request_logs_filtered(
        &self,
        filters: &crate::types::LogFilters,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
        let offset = crate::api::pagination::offset(page, per_page);

        let mut count_qb: QueryBuilder<sqlx::Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM request_logs WHERE 1=1");
        if let Some(ref m) = filters.model {
            count_qb.push(" AND model = ");
            count_qb.push_bind(m.clone());
        }
        if let Some(ref p) = filters.provider {
            count_qb.push(" AND provider = ");
            count_qb.push_bind(p.clone());
        }
        if let Some(ref s) = filters.status {
            count_qb.push(" AND status = ");
            count_qb.push_bind(s.clone());
        }
        if let Some(ref f) = filters.from {
            count_qb.push(" AND created_at >= ");
            count_qb.push_bind(f.clone());
        }
        if let Some(ref t) = filters.to {
            count_qb.push(" AND created_at <= ");
            count_qb.push_bind(t.clone());
        }
        let total: (i64,) = count_qb.build_query_as().fetch_one(&self.pool).await?;

        let mut data_qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(format!(
            "SELECT {REQUEST_LOG_COLS} FROM request_logs WHERE 1=1"
        ));
        if let Some(ref m) = filters.model {
            data_qb.push(" AND model = ");
            data_qb.push_bind(m.clone());
        }
        if let Some(ref p) = filters.provider {
            data_qb.push(" AND provider = ");
            data_qb.push_bind(p.clone());
        }
        if let Some(ref s) = filters.status {
            data_qb.push(" AND status = ");
            data_qb.push_bind(s.clone());
        }
        if let Some(ref f) = filters.from {
            data_qb.push(" AND created_at >= ");
            data_qb.push_bind(f.clone());
        }
        if let Some(ref t) = filters.to {
            data_qb.push(" AND created_at <= ");
            data_qb.push_bind(t.clone());
        }
        data_qb.push(" ORDER BY created_at DESC LIMIT ");
        data_qb.push_bind(per_page as i64);
        data_qb.push(" OFFSET ");
        data_qb.push_bind(offset as i64);

        let rows: Vec<RequestLogRow> = data_qb.build_query_as().fetch_all(&self.pool).await?;
        let logs = rows.into_iter().map(request_log_from_row).collect();
        Ok((logs, total.0 as u64))
    }

    async fn create_api_key(
        &self,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
        token_quota: Option<f64>,
        rpm_limit: Option<u32>,
        scopes: &str,
        webhook_url: Option<&str>,
    ) -> Result<ApiKey, sqlx::Error> {
        // "" is no webhook (review 2026-11-15 round 2, finding 4).
        let webhook_url = webhook_url.filter(|u| !u.is_empty());
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();
        // Back-compat method: untenanted keys land in the default org/app so they
        // remain visible to tenant-scoped listing in single-tenant mode.
        let org_id = crate::api::middleware::DEFAULT_ORG_ID;
        let app_id = crate::api::middleware::DEFAULT_APP_ID;

        sqlx::query(
            r#"
            INSERT INTO api_keys (id, name, key_hash, key_prefix, created_at, is_active, token_quota, tokens_used, rpm_limit, scopes, webhook_url, org_id, app_id)
            VALUES ($1, $2, $3, $4, $5, TRUE, $6, 0, $7, $8, $9, $10, $11)
            "#,
        )
        .bind(id.to_string())
        .bind(name)
        .bind(key_hash)
        .bind(key_prefix)
        .bind(now)
        .bind(token_quota)
        .bind(rpm_limit.map(|v| v as i32))
        .bind(scopes)
        .bind(webhook_url)
        .bind(org_id)
        .bind(app_id)
        .execute(&self.pool)
        .await?;

        Ok(ApiKey {
            id,
            name: name.to_string(),
            key_hash: key_hash.to_string(),
            key_prefix: key_prefix.to_string(),
            created_at: now,
            expires_at: None,
            is_active: true,
            token_quota,
            tokens_used: 0.0,
            rpm_limit,
            scopes: scopes.to_string(),
            webhook_url: webhook_url.map(|s| s.to_string()),
            owner_user_id: None,
            org_id: Some(org_id.to_string()),
            app_id: Some(app_id.to_string()),
            public_id: None,
        })
    }

    async fn get_api_key(&self, id: &Uuid) -> Result<Option<ApiKey>, sqlx::Error> {
        let sql = format!("SELECT {} FROM api_keys WHERE id = $1", API_KEY_COLS);
        let row = sqlx::query_as::<_, ApiKeyRow>(&sql)
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(api_key_from_row))
    }

    async fn update_api_key(
        &self,
        id: &Uuid,
        req: &UpdateApiKeyRequest,
    ) -> Result<Option<ApiKey>, sqlx::Error> {
        // Build numbered SET clause for Postgres
        let mut sets: Vec<String> = Vec::new();
        let mut param_idx: usize = 1;

        if req.name.is_some() { sets.push(format!("name = ${}", param_idx)); param_idx += 1; }
        if req.token_quota.is_some() { sets.push(format!("token_quota = ${}", param_idx)); param_idx += 1; }
        if req.rpm_limit.is_some() { sets.push(format!("rpm_limit = ${}", param_idx)); param_idx += 1; }
        if req.scopes.is_some() { sets.push(format!("scopes = ${}", param_idx)); param_idx += 1; }
        if req.webhook_url.is_some() { sets.push(format!("webhook_url = ${}", param_idx)); param_idx += 1; }
        if req.expires_at.is_some() { sets.push(format!("expires_at = ${}", param_idx)); param_idx += 1; }
        if req.is_active.is_some() { sets.push(format!("is_active = ${}", param_idx)); param_idx += 1; }

        if sets.is_empty() {
            return self.get_api_key(id).await;
        }

        let sql = format!("UPDATE api_keys SET {} WHERE id = ${}", sets.join(", "), param_idx);
        let mut q = sqlx::query(&sql);
        if let Some(ref v) = req.name { q = q.bind(v); }
        if let Some(v) = req.token_quota { q = q.bind(v); }
        if let Some(v) = req.rpm_limit { q = q.bind(v.map(|v| v as i32)); }
        if let Some(ref v) = req.scopes { q = q.bind(v); }
        if let Some(ref v) = req.webhook_url { q = q.bind(v.as_deref().filter(|u| !u.is_empty())); }
        if let Some(v) = req.expires_at { q = q.bind(v); }
        if let Some(v) = req.is_active { q = q.bind(v); }
        q = q.bind(id.to_string());

        let result = q.execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_api_key(id).await
    }

    async fn lookup_api_key_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
        let sql = format!(
            "SELECT {} FROM api_keys WHERE key_hash = $1 AND is_active = TRUE",
            API_KEY_COLS
        );
        let row = sqlx::query_as::<_, ApiKeyRow>(&sql)
            .bind(key_hash)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(api_key_from_row))
    }

    /// One statement: the key, its maker's `is_active`, and the maker's role
    /// in the key's org (`null_org` for a key with none).
    async fn lookup_api_key_with_owner(&self, key_hash: &str, null_org: Option<&str>) -> Result<Option<(ApiKey, crate::db::KeyOwner)>, sqlx::Error> {
        let sql = format!(
            "SELECT {}, u.is_active AS owner_active, m.role AS owner_role FROM api_keys k \
             LEFT JOIN users u ON u.id = k.owner_user_id \
             LEFT JOIN organization_members m ON m.user_id = k.owner_user_id AND m.org_id = COALESCE(k.org_id, $2) \
             WHERE k.key_hash = $1 AND k.is_active = TRUE",
            key_cols_of_k()
        );
        let row = sqlx::query_as::<_, ApiKeyOwnerRow>(&sql)
            .bind(key_hash)
            .bind(null_org)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(api_key_owner_from_row))
    }

    async fn atomic_charge_tokens(
        &self,
        id: &Uuid,
        cost_usd: f64,
    ) -> Result<f64, sqlx::Error> {
        // Single-statement UPDATE ... RETURNING is atomic in Postgres; no
        // need for an explicit BEGIN/COMMIT around one query.
        let row: Option<(f64,)> = sqlx::query_as(
            "UPDATE api_keys SET tokens_used = tokens_used + $1 \
             WHERE id = $2 RETURNING tokens_used"
        )
        .bind(cost_usd)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| r.0).ok_or(sqlx::Error::RowNotFound)
    }

    async fn reserve_tokens(&self, id: &Uuid, amount: f64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE api_keys SET tokens_used = tokens_used + $1 \
             WHERE id = $2 AND (token_quota IS NULL OR tokens_used + $1 <= token_quota)",
        )
        .bind(amount)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn validate_api_key(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
        self.lookup_api_key_by_hash(key_hash).await
    }

    async fn list_api_keys(&self) -> Result<Vec<ApiKey>, sqlx::Error> {
        let sql = format!(
            "SELECT {} FROM api_keys ORDER BY created_at DESC",
            API_KEY_COLS
        );
        let rows = sqlx::query_as::<_, ApiKeyRow>(&sql)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(api_key_from_row).collect())
    }

    async fn list_api_keys_for_owner(&self, owner_user_id: &str) -> Result<Vec<ApiKey>, sqlx::Error> {
        let sql = format!(
            "SELECT {} FROM api_keys WHERE owner_user_id = $1 ORDER BY created_at DESC",
            API_KEY_COLS
        );
        let rows = sqlx::query_as::<_, ApiKeyRow>(&sql)
            .bind(owner_user_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(api_key_from_row).collect())
    }

    async fn set_api_key_owner(&self, id: &Uuid, owner_user_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE api_keys SET owner_user_id = $1 WHERE id = $2")
            .bind(owner_user_id)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn revoke_api_key(&self, id: &Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("UPDATE api_keys SET is_active = FALSE WHERE id = $1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn rotate_api_key(
        &self,
        id: &Uuid,
        public_id: &str,
        key_hash: &str,
        key_prefix: &str,
        owner: Option<&str>,
    ) -> Result<Option<ApiKey>, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE api_keys SET public_id = $1, key_hash = $2, key_prefix = $3, \
             owner_user_id = COALESCE($4, owner_user_id) WHERE id = $5",
        )
        .bind(public_id)
        .bind(key_hash)
        .bind(key_prefix)
        .bind(owner)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_api_key(id).await
    }

    async fn get_stats(&self) -> Result<ProxyStats, sqlx::Error> {
        let totals: (i64, i64, i64, f64, f64) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*) as total,
                COALESCE(SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END), 0) as success,
                COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as failed,
                COALESCE(SUM(cost_usd), 0.0) as total_cost,
                COALESCE(AVG(latency_ms), 0.0)::double precision as avg_latency
            FROM request_logs
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        let model_stats = sqlx::query_as::<_, ModelUsageRow>(
            r#"
            SELECT model, COUNT(*) as requests, COALESCE(SUM(cost_usd), 0.0) as cost_usd, COALESCE(AVG(latency_ms), 0.0)::double precision as avg_latency_ms
            FROM request_logs
            GROUP BY model
            ORDER BY requests DESC
            LIMIT 20
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let provider_stats = sqlx::query_as::<_, ProviderUsageRow>(
            r#"
            SELECT provider,
                   COUNT(*) as requests,
                   SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) as failures,
                   COALESCE(SUM(cost_usd), 0.0) as cost_usd,
                   COALESCE(AVG(latency_ms), 0.0)::double precision as avg_latency_ms
            FROM request_logs
            GROUP BY provider
            ORDER BY requests DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let rpm: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM request_logs WHERE created_at > NOW() - INTERVAL '1 minute'",
        )
        .fetch_one(&self.pool)
        .await?;

        let percentiles = self.latency_percentiles(60).await?;
        Ok(build_proxy_stats(totals, model_stats, provider_stats, rpm.0, percentiles))
    }

    async fn get_stats_for_tenant(
        &self,
        org_id: &str,
        app_id: Option<&str>,
        metadata: &[(String, String)],
    ) -> Result<ProxyStats, sqlx::Error> {
        // Tenant predicate; $1 = org_id, $2 = app_id when present, then a key
        // and a value for each metadata pair the requests must hold (L17).
        let mut tenant_clause = String::from(if app_id.is_some() {
            "org_id = $1 AND app_id = $2"
        } else {
            "org_id = $1"
        });
        let mut n = if app_id.is_some() { 2 } else { 1 };
        for _ in metadata {
            tenant_clause.push_str(&format!(" AND (metadata::jsonb ->> ${}) = ${}", n + 1, n + 2));
            n += 2;
        }
        macro_rules! bind_tenant {
            ($q:expr) => {{
                let mut q = $q.bind(org_id);
                if let Some(a) = app_id {
                    q = q.bind(a);
                }
                for (k, v) in metadata {
                    q = q.bind(k).bind(v);
                }
                q
            }};
        }

        let totals_sql = format!(
            "SELECT COUNT(*) as total, \
             COALESCE(SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END), 0) as success, \
             COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as failed, \
             COALESCE(SUM(cost_usd), 0.0) as total_cost, \
             COALESCE(AVG(latency_ms), 0.0)::double precision as avg_latency \
             FROM request_logs WHERE {tenant_clause}"
        );
        let totals: (i64, i64, i64, f64, f64) =
            bind_tenant!(sqlx::query_as::<_, (i64, i64, i64, f64, f64)>(&totals_sql))
                .fetch_one(&self.pool)
                .await?;

        let model_sql = format!(
            "SELECT model, COUNT(*) as requests, COALESCE(SUM(cost_usd), 0.0) as cost_usd, \
             COALESCE(AVG(latency_ms), 0.0)::double precision as avg_latency_ms \
             FROM request_logs WHERE {tenant_clause} \
             GROUP BY model ORDER BY requests DESC LIMIT 20"
        );
        let model_stats = bind_tenant!(sqlx::query_as::<_, ModelUsageRow>(&model_sql))
            .fetch_all(&self.pool)
            .await?;

        let provider_sql = format!(
            "SELECT provider, COUNT(*) as requests, \
             SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) as failures, \
             COALESCE(SUM(cost_usd), 0.0) as cost_usd, \
             COALESCE(AVG(latency_ms), 0.0)::double precision as avg_latency_ms \
             FROM request_logs WHERE {tenant_clause} \
             GROUP BY provider ORDER BY requests DESC"
        );
        let provider_stats = bind_tenant!(sqlx::query_as::<_, ProviderUsageRow>(&provider_sql))
            .fetch_all(&self.pool)
            .await?;

        let rpm_sql = format!(
            "SELECT COUNT(*) FROM request_logs \
             WHERE {tenant_clause} AND created_at > NOW() - INTERVAL '1 minute'"
        );
        let rpm: (i64,) = bind_tenant!(sqlx::query_as::<_, (i64,)>(&rpm_sql))
            .fetch_one(&self.pool)
            .await?;

        // Reuse the global percentiles window (tenant-specific percentiles not required).
        let percentiles = self.latency_percentiles(60).await?;
        Ok(build_proxy_stats(totals, model_stats, provider_stats, rpm.0, percentiles))
    }

    async fn latency_percentiles(&self, since_minutes: i64) -> Result<LatencyPercentiles, sqlx::Error> {
        // Postgres: use percentile_cont for exact percentiles in a single query.
        let row: Option<(Option<f64>, Option<f64>, Option<f64>, i64)> = sqlx::query_as(
            r#"
            SELECT
                percentile_cont(0.50) WITHIN GROUP (ORDER BY latency_ms) AS p50,
                percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms) AS p95,
                percentile_cont(0.99) WITHIN GROUP (ORDER BY latency_ms) AS p99,
                COUNT(*) AS sample_count
            FROM (
                SELECT latency_ms FROM request_logs
                WHERE created_at > NOW() - ($1 || ' minutes')::INTERVAL
                  AND status = 'completed'
                LIMIT 10000
            ) sub
            "#,
        )
        .bind(since_minutes)
        .fetch_optional(&self.pool)
        .await?;

        let (p50, p95, p99, sample_count) = row.unwrap_or((None, None, None, 0));
        Ok(LatencyPercentiles {
            p50_ms: p50.unwrap_or(0.0),
            p95_ms: p95.unwrap_or(0.0),
            p99_ms: p99.unwrap_or(0.0),
            sample_count: sample_count as u64,
            window_minutes: since_minutes,
        })
    }

    async fn insert_audit_log(&self, entry: &AuditLogEntry) -> Result<(), sqlx::Error> {
        // `org_id` as the server filed it (`api::audit::audit_org`): NULL is no org.
        sqlx::query(
            r#"
            INSERT INTO audit_log
                (id, actor_key_id, actor_label, action, target_type, target_id,
                 before_json, after_json, org_id, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            "#,
        )
        .bind(&entry.id)
        .bind(&entry.actor_key_id)
        .bind(&entry.actor_label)
        .bind(&entry.action)
        .bind(&entry.target_type)
        .bind(&entry.target_id)
        .bind(&entry.before_json)
        .bind(&entry.after_json)
        .bind(&entry.org_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn move_master_key_audit_entries_to_no_org(&self, org_id: &str, actions: &[&str]) -> Result<u64, sqlx::Error> {
        if actions.is_empty() {
            return Ok(0);
        }
        // The master key's entries: no key, and the label every writer gives it.
        let mut qb = QueryBuilder::<sqlx::Postgres>::new(
            "UPDATE audit_log SET org_id = NULL WHERE actor_key_id IS NULL AND actor_label = 'master-key' AND org_id = ",
        );
        qb.push_bind(org_id.to_string());
        qb.push(" AND action IN (");
        let mut listed = qb.separated(", ");
        for action in actions {
            listed.push_bind(action.to_string());
        }
        listed.push_unseparated(")");
        Ok(qb.build().execute(&self.pool).await?.rows_affected())
    }

    async fn list_audit_log(
        &self,
        filter: &AuditLogFilter,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<AuditLogEntry>, i64), sqlx::Error> {
        let offset = crate::api::pagination::offset(page, per_page);

        let mut count_qb: QueryBuilder<sqlx::Postgres> =
            QueryBuilder::new("SELECT COUNT(*) FROM audit_log WHERE 1=1");
        if let Some(ref v) = filter.actor_key_id {
            count_qb.push(" AND actor_key_id = ");
            count_qb.push_bind(v.clone());
        }
        if let Some(ref v) = filter.action {
            count_qb.push(" AND action = ");
            count_qb.push_bind(v.clone());
        }
        if let Some(ref v) = filter.from {
            count_qb.push(" AND created_at >= ");
            count_qb.push_bind(v.clone());
        }
        if let Some(ref v) = filter.to {
            count_qb.push(" AND created_at <= ");
            count_qb.push_bind(v.clone());
        }
        let total: (i64,) = count_qb.build_query_as().fetch_one(&self.pool).await?;

        let mut data_qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(
            "SELECT id, actor_key_id, actor_label, action, target_type, target_id, \
             before_json, after_json, created_at, org_id FROM audit_log WHERE 1=1",
        );
        if let Some(ref v) = filter.actor_key_id {
            data_qb.push(" AND actor_key_id = ");
            data_qb.push_bind(v.clone());
        }
        if let Some(ref v) = filter.action {
            data_qb.push(" AND action = ");
            data_qb.push_bind(v.clone());
        }
        if let Some(ref v) = filter.from {
            data_qb.push(" AND created_at >= ");
            data_qb.push_bind(v.clone());
        }
        if let Some(ref v) = filter.to {
            data_qb.push(" AND created_at <= ");
            data_qb.push_bind(v.clone());
        }
        data_qb.push(" ORDER BY created_at DESC LIMIT ");
        data_qb.push_bind(per_page as i64);
        data_qb.push(" OFFSET ");
        data_qb.push_bind(offset as i64);

        let rows: Vec<AuditLogRow> = data_qb.build_query_as().fetch_all(&self.pool).await?;
        let entries = rows.into_iter().map(audit_log_from_row).collect();
        Ok((entries, total.0))
    }

    async fn insert_webhook_delivery(&self, delivery: &WebhookDelivery) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO webhook_deliveries
                (id, key_id, generation_id, url, attempt_number, status_code, success,
                 response_body, error_message, payload_json, created_at, delivery_id, org_id, app_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW(), $11,
                    COALESCE((SELECT org_id FROM webhook_outbox WHERE id = $11), (SELECT org_id FROM generations WHERE id = $3)),
                    COALESCE((SELECT app_id FROM webhook_outbox WHERE id = $11), (SELECT app_id FROM generations WHERE id = $3)))
            "#,
        )
        .bind(&delivery.id)
        .bind(&delivery.key_id)
        .bind(&delivery.generation_id)
        .bind(&delivery.url)
        .bind(delivery.attempt_number)
        .bind(delivery.status_code)
        .bind(delivery.success as i32)
        .bind(&delivery.response_body)
        .bind(&delivery.error_message)
        .bind(&delivery.payload_json)
        .bind(&delivery.delivery_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_webhook_deliveries(
        &self,
        key_id: &str,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<WebhookDelivery>, i64), sqlx::Error> {
        let offset = crate::api::pagination::offset(page, per_page);

        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM webhook_deliveries WHERE key_id = $1"
        )
        .bind(key_id)
        .fetch_one(&self.pool)
        .await?;

        // An outbox delivery's attempts keep no copy of its payload: each
        // shows the delivery's, its latest attempt's (migration 029).
        let rows = sqlx::query_as::<_, WebhookDeliveryRow>(
            "SELECT d.id, d.key_id, d.generation_id, d.url, d.attempt_number, d.status_code, d.success, \
             d.response_body, d.error_message, \
             CASE WHEN d.payload_json = '' AND o.payload_json IS NOT NULL THEN o.payload_json ELSE d.payload_json END AS payload_json, \
             d.created_at, d.delivery_id \
             FROM webhook_deliveries d LEFT JOIN webhook_outbox o ON o.id = d.delivery_id \
             WHERE d.key_id = $1 \
             ORDER BY d.created_at DESC LIMIT $2 OFFSET $3"
        )
        .bind(key_id)
        .bind(per_page as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await?;

        let deliveries = rows.into_iter().map(webhook_delivery_from_row).collect();
        Ok((deliveries, total.0))
    }

    // ─── Webhook outbox (migration 029) ──────────────────────────────────
    // Every time is the database's (`now()`), as the poll claims' are.

    fn keeps_webhook_outbox(&self) -> bool {
        true
    }

    async fn webhook_outbox_of(&self, generation_id: &str) -> Result<Option<crate::db::OutboxRecord>, sqlx::Error> {
        let sql = format!("SELECT {} FROM webhook_outbox WHERE generation_id = $1", super::sqlite::WEBHOOK_OUTBOX_COLS);
        let row = sqlx::query_as::<_, super::sqlite::WebhookOutboxRow>(&sql).bind(generation_id).fetch_optional(&self.pool).await?;
        Ok(row.map(super::sqlite::webhook_outbox_from_row))
    }

    async fn claim_webhook_delivery_of(
        &self,
        generation_id: &str,
        claimant: &str,
        hold: chrono::Duration,
    ) -> Result<Option<crate::db::OutboxDelivery>, sqlx::Error> {
        let row = sqlx::query_as::<_, super::sqlite::OutboxClaimRow>(
            "UPDATE webhook_outbox \
             SET claimed_by = $2, claimed_until = now() + make_interval(secs => $3) \
             WHERE generation_id = $1 AND status = 'pending' AND attempts = 0 \
               AND (claimed_until IS NULL OR claimed_until < now()) \
             RETURNING id, generation_id, key_id, org_id, attempts, redelivered_at IS NOT NULL AS redelivered",
        )
        .bind(generation_id)
        .bind(claimant)
        .bind(hold.num_milliseconds() as f64 / 1000.0)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Into::into))
    }

    async fn claim_due_webhook_deliveries(
        &self,
        claimant: &str,
        limit: u32,
        per_org: u32,
        hold: chrono::Duration,
    ) -> Result<Vec<crate::db::OutboxDelivery>, sqlx::Error> {
        // Ranked within each org (a window function, which can't share a
        // SELECT with FOR UPDATE), the deliveries `claimant` holds already
        // counted; then the chosen rows are locked with SKIP LOCKED, so a
        // row another claimer is taking right now goes to nobody here, and
        // re-checked, so one another claimer took since this statement began
        // is left out. Concurrent claims never wait on or share a delivery.
        let rows = sqlx::query_as::<_, super::sqlite::OutboxClaimRow>(
            "WITH held AS ( \
                 SELECT COALESCE(org_id, '') AS org, COUNT(*) AS n FROM webhook_outbox \
                 WHERE claimed_by = $1 AND status = 'pending' AND claimed_until >= now() \
                 GROUP BY 1 \
             ), ranked AS ( \
                 SELECT o.id AS ranked_id, o.next_attempt_at AS due_at, o.created_at AS made_at, \
                        ROW_NUMBER() OVER (PARTITION BY COALESCE(o.org_id, '') ORDER BY o.next_attempt_at, o.created_at, o.id) \
                          + COALESCE(h.n, 0) AS place \
                 FROM webhook_outbox o LEFT JOIN held h ON h.org = COALESCE(o.org_id, '') \
                 WHERE o.status = 'pending' AND o.next_attempt_at <= now() \
                   AND (o.claimed_until IS NULL OR o.claimed_until < now()) \
             ), chosen AS ( \
                 SELECT ranked_id FROM ranked WHERE place <= $4 ORDER BY due_at, made_at, ranked_id LIMIT $3 \
             ), locked AS MATERIALIZED ( \
                 SELECT o.id AS locked_id FROM webhook_outbox o \
                 WHERE o.id IN (SELECT ranked_id FROM chosen) \
                   AND o.status = 'pending' AND (o.claimed_until IS NULL OR o.claimed_until < now()) \
                 FOR UPDATE SKIP LOCKED \
             ) \
             UPDATE webhook_outbox w \
             SET claimed_by = $1, claimed_until = now() + make_interval(secs => $2) \
             FROM locked \
             WHERE w.id = locked.locked_id \
             RETURNING w.id, w.generation_id, w.key_id, w.org_id, w.attempts, w.redelivered_at IS NOT NULL AS redelivered",
        )
        .bind(claimant)
        .bind(hold.num_milliseconds() as f64 / 1000.0)
        .bind(limit as i64)
        .bind(per_org as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn webhook_delivery_due(&self) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM webhook_outbox \
             WHERE status = 'pending' AND next_attempt_at <= now() AND (claimed_until IS NULL OR claimed_until < now()))",
        )
        .fetch_one(&self.pool)
        .await
    }

    async fn webhook_answered_for(&self, generation_id: &str) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM webhook_deliveries WHERE generation_id = $1 AND success = 1)")
            .bind(generation_id)
            .fetch_one(&self.pool)
            .await
    }

    async fn renew_webhook_claims(&self, claimant: &str, ids: &[String], hold: chrono::Duration) -> Result<u64, sqlx::Error> {
        if ids.is_empty() {
            return Ok(0);
        }
        let renewed = sqlx::query(
            "UPDATE webhook_outbox SET claimed_until = now() + make_interval(secs => $3) \
             WHERE claimed_by = $1 AND id = ANY($2) AND status = 'pending'",
        )
        .bind(claimant)
        .bind(ids.to_vec())
        .bind(hold.num_milliseconds() as f64 / 1000.0)
        .execute(&self.pool)
        .await?;
        Ok(renewed.rows_affected())
    }

    async fn release_webhook_claims(&self, claimant: &str, keep: &[String]) -> Result<u64, sqlx::Error> {
        let freed = sqlx::query(
            "UPDATE webhook_outbox SET claimed_by = NULL, claimed_until = NULL \
             WHERE claimed_by = $1 AND status = 'pending' AND NOT (id = ANY($2))",
        )
        .bind(claimant)
        .bind(keep.to_vec())
        .execute(&self.pool)
        .await?;
        Ok(freed.rows_affected())
    }

    async fn finish_webhook_attempt(&self, id: &str, claimant: &str, attempt: &crate::db::OutboxAttempt<'_>) -> Result<bool, sqlx::Error> {
        let after = attempt.outcome.retry_after().map(|d| d.num_milliseconds() as f64 / 1000.0);
        let done = sqlx::query(
            "UPDATE webhook_outbox \
             SET status = $3, attempts = $4, \
                 next_attempt_at = CASE WHEN $5::float8 IS NULL THEN next_attempt_at ELSE now() + make_interval(secs => $5::float8) END, \
                 claimed_by = NULL, claimed_until = NULL, \
                 last_status_code = $6, last_error = $7, payload_json = COALESCE($8, payload_json), \
                 finished_at = CASE WHEN $3 = 'pending' THEN NULL ELSE now() END \
             WHERE id = $1 AND claimed_by = $2 AND status = 'pending'",
        )
        .bind(id)
        .bind(claimant)
        .bind(attempt.outcome.status())
        .bind(attempt.attempts)
        .bind(after)
        .bind(attempt.status_code)
        .bind(attempt.error)
        .bind(attempt.payload_json)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    async fn redeliver_webhook_delivery(&self, key_id: &str, delivery_id: &str, cooldown: chrono::Duration) -> Result<crate::db::Redelivery, sqlx::Error> {
        let sql = format!(
            "UPDATE webhook_outbox \
             SET status = 'pending', attempts = 0, next_attempt_at = now(), \
                 claimed_by = NULL, claimed_until = NULL, last_status_code = NULL, last_error = NULL, finished_at = NULL, \
                 redelivered_at = now() \
             WHERE id = $1 AND key_id = $2 AND status IN ('delivered', 'failed') \
               AND (redelivered_at IS NULL OR redelivered_at <= now() - make_interval(secs => $3)) \
             RETURNING {}",
            super::sqlite::WEBHOOK_OUTBOX_COLS
        );
        let redelivered = sqlx::query_as::<_, super::sqlite::WebhookOutboxRow>(&sql)
            .bind(delivery_id)
            .bind(key_id)
            .bind(cooldown.num_milliseconds() as f64 / 1000.0)
            .fetch_optional(&self.pool)
            .await?;
        if let Some(row) = redelivered {
            return Ok(crate::db::Redelivery::Redelivered(super::sqlite::webhook_outbox_from_row(row)));
        }
        let sql = format!("SELECT {} FROM webhook_outbox WHERE id = $1 AND key_id = $2", super::sqlite::WEBHOOK_OUTBOX_COLS);
        let row = sqlx::query_as::<_, super::sqlite::WebhookOutboxRow>(&sql).bind(delivery_id).bind(key_id).fetch_optional(&self.pool).await?;
        Ok(crate::db::trait_def::redelivery_refused(row.map(super::sqlite::webhook_outbox_from_row), cooldown))
    }

    async fn purge_webhook_log(&self, days: u32, limit: u32) -> Result<crate::db::WebhookLogPurged, sqlx::Error> {
        let days = i32::try_from(days).unwrap_or(i32::MAX);
        let deliveries = sqlx::query(
            "DELETE FROM webhook_outbox WHERE id IN ( \
                 SELECT id FROM webhook_outbox \
                 WHERE finished_at < now() - make_interval(days => $1) AND status IN ('delivered', 'failed') LIMIT $2)",
        )
        .bind(days)
        .bind(i64::from(limit))
        .execute(&self.pool)
        .await?
        .rows_affected();
        let attempts = sqlx::query(
            "DELETE FROM webhook_deliveries WHERE id IN ( \
                 SELECT id FROM webhook_deliveries WHERE created_at < now() - make_interval(days => $1) LIMIT $2)",
        )
        .bind(days)
        .bind(i64::from(limit))
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(crate::db::WebhookLogPurged { deliveries, attempts })
    }

    async fn purge_request_artifacts(&self, before: chrono::DateTime<chrono::Utc>, limit: u32) -> Result<u64, sqlx::Error> {
        let done = sqlx::query(
            "DELETE FROM request_artifacts WHERE request_id IN ( \
                 SELECT request_id FROM request_artifacts WHERE created_at < $1 ORDER BY created_at LIMIT $2)",
        )
        .bind(before)
        .bind(i64::from(limit))
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected())
    }

    async fn purge_request_logs(&self, before: chrono::DateTime<chrono::Utc>, limit: u32) -> Result<u64, sqlx::Error> {
        // Their tags go by the foreign key (migration 037, ON DELETE CASCADE).
        let done = sqlx::query(
            "DELETE FROM request_logs WHERE id IN ( \
                 SELECT id FROM request_logs WHERE created_at < $1 ORDER BY created_at LIMIT $2)",
        )
        .bind(before)
        .bind(i64::from(limit))
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected())
    }

    // ─── A deleted org's or app's files (migration 031) ──────────────────

    async fn claim_tenant_file_deletions(
        &self,
        of: Option<(&str, &str)>,
        limit: u32,
        hold: chrono::Duration,
    ) -> Result<Vec<crate::db::TenantFileDeletion>, sqlx::Error> {
        // The due rows are locked with SKIP LOCKED, so a row another claim is
        // taking right now goes to nobody here, and the lock re-checks that
        // each is still due, so one another claim took since this statement
        // began is left out. Concurrent claims never wait on or share a row.
        let (scope, tenant) = of.unzip();
        let rows = sqlx::query_as::<_, super::sqlite::TenantFileDeletionRow>(
            "WITH due AS MATERIALIZED ( \
                 SELECT id FROM tenant_file_deletions \
                 WHERE next_attempt_at <= now() \
                   AND ($1::text IS NULL OR (scope = $1 AND tenant_id = $2)) \
                 ORDER BY next_attempt_at, id \
                 LIMIT $4 \
                 FOR UPDATE SKIP LOCKED \
             ) \
             UPDATE tenant_file_deletions t \
             SET attempts = t.attempts + 1, next_attempt_at = now() + make_interval(secs => $3) \
             FROM due \
             WHERE t.id = due.id \
             RETURNING t.id, t.scope, t.tenant_id, t.generation_id, t.url, t.attempts",
        )
        .bind(scope)
        .bind(tenant)
        .bind(hold.num_milliseconds() as f64 / 1000.0)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn finish_tenant_file_deletion(&self, id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM tenant_file_deletions WHERE id = $1").bind(id).execute(&self.pool).await?;
        Ok(done.rows_affected() > 0)
    }

    async fn retry_tenant_file_deletion(&self, id: i64, attempts: i32, after: chrono::Duration, error: &str) -> Result<bool, sqlx::Error> {
        let done = sqlx::query(
            "UPDATE tenant_file_deletions SET next_attempt_at = now() + make_interval(secs => $3), last_error = $4 \
             WHERE id = $1 AND attempts = $2",
        )
        .bind(id)
        .bind(attempts)
        .bind(after.num_milliseconds() as f64 / 1000.0)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    async fn record_tenant_file_deletions(&self, scope: &str, tenant_id: &str, generation_id: &str, urls: &[String]) -> Result<(), sqlx::Error> {
        let files: Vec<(String, String)> = urls.iter().map(|url| (generation_id.to_string(), url.clone())).collect();
        let mut conn = self.pool.acquire().await?;
        insert_tenant_files(&mut conn, scope, tenant_id, &files).await
    }

    async fn forget_request_rows(&self, generation_id: Option<&str>, request_id: &str) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        for stmt in [
            "DELETE FROM request_artifacts WHERE request_id = $1",
            "DELETE FROM request_log_tags WHERE request_id = $1",
            "DELETE FROM request_logs WHERE id = $1",
        ] {
            sqlx::query(stmt).bind(request_id).execute(&mut *tx).await?;
        }
        if let Some(generation_id) = generation_id {
            sqlx::query("DELETE FROM generations WHERE id = $1").bind(generation_id).execute(&mut *tx).await?;
        }
        tx.commit().await
    }

    /// request_logs, request_artifacts and generations reference their org
    /// and app (migration 008), and request_log_tags its log row (037).
    fn request_rows_reference_their_tenant(&self) -> bool {
        true
    }

    async fn mark_cache_hit(&self, request_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE request_logs SET cache_hit = TRUE WHERE id = $1").bind(request_id).execute(&self.pool).await?;
        Ok(())
    }

    async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    async fn insert_request_artifact(&self, a: &RequestArtifact) -> Result<(), sqlx::Error> {
        let params_str = a.params_json.as_ref().map(|v| v.to_string());
        let refs_str = a.refs_meta_json.as_ref().map(|v| v.to_string());
        sqlx::query(
            r#"
            INSERT INTO request_artifacts
                (request_id, media_type, prompt, negative_prompt, params_json, refs_meta_json,
                 output_kind, output_value, output_mime, output_truncated, error_message, created_at,
                 org_id, app_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW(),
                    COALESCE($12, $13), COALESCE($14, $15))
            ON CONFLICT (request_id) DO UPDATE SET
                output_kind = EXCLUDED.output_kind,
                output_value = EXCLUDED.output_value,
                output_mime = EXCLUDED.output_mime,
                output_truncated = EXCLUDED.output_truncated,
                error_message = EXCLUDED.error_message
            "#,
        )
        .bind(&a.request_id)
        .bind(&a.media_type)
        .bind(&a.prompt)
        .bind(&a.negative_prompt)
        .bind(params_str.as_deref())
        .bind(refs_str.as_deref())
        .bind(&a.output_kind)
        .bind(&a.output_value)
        .bind(&a.output_mime)
        .bind(a.output_truncated)
        .bind(&a.error_message)
        .bind(a.org_id.as_deref())
        .bind(crate::api::middleware::DEFAULT_ORG_ID)
        .bind(a.app_id.as_deref())
        .bind(crate::api::middleware::DEFAULT_APP_ID)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_request_artifact(&self, request_id: &str) -> Result<Option<RequestArtifact>, sqlx::Error> {
        use super::sqlite::{RequestArtifactRow, artifact_from_row};
        let row = sqlx::query_as::<_, RequestArtifactRow>(
            "SELECT request_id, media_type, prompt, negative_prompt, params_json, refs_meta_json, \
             output_kind, output_value, output_mime, output_truncated, \
             error_message, created_at, org_id, app_id \
             FROM request_artifacts WHERE request_id = $1"
        )
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(artifact_from_row))
    }

    // ─── Users ──────────────────────────────────────────────────────────

    async fn create_user(&self, user: &User) -> Result<(), sqlx::Error> {
        insert_user(&self.pool, user).await
    }

    async fn get_user_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
             created_at, updated_at, last_login_at, is_active, sign_in_epoch \
             FROM users WHERE email = $1"
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(user_from_row))
    }

    async fn get_user_by_id(&self, id: &str) -> Result<Option<User>, sqlx::Error> {
        fetch_user(&self.pool, id).await
    }

    async fn get_user_by_oauth(&self, provider: &str, oauth_id: &str) -> Result<Option<User>, sqlx::Error> {
        let col = match provider {
            "github" => "oauth_github_id",
            "google" => "oauth_google_id",
            _ => return Ok(None),
        };
        let sql = format!(
            "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
             created_at, updated_at, last_login_at, is_active, sign_in_epoch \
             FROM users WHERE {} = $1",
            col
        );
        let row = sqlx::query_as::<_, UserRow>(&sql)
            .bind(oauth_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(user_from_row))
    }

    async fn update_user(
        &self,
        id: &str,
        role: Option<Role>,
        is_active: Option<bool>,
        password_hash: Option<&str>,
    ) -> Result<Option<User>, sqlx::Error> {
        Ok(self.update_user_with_revoked(id, role, is_active, password_hash, None).await?.map(|(user, _)| user))
    }

    async fn update_user_with_revoked(
        &self,
        id: &str,
        role: Option<Role>,
        is_active: Option<bool>,
        password_hash: Option<&str>,
        member_org: Option<&str>,
    ) -> Result<Option<(User, Vec<Invitation>)>, sqlx::Error> {
        let mut sets: Vec<String> = Vec::new();
        let mut param_idx: usize = 1;

        if role.is_some() { sets.push(format!("role = ${}", param_idx)); param_idx += 1; }
        if is_active.is_some() { sets.push(format!("is_active = ${}", param_idx)); param_idx += 1; }
        // Reactivating moves the sign-in epoch on (review round 5, finding
        // 2): a session stamped while the account was deactivated doesn't
        // come back with it. (The right side reads the row before the write.)
        if is_active == Some(true) { sets.push("sign_in_epoch = CASE WHEN is_active THEN sign_in_epoch ELSE sign_in_epoch + 1 END".to_string()); }
        if password_hash.is_some() { sets.push(format!("password_hash = ${}", param_idx)); param_idx += 1; }
        sets.push("updated_at = NOW()".to_string());

        let sql = format!("UPDATE users SET {} WHERE id = ${}", sets.join(", "), param_idx);
        let mut q = sqlx::query(&sql);
        if let Some(r) = role { q = q.bind(r.as_str()); }
        if let Some(a) = is_active { q = q.bind(a); }
        if let Some(ph) = password_hash { q = q.bind(ph); }
        q = q.bind(id);

        let mut tx = self.pool.begin().await?;
        // The account's row first (review of round 8): every change that
        // touches an account's memberships or links locks the account's row
        // before them (a confirm, a transfer of ownership, a link's issue for
        // its issuer), so none of them deadlocks with this. Gone: nothing to
        // change.
        if sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR NO KEY UPDATE").bind(id).fetch_optional(&mut *tx).await?.is_none() {
            return Ok(None);
        }
        // The role in `member_org` too (review round 7, finding 3), where the
        // single-tenant permissions come from, in this transaction: a failure
        // leaves both roles as they were. Never an Owner's: an account made
        // the Owner since the caller checked stays the Owner, and this fails
        // with nothing written, so a retry is refused as the Owner's.
        if let (Some(r), Some(org)) = (role, member_org) {
            let moved = sqlx::query("UPDATE organization_members SET role = $1 WHERE org_id = $2 AND user_id = $3 AND role <> 'owner'")
                .bind(r.as_str())
                .bind(org)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            if moved.rows_affected() == 0 {
                return Err(sqlx::Error::RowNotFound);
            }
        }
        // Migrations 042/043's triggers delete the account's unused reset
        // links, and the ones it issued, when this changes its role or writes
        // is_active false.
        let result = q.execute(&mut *tx).await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        // Deactivating ends what the account was signed in with, in the
        // transaction that deactivates it (review round 3b, S1), and revokes
        // the pending invitations it sent, in every org (SEC-INVITES), in the
        // same transaction (review round 6, finding 2). The row's lock, taken
        // by the UPDATE above, makes an invitation sent from the account's
        // session meanwhile wait for this commit (and then be refused), or
        // land before the revoke reads.
        let mut revoked = Vec::new();
        if is_active == Some(false) {
            end_sign_ins(&mut tx, id, None).await?;
            revoked = revoke_pending_invitations_in(&mut *tx, None, None, Some(id)).await?;
        }
        // Read back before the commit (review round 7, finding 3): after it,
        // nothing can fail and leave the revokes without their audit entries.
        let user = fetch_user(&mut *tx, id).await?.ok_or(sqlx::Error::RowNotFound)?;
        tx.commit().await?;
        Ok(Some((user, revoked)))
    }

    async fn touch_last_login(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET last_login_at = NOW(), updated_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn count_users(&self) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    async fn create_first_owner(&self, user: &User, org_id: &str) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Under READ COMMITTED two first signups would each count no
        // account, so the claim on the one `first_owner` row (migration 034)
        // decides: the second's insert waits for the first's transaction,
        // then conflicts (under a stricter isolation, it fails instead).
        // Returning before the commit rolls everything back.
        let claimed = sqlx::query("INSERT INTO first_owner (id, user_id) VALUES (1, $1) ON CONFLICT (id) DO NOTHING")
            .bind(&user.id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if claimed == 0 {
            return Ok(false);
        }
        // Accounts made before the claim existed close the signup too.
        let (accounts,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users").fetch_one(&mut *tx).await?;
        if accounts > 0 {
            return Ok(false);
        }
        insert_user(&mut *tx, user).await?;
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query(
            "INSERT INTO organization_members (org_id, user_id, role, created_at) \
             VALUES ($1, $2, $3, NOW())",
        )
        .bind(org_id)
        .bind(&user.id)
        .bind(Role::Owner.as_str())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn list_users(&self) -> Result<Vec<User>, sqlx::Error> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
             created_at, updated_at, last_login_at, is_active, sign_in_epoch \
             FROM users ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(user_from_row).collect())
    }

    async fn link_oauth(&self, user_id: &str, provider: &str, oauth_id: &str) -> Result<(), sqlx::Error> {
        let col = match provider {
            "github" => "oauth_github_id",
            "google" => "oauth_google_id",
            _ => return Ok(()),
        };
        let sql = format!("UPDATE users SET {} = $1, updated_at = NOW() WHERE id = $2", col);
        sqlx::query(&sql)
            .bind(oauth_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn transfer_owner(&self, new_owner_id: &str) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE users SET role = 'admin', updated_at = NOW() WHERE role = 'owner'")
            .execute(&mut *tx)
            .await?;
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE users SET role = 'owner', updated_at = NOW() WHERE id = $1")
            .bind(new_owner_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    // ─── Sessions ───────────────────────────────────────────────────────

    async fn create_session(&self, s: &Session) -> Result<(), sqlx::Error> {
        // Kept by its token's hash (migration 044), stamped with the epoch
        // the account has now (migration 043).
        sqlx::query(
            "INSERT INTO sessions (id, user_id, created_at, expires_at, ip, user_agent, csrf_token, sign_in_epoch) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE((SELECT sign_in_epoch FROM users WHERE id = $2), 0))"
        )
        .bind(crate::auth::tokens::token_hash(&s.id))
        .bind(&s.user_id)
        .bind(s.created_at)
        .bind(s.expires_at)
        .bind(&s.ip)
        .bind(&s.user_agent)
        .bind(&s.csrf_token)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn create_session_at_epoch(&self, s: &Session, epoch: i64) -> Result<(), sqlx::Error> {
        // Kept by its token's hash (migration 044).
        sqlx::query(
            "INSERT INTO sessions (id, user_id, created_at, expires_at, ip, user_agent, csrf_token, sign_in_epoch) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
        .bind(crate::auth::tokens::token_hash(&s.id))
        .bind(&s.user_id)
        .bind(s.created_at)
        .bind(s.expires_at)
        .bind(&s.ip)
        .bind(&s.user_agent)
        .bind(&s.csrf_token)
        .bind(epoch)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn sign_in_epoch(&self, user_id: &str) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar("SELECT sign_in_epoch FROM users WHERE id = $1").bind(user_id).fetch_optional(&self.pool).await
    }

    async fn session_epoch(&self, id: &str) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar("SELECT sign_in_epoch FROM sessions WHERE id = $1").bind(id).fetch_optional(&self.pool).await
    }

    async fn change_own_password(&self, user_id: &str, password_hash: &str, verified_hash: &str, keep_session: &str) -> Result<crate::db::PasswordChange, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Only while nothing moved since the current password was checked
        // (review round 5, finding 1): the account still holds the hash it was
        // checked against, is active, and is at the sending session's epoch,
        // all in this one statement. A reset, a deactivation or another
        // change that committed meanwhile makes it write nothing.
        let changed = sqlx::query(
            "UPDATE users SET password_hash = $1, updated_at = NOW() \
             WHERE id = $2 AND password_hash = $3 AND is_active \
             AND sign_in_epoch = (SELECT sign_in_epoch FROM sessions WHERE id = $4 AND user_id = $2)",
        )
        .bind(password_hash)
        .bind(user_id)
        .bind(verified_hash)
        .bind(keep_session)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed == 0 {
            return Ok(crate::db::PasswordChange::SignInEnded);
        }
        end_sign_ins(&mut tx, user_id, Some(keep_session)).await?;
        tx.commit().await?;
        Ok(crate::db::PasswordChange::Done)
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>, sqlx::Error> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, user_id, created_at, expires_at, ip, user_agent, csrf_token \
             FROM sessions WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(session_from_row))
    }

    async fn get_session_with_user(&self, cookie: &str) -> Result<Option<(Session, Option<User>)>, sqlx::Error> {
        // Found by the cookie's hash, never the cookie itself (migration
        // 044). A session stamped with an epoch the account has moved past (a
        // reset, deactivation or password change committed after its sign-in
        // began) is no session (review round 4, finding 2).
        let sql = format!(
            "SELECT {SESSION_USER_COLS} FROM sessions s LEFT JOIN users u ON u.id = s.user_id \
             WHERE s.id = $1 AND (u.id IS NULL OR u.sign_in_epoch = s.sign_in_epoch)"
        );
        let row = sqlx::query_as::<_, SessionUserRow>(&sql).bind(crate::auth::tokens::token_hash(cookie)).fetch_optional(&self.pool).await?;
        Ok(row.map(session_user_from_row))
    }

    async fn delete_session(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_user_sessions(&self, user_id: &str, except_id: Option<&str>) -> Result<u64, sqlx::Error> {
        let result = if let Some(exc) = except_id {
            sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND id != $2")
                .bind(user_id)
                .bind(exc)
                .execute(&self.pool)
                .await?
        } else {
            sqlx::query("DELETE FROM sessions WHERE user_id = $1")
                .bind(user_id)
                .execute(&self.pool)
                .await?
        };
        Ok(result.rows_affected())
    }

    async fn list_user_sessions(&self, user_id: &str) -> Result<Vec<Session>, sqlx::Error> {
        let rows = sqlx::query_as::<_, SessionRow>(
            "SELECT s.id, s.user_id, s.created_at, s.expires_at, s.ip, s.user_agent, s.csrf_token \
             FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.user_id = $1 AND s.sign_in_epoch = u.sign_in_epoch ORDER BY s.created_at DESC"
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(session_from_row).collect())
    }

    async fn bump_session_expiry(
        &self,
        id: &str,
        new_expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE sessions SET expires_at = $1 WHERE id = $2")
            .bind(new_expires_at)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ─── Invitations ────────────────────────────────────────────────────

    async fn create_invitation(&self, inv: &Invitation) -> Result<(), sqlx::Error> {
        self.create_invitation_from(inv, crate::db::SignIn::MasterKey).await.map(|_| ())
    }

    async fn create_invitation_from(&self, inv: &Invitation, from: crate::db::SignIn<'_>) -> Result<bool, sqlx::Error> {
        // Only the token's hash is kept (migration 041), in both columns. The
        // sender's sign-in is checked in this statement (review round 6,
        // finding 2).
        let hash = crate::auth::tokens::token_hash(&inv.token);
        let (guard, sign_in) = live_sign_in_guard(from, 10);
        let sql = format!(
            "INSERT INTO invitations (id, email, role, token, token_hash, invited_by, org_id, expires_at, used_at, created_at) \
             SELECT $1, $2, $3, $4, $4, $5, $6, $7, $8, $9{guard}"
        );
        let mut q = sqlx::query(&sql)
            .bind(&inv.id)
            .bind(&inv.email)
            .bind(inv.role.as_str())
            .bind(&hash)
            .bind(&inv.invited_by)
            .bind(&inv.org_id)
            .bind(inv.expires_at)
            .bind(inv.used_at)
            .bind(inv.created_at);
        if let Some(id) = sign_in {
            q = q.bind(id);
        }
        Ok(q.execute(&self.pool).await?.rows_affected() == 1)
    }

    async fn get_invitation(&self, token: &str) -> Result<Option<Invitation>, sqlx::Error> {
        let row = sqlx::query_as::<_, InvitationRow>(
            "SELECT id, email, role, token, invited_by, org_id, expires_at, used_at, created_at \
             FROM invitations WHERE token_hash = $1 OR (token_hash IS NULL AND token = $2)"
        )
        .bind(crate::auth::tokens::token_hash(token))
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(invitation_from_row))
    }

    async fn mark_invitation_used(&self, token: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE invitations SET used_at = NOW() \
             WHERE (token_hash = $1 OR (token_hash IS NULL AND token = $2)) AND used_at IS NULL",
        )
        .bind(crate::auth::tokens::token_hash(token))
        .bind(token)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    async fn invitation_accepted(&self, org_id: &str, id: &str) -> Result<bool, sqlx::Error> {
        let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM invitations WHERE id = $1 AND org_id = $2 AND used_at IS NOT NULL")
            .bind(id)
            .bind(org_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    async fn accept_invitation(&self, token: &str, new_user: Option<&User>, user_id: &str, org_id: &str, role: Role) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Consumed first: a concurrent revoke or acceptance waits on the row
        // and then finds it consumed, or ran before and left nothing.
        let consumed = sqlx::query(
            "UPDATE invitations SET used_at = NOW() \
             WHERE (token_hash = $1 OR (token_hash IS NULL AND token = $2)) AND used_at IS NULL",
        )
        .bind(crate::auth::tokens::token_hash(token))
        .bind(token)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if consumed == 0 {
            return Ok(false);
        }
        if let Some(user) = new_user {
            insert_user(&mut *tx, user).await?;
        }
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query(
            "INSERT INTO organization_members (org_id, user_id, role, created_at) VALUES ($1, $2, $3, NOW()) \
             ON CONFLICT (org_id, user_id) DO NOTHING",
        )
        .bind(org_id)
        .bind(user_id)
        .bind(role.as_str())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn delete_invitation(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM invitations WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn revoke_pending_invitations(&self, org_id: Option<&str>, email: Option<&str>, invited_by: Option<&str>) -> Result<Vec<Invitation>, sqlx::Error> {
        revoke_pending_invitations_in(&self.pool, org_id, email, invited_by).await
    }

    async fn list_invitations(&self) -> Result<Vec<Invitation>, sqlx::Error> {
        let rows = sqlx::query_as::<_, InvitationRow>(
            "SELECT id, email, role, token, invited_by, org_id, expires_at, used_at, created_at \
             FROM invitations ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(invitation_from_row).collect())
    }

    async fn list_unused_invitations(&self, org_id: &str) -> Result<Vec<Invitation>, sqlx::Error> {
        let rows = sqlx::query_as::<_, InvitationRow>(
            "SELECT id, email, role, token, invited_by, org_id, expires_at, used_at, created_at \
             FROM invitations WHERE org_id = $1 AND used_at IS NULL ORDER BY created_at DESC"
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(invitation_from_row).collect())
    }

    async fn revoke_invitation(&self, org_id: &str, id: &str) -> Result<Option<Invitation>, sqlx::Error> {
        let row = sqlx::query_as::<_, InvitationRow>(
            "DELETE FROM invitations WHERE id = $1 AND org_id = $2 AND used_at IS NULL \
             RETURNING id, email, role, token, invited_by, org_id, expires_at, used_at, created_at"
        )
        .bind(id)
        .bind(org_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(invitation_from_row))
    }

    // ─── Password Resets ────────────────────────────────────────────────

    async fn create_password_reset(&self, r: &PasswordReset) -> Result<(), sqlx::Error> {
        self.create_password_reset_from(r, crate::db::SignIn::MasterKey).await.map(|_| ())
    }

    async fn create_password_reset_from(&self, r: &PasswordReset, from: crate::db::SignIn<'_>) -> Result<bool, sqlx::Error> {
        // Only the token's hash is kept (migration 041). In one transaction,
        // each step taking its locks in the order every account change takes
        // them (the account's row, then its memberships and links: review of
        // round 8), so none of them deadlocks with this:
        //  1. The issuer's sign-in is checked live, under a share lock on its
        //     row (review round 6, finding 2): its reset, deactivation,
        //     password change, role change or transfer of ownership locks that
        //     row first, then deletes the links it issued.
        //  2. The account's older unused links are voided, which locks them
        //     before anything locks the account's row (review round 7,
        //     finding 4): a confirm locks its link, then that row.
        //  3. The new link goes in. Its foreign key takes a key-share lock on
        //     the account's row, which a confirm holding the row makes wait
        //     here, not in a cycle.
        // The issuer is never the account (`issue_password_reset` refuses
        // one's own), so the share lock in 1 is never on the row a confirm
        // locks in 2. A refused link changes nothing.
        let hash = crate::auth::tokens::token_hash(&r.token);
        let mut tx = self.pool.begin().await?;
        if let Some((query, id)) = live_sign_in_query(from, 1) {
            if sqlx::query(&query).bind(id).fetch_optional(&mut *tx).await?.is_none() {
                return Ok(false);
            }
        }
        sqlx::query("DELETE FROM password_resets WHERE user_id = $1 AND used_at IS NULL")
            .bind(&r.user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO password_resets (token, user_id, expires_at, used_at, created_at, issued_by) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&hash)
        .bind(&r.user_id)
        .bind(r.expires_at)
        .bind(r.used_at)
        .bind(r.created_at)
        .bind(&r.issued_by)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn get_password_reset(&self, token: &str) -> Result<Option<PasswordReset>, sqlx::Error> {
        let row = sqlx::query_as::<_, PasswordResetRow>(
            "SELECT token, user_id, expires_at, used_at, created_at, issued_by \
             FROM password_resets WHERE token = $1"
        )
        .bind(crate::auth::tokens::token_hash(token))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(password_reset_from_row))
    }

    async fn confirm_password_reset(&self, token: &str, password_hash: &str, mode: crate::config::Mode) -> Result<crate::db::ResetConfirm, sqlx::Error> {
        use crate::db::ResetConfirm;
        let mut tx = self.pool.begin().await?;
        let hash = crate::auth::tokens::token_hash(token);
        // The account's row first, locked until the commit (review of round
        // 8): every change that touches an account's links or memberships
        // locks the account's row before them, so a deactivation or a role
        // write waits for this confirm, or this one for it, and none of them
        // deadlocks with it. (A new link's foreign key takes a key-share lock
        // on the row, which this one lets through.)
        let Some(user_id): Option<String> = sqlx::query_scalar("SELECT user_id FROM password_resets WHERE token = $1 AND used_at IS NULL")
            .bind(&hash)
            .fetch_optional(&mut *tx)
            .await?
        else {
            return Ok(ResetConfirm::Unusable);
        };
        let Some(active): Option<bool> = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1 FOR NO KEY UPDATE").bind(&user_id).fetch_optional(&mut *tx).await? else {
            return Ok(ResetConfirm::Unusable);
        };
        // Then the single-use CAS; returning before the commit undoes it. A
        // newer link issued meanwhile voided this one.
        let claimed: Option<(String, Option<String>)> = sqlx::query_as(
            "UPDATE password_resets SET used_at = NOW() WHERE token = $1 AND used_at IS NULL AND user_id = $2 RETURNING user_id, issued_by",
        )
        .bind(&hash)
        .bind(&user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((user_id, issued_by)) = claimed else { return Ok(ResetConfirm::Unusable) };
        // The issuer may still issue this link, now (review round 4,
        // findings 4 and 5); the master key's links carry no issuer.
        if let Some(issuer) = issued_by {
            let issuer_active: Option<bool> = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1").bind(&issuer).fetch_optional(&mut *tx).await?;
            let roles_of = |rows: Vec<(String, String)>| rows.into_iter().filter_map(|(org, role)| Role::parse(&role).map(|r| (org, r))).collect::<Vec<_>>();
            let issuer_roles: Vec<(String, String)> = sqlx::query_as("SELECT org_id, role FROM organization_members WHERE user_id = $1").bind(&issuer).fetch_all(&mut *tx).await?;
            let account_roles: Vec<(String, String)> = sqlx::query_as("SELECT org_id, role FROM organization_members WHERE user_id = $1").bind(&user_id).fetch_all(&mut *tx).await?;
            let may = crate::auth::permissions::issuer_may_still_issue(
                issuer_active == Some(true),
                &roles_of(issuer_roles),
                &roles_of(account_roles),
                active,
                mode,
                crate::api::middleware::DEFAULT_ORG_ID,
            );
            if !may {
                return Ok(ResetConfirm::IssuerRevoked);
            }
        }
        // A deactivated account is reactivated (review round 3b, S1): its
        // deactivation deleted the links issued before it, so this one came
        // from someone who may reactivate it.
        sqlx::query("UPDATE users SET password_hash = $1, is_active = TRUE, updated_at = NOW() WHERE id = $2")
            .bind(password_hash)
            .bind(&user_id)
            .execute(&mut *tx)
            .await?;
        // Whoever held the old password, or a session, an app or a code it
        // signed in, is out, in this transaction: a failure leaves the link
        // usable, and no answer leaves a sign-in behind (round 3b, S2).
        end_sign_ins(&mut tx, &user_id, None).await?;
        // A reset (the recovery of an account someone else may hold) also
        // revokes the pending invitations it sent, a stand-in a thief invited
        // say, handed back for the audit log (review round 5, finding 3).
        let revoked = revoke_pending_invitations_in(&mut *tx, None, None, Some(&user_id)).await?;
        tx.commit().await?;
        Ok(ResetConfirm::Done { user_id, reactivated: !active, revoked_invitations: revoked })
    }

    async fn mark_password_reset_used(&self, token: &str) -> Result<bool, sqlx::Error> {
        // CAS: only the first caller transitions used_at; concurrent confirms
        // with the same token can't both win (single-use guarantee).
        let res = sqlx::query(
            "UPDATE password_resets SET used_at = NOW() WHERE token = $1 AND used_at IS NULL",
        )
        .bind(crate::auth::tokens::token_hash(token))
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    // ─── Login Attempts ─────────────────────────────────────────────────

    async fn record_login_attempt(&self, email: &str, success: bool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO login_attempts (email, attempted_at, success) VALUES ($1, NOW(), $2)"
        )
        .bind(email)
        .bind(success)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn recent_failed_login_attempts(
        &self,
        email: &str,
        since: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
        let rows: Vec<(DateTime<Utc>,)> = sqlx::query_as(
            "SELECT attempted_at FROM login_attempts \
             WHERE email = $1 AND success = FALSE AND attempted_at >= $2 \
             ORDER BY attempted_at DESC"
        )
        .bind(email)
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(t,)| t).collect())
    }

    // ─── Organizations ──────────────────────────────────────────────────

    async fn create_organization(&self, o: &Organization) -> Result<(), sqlx::Error> {
        insert_organization(&self.pool, o).await
    }

    async fn create_org_with_owner(&self, org: &Organization, app: &Application, owner: crate::db::OrgOwner<'_>) -> Result<(), sqlx::Error> {
        // One transaction: returning on an error before the commit rolls
        // back whatever was written.
        let mut tx = self.pool.begin().await?;
        if let crate::db::OrgOwner::New(user) = owner {
            insert_user(&mut *tx, user).await?;
        }
        insert_organization(&mut *tx, org).await?;
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query(
            "INSERT INTO organization_members (org_id, user_id, role, created_at) \
             VALUES ($1, $2, $3, NOW())",
        )
        .bind(&org.id)
        .bind(owner.user_id())
        .bind(Role::Owner.as_str())
        .execute(&mut *tx)
        .await?;
        insert_application(&mut *tx, app).await?;
        tx.commit().await
    }

    async fn get_organization(&self, id: &str) -> Result<Option<Organization>, sqlx::Error> {
        let row = sqlx::query_as::<_, OrganizationRow>(
            "SELECT id, name, slug, plan, status, created_at, updated_at FROM organizations WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(organization_from_row))
    }

    async fn get_org_by_slug(&self, slug: &str) -> Result<Option<Organization>, sqlx::Error> {
        let row = sqlx::query_as::<_, OrganizationRow>(
            "SELECT id, name, slug, plan, status, created_at, updated_at FROM organizations WHERE slug = $1",
        )
        .bind(slug)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(organization_from_row))
    }

    async fn list_orgs_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<(Organization, Role)>, sqlx::Error> {
        let rows = sqlx::query_as::<_, OrgWithRoleRow>(
            "SELECT o.id, o.name, o.slug, o.plan, o.status, o.created_at, o.updated_at, m.role \
             FROM organizations o \
             JOIN organization_members m ON m.org_id = o.id \
             WHERE m.user_id = $1 ORDER BY o.created_at ASC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(org_with_role_from_row).collect())
    }

    async fn update_organization(
        &self,
        id: &str,
        name: Option<&str>,
    ) -> Result<Option<Organization>, sqlx::Error> {
        if let Some(n) = name {
            let result = sqlx::query(
                "UPDATE organizations SET name = $1, updated_at = NOW() WHERE id = $2",
            )
            .bind(n)
            .bind(id)
            .execute(&self.pool)
            .await?;
            if result.rows_affected() == 0 {
                return Ok(None);
            }
        }
        self.get_organization(id).await
    }

    async fn delete_organization(&self, id: &str) -> Result<bool, sqlx::Error> {
        let mut tries = 1;
        loop {
            match self.delete_organization_once(id).await {
                Err(e) if tries < TENANT_DELETE_TRIES && a_row_arrived(&e) => tries += 1,
                done => return done,
            }
        }
    }

    // ─── Members ────────────────────────────────────────────────────────

    async fn add_org_member(
        &self,
        org_id: &str,
        user_id: &str,
        role: Role,
    ) -> Result<(), sqlx::Error> {
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query(
            "INSERT INTO organization_members (org_id, user_id, role, created_at) \
             VALUES ($1, $2, $3, NOW())",
        )
        .bind(org_id)
        .bind(user_id)
        .bind(role.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_membership(
        &self,
        org_id: &str,
        user_id: &str,
    ) -> Result<Option<Role>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT role FROM organization_members WHERE org_id = $1 AND user_id = $2",
        )
        .bind(org_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(r,)| Role::parse(&r)))
    }

    async fn list_org_members(&self, org_id: &str) -> Result<Vec<OrganizationMember>, sqlx::Error> {
        let rows = sqlx::query_as::<_, OrgMemberRow>(
            "SELECT m.org_id, m.user_id, u.email, m.role, m.created_at \
             FROM organization_members m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.org_id = $1 ORDER BY m.created_at ASC",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(org_member_from_row).collect())
    }

    async fn update_member_role(
        &self,
        org_id: &str,
        user_id: &str,
        role: Role,
    ) -> Result<(), sqlx::Error> {
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE organization_members SET role = $1 WHERE org_id = $2 AND user_id = $3")
            .bind(role.as_str())
            .bind(org_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn remove_org_member(&self, org_id: &str, user_id: &str) -> Result<(), sqlx::Error> {
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("DELETE FROM organization_members WHERE org_id = $1 AND user_id = $2")
            .bind(org_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn transfer_org_owner(
        &self,
        org_id: &str,
        new_owner_user_id: &str,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Concurrent transfers queue on the org (see `transfer_deployment_owner`).
        sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR NO KEY UPDATE")
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE organization_members SET role = 'admin' WHERE org_id = $1 AND role = 'owner'")
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        let promoted =
            // Migrations 042/043's triggers delete unused reset links here (see 043's header).
            sqlx::query("UPDATE organization_members SET role = 'owner' WHERE org_id = $1 AND user_id = $2")
                .bind(org_id)
                .bind(new_owner_user_id)
                .execute(&mut *tx)
                .await?;
        if promoted.rows_affected() == 0 {
            // new owner is not a member; dropping the tx rolls back the demote
            return Err(sqlx::Error::RowNotFound);
        }
        tx.commit().await
    }

    async fn transfer_deployment_owner(
        &self,
        org_id: &str,
        new_owner_user_id: &str,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Two transfers at once must queue on the org: under READ COMMITTED
        // the second's demotion wouldn't see the Owner the first promoted,
        // and would leave two.
        sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR NO KEY UPDATE")
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        // The account rows written below, before the memberships, whose 043
        // trigger deletes the old Owner's issued links (review of round 8):
        // every change that touches an account's links locks the account's
        // row first, so none of them deadlocks with this (a link the old
        // Owner's session was issuing did). In id order.
        sqlx::query("SELECT id FROM users WHERE role = 'owner' OR id = $1 ORDER BY id FOR NO KEY UPDATE")
            .bind(new_owner_user_id)
            .execute(&mut *tx)
            .await?;
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE organization_members SET role = 'admin' WHERE org_id = $1 AND role = 'owner'")
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        let promoted =
            // Migrations 042/043's triggers delete unused reset links here (see 043's header).
            sqlx::query("UPDATE organization_members SET role = 'owner' WHERE org_id = $1 AND user_id = $2")
                .bind(org_id)
                .bind(new_owner_user_id)
                .execute(&mut *tx)
                .await?;
        if promoted.rows_affected() == 0 {
            // Not a member: dropping the tx rolls the demotion back.
            return Err(sqlx::Error::RowNotFound);
        }
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE users SET role = 'admin', updated_at = NOW() WHERE role = 'owner' AND id <> $1")
            .bind(new_owner_user_id)
            .execute(&mut *tx)
            .await?;
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE users SET role = 'owner', updated_at = NOW() WHERE id = $1")
            .bind(new_owner_user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    // ─── Applications ───────────────────────────────────────────────────

    async fn create_application(&self, a: &Application) -> Result<(), sqlx::Error> {
        insert_application(&self.pool, a).await
    }

    async fn get_application(&self, id: &str) -> Result<Option<Application>, sqlx::Error> {
        let row = sqlx::query_as::<_, ApplicationRow>(
            "SELECT id, org_id, name, slug, status, created_at, updated_at FROM applications WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(application_from_row))
    }

    async fn list_apps_for_org(&self, org_id: &str) -> Result<Vec<Application>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ApplicationRow>(
            "SELECT id, org_id, name, slug, status, created_at, updated_at \
             FROM applications WHERE org_id = $1 ORDER BY created_at ASC",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(application_from_row).collect())
    }

    async fn update_application(
        &self,
        id: &str,
        name: Option<&str>,
    ) -> Result<Option<Application>, sqlx::Error> {
        if let Some(n) = name {
            let result = sqlx::query(
                "UPDATE applications SET name = $1, updated_at = NOW() WHERE id = $2",
            )
            .bind(n)
            .bind(id)
            .execute(&self.pool)
            .await?;
            if result.rows_affected() == 0 {
                return Ok(None);
            }
        }
        self.get_application(id).await
    }

    async fn delete_application(&self, id: &str) -> Result<bool, sqlx::Error> {
        let mut tries = 1;
        loop {
            match self.delete_application_once(id).await {
                Err(e) if tries < TENANT_DELETE_TRIES && a_row_arrived(&e) => tries += 1,
                done => return done,
            }
        }
    }

    // ─── Provider Credentials (org-scoped) ─────────────────────────────

    async fn upsert_org_provider_credential(
        &self,
        org_id: &str,
        provider: &str,
        ciphertext: &str,
        nonce: &str,
        display_hint: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO org_provider_credentials \
                (id, org_id, provider, ciphertext, nonce, display_hint, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, NOW(), NOW()) \
             ON CONFLICT (org_id, provider) DO UPDATE SET \
                ciphertext = EXCLUDED.ciphertext, \
                nonce = EXCLUDED.nonce, \
                display_hint = EXCLUDED.display_hint, \
                updated_at = NOW()",
        )
        .bind(id.to_string())
        .bind(org_id)
        .bind(provider)
        .bind(ciphertext)
        .bind(nonce)
        .bind(display_hint)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_org_provider_credential(
        &self,
        org_id: &str,
        provider: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT ciphertext, nonce FROM org_provider_credentials WHERE org_id = $1 AND provider = $2",
        )
        .bind(org_id)
        .bind(provider)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn list_org_provider_credentials(
        &self,
        org_id: &str,
    ) -> Result<Vec<ProviderCredentialInfo>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ProviderCredentialRow>(
            "SELECT provider, display_hint, created_at FROM org_provider_credentials \
             WHERE org_id = $1 ORDER BY provider ASC",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(provider_credential_from_row).collect())
    }

    async fn delete_org_provider_credential(
        &self,
        org_id: &str,
        provider: &str,
    ) -> Result<bool, sqlx::Error> {
        let result =
            sqlx::query(
                "DELETE FROM org_provider_credentials WHERE org_id = $1 AND provider = $2",
            )
            .bind(org_id)
            .bind(provider)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn upsert_app_storage(&self, input: &AppStorageUpsert) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO app_storage_credentials \
                (app_id, backend, bucket_name, region, endpoint_url, custom_public_url, \
                 path_prefix, access_key_id_hint, secret_ciphertext, secret_nonce, \
                 credential_mode, role_arn, fallback_to_litegen_store, \
                 created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, NOW(), NOW()) \
             ON CONFLICT (app_id) DO UPDATE SET \
                backend = EXCLUDED.backend, \
                bucket_name = EXCLUDED.bucket_name, \
                region = EXCLUDED.region, \
                endpoint_url = EXCLUDED.endpoint_url, \
                custom_public_url = EXCLUDED.custom_public_url, \
                path_prefix = EXCLUDED.path_prefix, \
                access_key_id_hint = EXCLUDED.access_key_id_hint, \
                secret_ciphertext = EXCLUDED.secret_ciphertext, \
                secret_nonce = EXCLUDED.secret_nonce, \
                credential_mode = EXCLUDED.credential_mode, \
                role_arn = EXCLUDED.role_arn, \
                fallback_to_litegen_store = EXCLUDED.fallback_to_litegen_store, \
                updated_at = NOW()",
        )
        .bind(&input.app_id)
        .bind(&input.backend)
        .bind(&input.bucket_name)
        .bind(&input.region)
        .bind(&input.endpoint_url)
        .bind(&input.custom_public_url)
        .bind(&input.path_prefix)
        .bind(&input.access_key_id_hint)
        .bind(&input.secret_ciphertext)
        .bind(&input.secret_nonce)
        .bind(&input.credential_mode)
        .bind(&input.role_arn)
        .bind(input.fallback_to_litegen_store)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_app_storage(&self, app_id: &str) -> Result<Option<AppStorageRow>, sqlx::Error> {
        let row = sqlx::query_as::<_, AppStorageRow>(
            "SELECT backend, bucket_name, region, endpoint_url, custom_public_url, \
                    path_prefix, access_key_id_hint, secret_ciphertext, secret_nonce, updated_at, \
                    credential_mode, role_arn, fallback_to_litegen_store \
             FROM app_storage_credentials WHERE app_id = $1",
        )
        .bind(app_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn delete_app_storage(&self, app_id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM app_storage_credentials WHERE app_id = $1")
            .bind(app_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn get_app_file_access(&self, app_id: &str) -> Result<Option<crate::types::AppFileAccessRow>, sqlx::Error> {
        sqlx::query_as::<_, crate::types::AppFileAccessRow>(
            "SELECT app_id, mode, owner_scope, signed_url_ttl_seconds, keep_outputs, retention_days, signed_links, updated_at FROM app_file_access WHERE app_id = $1",
        )
        .bind(app_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn generation_file_access(&self, id: &str) -> Result<Option<(Option<String>, Option<crate::types::AppFileAccessRow>)>, sqlx::Error> {
        let sql = format!("SELECT {GENERATION_ACCESS_COLS} FROM generations g LEFT JOIN app_file_access a ON a.app_id = g.app_id WHERE g.id = $1");
        let row = sqlx::query_as::<_, GenerationAccessRow>(&sql).bind(id).fetch_optional(&self.pool).await?;
        Ok(row.map(generation_access_from_row))
    }

    async fn put_app_file_access(&self, input: &crate::types::AppFileAccessUpsert) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO app_file_access (app_id, mode, owner_scope, signed_url_ttl_seconds, keep_outputs, retention_days, signed_links, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, now()) \
             ON CONFLICT (app_id) DO UPDATE SET \
                mode = excluded.mode, \
                owner_scope = excluded.owner_scope, \
                signed_url_ttl_seconds = excluded.signed_url_ttl_seconds, \
                keep_outputs = excluded.keep_outputs, \
                retention_days = excluded.retention_days, \
                signed_links = excluded.signed_links, \
                updated_at = now()",
        )
        .bind(&input.app_id)
        .bind(&input.mode)
        .bind(&input.owner_scope)
        .bind(input.signed_url_ttl_seconds)
        .bind(input.keep_outputs)
        .bind(input.retention_days)
        .bind(&input.signed_links)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn apps_with_retention(&self) -> Result<Vec<(String, i64)>, sqlx::Error> {
        sqlx::query_as("SELECT app_id, retention_days FROM app_file_access WHERE retention_days IS NOT NULL")
            .fetch_all(&self.pool)
            .await
    }

    async fn generations_due_for_retention(
        &self,
        app_id: Option<&str>,
        cutoff: chrono::DateTime<chrono::Utc>,
        limit: i64,
    ) -> Result<Vec<Generation>, sqlx::Error> {
        let rows = match app_id {
            Some(app) => {
                let sql = format!("SELECT {cols} FROM generations WHERE app_id = $1 AND files_deleted_at IS NULL AND status IN ('completed', 'failed', 'cancelled') AND created_at < $2 ORDER BY files_delete_attempted_at ASC NULLS FIRST, created_at ASC LIMIT $3", cols = GENERATION_COLS);
                sqlx::query_as::<_, GenerationRow>(&sql).bind(app).bind(cutoff).bind(limit).fetch_all(&self.pool).await?
            }
            None => {
                let sql = format!("SELECT {cols} FROM generations WHERE (app_id IS NULL OR app_id NOT IN (SELECT app_id FROM app_file_access WHERE retention_days IS NOT NULL)) AND files_deleted_at IS NULL AND status IN ('completed', 'failed', 'cancelled') AND created_at < $1 ORDER BY files_delete_attempted_at ASC NULLS FIRST, created_at ASC LIMIT $2", cols = GENERATION_COLS);
                sqlx::query_as::<_, GenerationRow>(&sql).bind(cutoff).bind(limit).fetch_all(&self.pool).await?
            }
        };
        Ok(rows.into_iter().map(generation_from_row).collect())
    }

    async fn mark_files_deleted(&self, id: &str, at: chrono::DateTime<chrono::Utc>, reason: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE generations SET files_deleted_at = $1, files_deleted_reason = $2 WHERE id = $3 AND files_deleted_at IS NULL")
            .bind(at)
            .bind(reason)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn clear_artifact_output(&self, request_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE request_artifacts SET output_kind = 'deleted', output_value = NULL, output_truncated = FALSE \
             WHERE request_id = $1 AND output_kind IN ('b64', 'url')",
        )
        .bind(request_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn note_files_delete_failed(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE generations SET files_delete_attempted_at = now() WHERE id = $1 AND files_deleted_at IS NULL")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn redact_generation_request(&self, id: &str) -> Result<(), sqlx::Error> {
        let before: Option<Option<String>> = sqlx::query_scalar("SELECT metadata FROM generations WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        if let Some(Some(text)) = before {
            if let Ok(mut meta) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(obj) = meta.as_object_mut() {
                    obj.remove("request");
                }
                // Only over what was read: a write in between keeps its own.
                sqlx::query("UPDATE generations SET metadata = $1 WHERE id = $2 AND metadata = $3")
                    .bind(meta.to_string())
                    .bind(id)
                    .bind(&text)
                    .execute(&self.pool)
                    .await?;
            }
        }
        sqlx::query("DELETE FROM request_artifacts WHERE request_id = $1").bind(id).execute(&self.pool).await?;
        // The artifacts of the cache hits it answered name it, and carry its
        // prompt and params: the same request.
        sqlx::query("DELETE FROM request_artifacts WHERE output_kind = 'generation' AND output_value = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn revoke_links(&self, scope: &str, scope_id: &str, at: chrono::DateTime<chrono::Utc>) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO link_revocations (scope, scope_id, revoked_at) VALUES ($1, $2, $3) ON CONFLICT (scope, scope_id) DO UPDATE SET revoked_at = GREATEST(link_revocations.revoked_at, excluded.revoked_at)")
            .bind(scope)
            .bind(scope_id)
            .bind(at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn links_revoked_at(&self, app_id: Option<&str>, generation_id: &str) -> Result<Option<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
        // At most three rows; the latest is taken here rather than by the
        // database, so stored timestamp text is never compared as text.
        let times: Vec<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar("SELECT revoked_at FROM link_revocations WHERE (scope = 'gateway' AND scope_id = '') OR (scope = 'app' AND scope_id = $1) OR (scope = 'generation' AND scope_id = $2)")
            .bind(app_id.unwrap_or(""))
            .bind(generation_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(times.into_iter().max())
    }

    async fn delete_app_file_access(&self, app_id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM app_file_access WHERE app_id = $1")
            .bind(app_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ─── Server secrets ─────────────────────────────────────────────────

    async fn server_secret(&self, name: &str, generate: &(dyn Fn() -> String + Send + Sync)) -> Result<String, sqlx::Error> {
        // The insert keeps the first value ever stored, whoever races for it;
        // the select then reads that value, which is the caller's own only if
        // its insert won.
        sqlx::query("INSERT INTO server_secrets (name, value) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING")
            .bind(name)
            .bind(generate())
            .execute(&self.pool)
            .await?;
        sqlx::query_scalar("SELECT value FROM server_secrets WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await
    }

    // ─── Model Allowlists ───────────────────────────────────────────────────────

    async fn get_org_allowed_models(&self, org_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT model_id FROM org_allowed_models WHERE org_id = $1 ORDER BY model_id")
                .bind(org_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(m,)| m).collect())
    }

    async fn set_org_allowed_models(
        &self,
        org_id: &str,
        models: &[String],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM org_allowed_models WHERE org_id = $1")
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        for model_id in models {
            sqlx::query("INSERT INTO org_allowed_models (org_id, model_id) VALUES ($1, $2)")
                .bind(org_id)
                .bind(model_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }

    async fn get_app_model_access(
        &self,
        app_id: &str,
    ) -> Result<(String, Vec<String>), sqlx::Error> {
        let mode_row: Option<(String,)> =
            sqlx::query_as("SELECT mode FROM app_model_access WHERE app_id = $1")
                .bind(app_id)
                .fetch_optional(&self.pool)
                .await?;
        let mode = mode_row.map(|(m,)| m).unwrap_or_else(|| "all".to_string());
        let ids: Vec<(String,)> =
            sqlx::query_as("SELECT model_id FROM app_model_access_ids WHERE app_id = $1 ORDER BY model_id")
                .bind(app_id)
                .fetch_all(&self.pool)
                .await?;
        Ok((mode, ids.into_iter().map(|(m,)| m).collect()))
    }

    async fn set_app_model_access(
        &self,
        app_id: &str,
        mode: &str,
        models: &[String],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO app_model_access (app_id, mode) VALUES ($1, $2) \
             ON CONFLICT (app_id) DO UPDATE SET mode = EXCLUDED.mode",
        )
        .bind(app_id)
        .bind(mode)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM app_model_access_ids WHERE app_id = $1")
            .bind(app_id)
            .execute(&mut *tx)
            .await?;
        for model_id in models {
            sqlx::query("INSERT INTO app_model_access_ids (app_id, model_id) VALUES ($1, $2)")
                .bind(app_id)
                .bind(model_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }

    // ─── API Keys (tenant-scoped) ───────────────────────────────────────

    async fn create_api_key_scoped(
        &self,
        org_id: &str,
        app_id: &str,
        public_id: &str,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
        token_quota: Option<f64>,
        rpm_limit: Option<u32>,
        scopes: &str,
        webhook_url: Option<&str>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<ApiKey, sqlx::Error> {
        // "" is no webhook (review 2026-11-15 round 2, finding 4).
        let webhook_url = webhook_url.filter(|u| !u.is_empty());
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO api_keys \
                (id, org_id, app_id, public_id, name, key_hash, key_prefix, is_active, \
                 tokens_used, token_quota, rpm_limit, scopes, webhook_url, expires_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE, 0, $8, $9, $10, $11, $12, NOW())",
        )
        .bind(id.to_string())
        .bind(org_id)
        .bind(app_id)
        .bind(public_id)
        .bind(name)
        .bind(key_hash)
        .bind(key_prefix)
        .bind(token_quota)
        .bind(rpm_limit.map(|v| v as i32))
        .bind(scopes)
        .bind(webhook_url)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        self.get_api_key(&id).await?.ok_or(sqlx::Error::RowNotFound)
    }

    async fn list_api_keys_for_app(&self, app_id: &str) -> Result<Vec<ApiKey>, sqlx::Error> {
        let sql = format!(
            "SELECT {} FROM api_keys WHERE app_id = $1 ORDER BY created_at DESC",
            API_KEY_COLS
        );
        let rows = sqlx::query_as::<_, ApiKeyRow>(&sql)
            .bind(app_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(api_key_from_row).collect())
    }

    // ─── Tenant-scoped reads ────────────────────────────────────────────

    async fn list_active_generations_for_tenant(&self, org_id: &str, app_id: Option<&str>) -> Result<Vec<Generation>, sqlx::Error> {
        let sql = format!(
            "SELECT {} FROM generations WHERE org_id = $1 AND ($2::text IS NULL OR app_id = $2) \
             AND status IN ('pending', 'processing') ORDER BY created_at ASC",
            GENERATION_COLS
        );
        let rows = sqlx::query_as::<_, GenerationRow>(&sql)
            .bind(org_id)
            .bind(app_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(generation_from_row).collect())
    }

    async fn list_generations_for_tenant(
        &self,
        org_id: &str,
        app_id: Option<&str>,
        page: u32,
        per_page: u32,
    ) -> Result<Vec<Generation>, sqlx::Error> {
        let offset = crate::api::pagination::offset(page, per_page) as i64;
        let rows = match app_id {
            Some(app) => {
                let sql = format!(
                    "SELECT {} FROM generations WHERE org_id = $1 AND app_id = $2 \
                     ORDER BY created_at DESC LIMIT $3 OFFSET $4",
                    GENERATION_COLS
                );
                sqlx::query_as::<_, GenerationRow>(&sql)
                    .bind(org_id)
                    .bind(app)
                    .bind(per_page as i64)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                let sql = format!(
                    "SELECT {} FROM generations WHERE org_id = $1 \
                     ORDER BY created_at DESC LIMIT $2 OFFSET $3",
                    GENERATION_COLS
                );
                sqlx::query_as::<_, GenerationRow>(&sql)
                    .bind(org_id)
                    .bind(per_page as i64)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        Ok(rows.into_iter().map(generation_from_row).collect())
    }

    async fn count_generations_for_tenant(
        &self,
        org_id: &str,
        app_id: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = match app_id {
            Some(app) => sqlx::query_as(
                "SELECT COUNT(*) FROM generations WHERE org_id = $1 AND app_id = $2",
            )
            .bind(org_id)
            .bind(app)
            .fetch_one(&self.pool)
            .await?,
            None => sqlx::query_as("SELECT COUNT(*) FROM generations WHERE org_id = $1")
                .bind(org_id)
                .fetch_one(&self.pool)
                .await?,
        };
        Ok(row.0)
    }

    async fn list_generations_for_owner(
        &self,
        org_id: &str,
        app_id: Option<&str>,
        key_ids: &[Uuid],
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<Generation>, u64), sqlx::Error> {
        if key_ids.is_empty() {
            return Ok((Vec::new(), 0));
        }
        use sqlx::QueryBuilder;

        // COUNT of every matching row (not just the current page).
        let mut cb = QueryBuilder::new("SELECT COUNT(*) FROM generations WHERE org_id = ");
        cb.push_bind(org_id.to_string());
        if let Some(app) = app_id {
            cb.push(" AND app_id = ").push_bind(app.to_string());
        }
        cb.push(" AND key_id IN (");
        {
            let mut sep = cb.separated(", ");
            for kid in key_ids {
                sep.push_bind(kid.to_string());
            }
        }
        cb.push(")");
        let (total,): (i64,) = cb.build_query_as().fetch_one(&self.pool).await?;

        // The requested page, filtered + ordered + limited at the DB.
        let offset = crate::api::pagination::offset(page, per_page) as i64;
        let mut qb = QueryBuilder::new(format!("SELECT {GENERATION_COLS} FROM generations WHERE org_id = "));
        qb.push_bind(org_id.to_string());
        if let Some(app) = app_id {
            qb.push(" AND app_id = ").push_bind(app.to_string());
        }
        qb.push(" AND key_id IN (");
        {
            let mut sep = qb.separated(", ");
            for kid in key_ids {
                sep.push_bind(kid.to_string());
            }
        }
        qb.push(") ORDER BY created_at DESC LIMIT ")
            .push_bind(per_page as i64)
            .push(" OFFSET ")
            .push_bind(offset);
        let rows: Vec<GenerationRow> = qb.build_query_as().fetch_all(&self.pool).await?;

        Ok((rows.into_iter().map(generation_from_row).collect(), total as u64))
    }

    async fn get_request_logs_for_tenant(
        &self,
        org_id: &str,
        app_id: Option<&str>,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
        let offset = crate::api::pagination::offset(page, per_page) as i64;
        const COLS: &str = REQUEST_LOG_COLS;
        let (total, rows) = match app_id {
            Some(app) => {
                let total: (i64,) = sqlx::query_as(
                    "SELECT COUNT(*) FROM request_logs WHERE org_id = $1 AND app_id = $2",
                )
                .bind(org_id)
                .bind(app)
                .fetch_one(&self.pool)
                .await?;
                let sql = format!(
                    "SELECT {COLS} FROM request_logs WHERE org_id = $1 AND app_id = $2 \
                     ORDER BY created_at DESC LIMIT $3 OFFSET $4"
                );
                let rows = sqlx::query_as::<_, RequestLogRow>(&sql)
                    .bind(org_id)
                    .bind(app)
                    .bind(per_page as i64)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await?;
                (total, rows)
            }
            None => {
                let total: (i64,) =
                    sqlx::query_as("SELECT COUNT(*) FROM request_logs WHERE org_id = $1")
                        .bind(org_id)
                        .fetch_one(&self.pool)
                        .await?;
                let sql = format!(
                    "SELECT {COLS} FROM request_logs WHERE org_id = $1 \
                     ORDER BY created_at DESC LIMIT $2 OFFSET $3"
                );
                let rows = sqlx::query_as::<_, RequestLogRow>(&sql)
                    .bind(org_id)
                    .bind(per_page as i64)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await?;
                (total, rows)
            }
        };
        let logs = rows.into_iter().map(request_log_from_row).collect();
        Ok((logs, total.0 as u64))
    }

    async fn list_audit_log_for_tenant(
        &self,
        org_id: &str,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<AuditLogEntry>, i64), sqlx::Error> {
        let offset = crate::api::pagination::offset(page, per_page) as i64;
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM audit_log WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&self.pool)
            .await?;
        let rows = sqlx::query_as::<_, AuditLogRow>(
            "SELECT id, actor_key_id, actor_label, action, target_type, target_id, \
             before_json, after_json, created_at, org_id FROM audit_log WHERE org_id = $1 \
             ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(org_id)
        .bind(per_page as i64)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let entries = rows.into_iter().map(audit_log_from_row).collect();
        Ok((entries, total.0))
    }

    async fn get_request_logs_for_tenant_filtered(
        &self,
        org_id: &str,
        app_id: Option<&str>,
        filter: &crate::db::RequestLogFilter<'_>,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
        use sqlx::QueryBuilder;
        // created_at is TIMESTAMPTZ, so bind the range as real timestamps (a
        // string bind would fail the type check — see the TIMESTAMPTZ-vs-TEXT bug).
        let mut cb = QueryBuilder::<sqlx::Postgres>::new("SELECT COUNT(*) FROM request_logs WHERE org_id = ");
        cb.push_bind(org_id.to_string());
        if let Some(app) = app_id { cb.push(" AND app_id = ").push_bind(app.to_string()); }
        if let Some(m) = filter.model { cb.push(" AND model = ").push_bind(m.to_string()); }
        if let Some(p) = filter.provider { cb.push(" AND provider = ").push_bind(p.to_string()); }
        if let Some(s) = filter.status { cb.push(" AND status = ").push_bind(s.to_string()); }
        if let Some(f) = filter.from { cb.push(" AND created_at >= ").push_bind(f); }
        if let Some(t) = filter.to { cb.push(" AND created_at <= ").push_bind(t); }
        for (k, v) in filter.metadata { cb.push(" AND (metadata::jsonb ->> ").push_bind(k.clone()).push(") = ").push_bind(v.clone()); }
        let (total,): (i64,) = cb.build_query_as().fetch_one(&self.pool).await?;

        let offset = crate::api::pagination::offset(page, per_page) as i64;
        let mut qb = QueryBuilder::<sqlx::Postgres>::new(format!("SELECT {REQUEST_LOG_COLS} FROM request_logs WHERE org_id = "));
        qb.push_bind(org_id.to_string());
        if let Some(app) = app_id { qb.push(" AND app_id = ").push_bind(app.to_string()); }
        if let Some(m) = filter.model { qb.push(" AND model = ").push_bind(m.to_string()); }
        if let Some(p) = filter.provider { qb.push(" AND provider = ").push_bind(p.to_string()); }
        if let Some(s) = filter.status { qb.push(" AND status = ").push_bind(s.to_string()); }
        if let Some(f) = filter.from { qb.push(" AND created_at >= ").push_bind(f); }
        if let Some(t) = filter.to { qb.push(" AND created_at <= ").push_bind(t); }
        for (k, v) in filter.metadata { qb.push(" AND (metadata::jsonb ->> ").push_bind(k.clone()).push(") = ").push_bind(v.clone()); }
        qb.push(" ORDER BY created_at DESC LIMIT ").push_bind(per_page as i64).push(" OFFSET ").push_bind(offset);
        let rows: Vec<RequestLogRow> = qb.build_query_as().fetch_all(&self.pool).await?;
        Ok((rows.into_iter().map(request_log_from_row).collect(), total as u64))
    }

    async fn list_audit_log_for_tenant_filtered(
        &self,
        org: crate::db::AuditLogOrg<'_>,
        filter: &crate::db::TenantAuditFilter<'_>,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<AuditLogEntry>, u64), sqlx::Error> {
        use sqlx::QueryBuilder;
        const COLS: &str = "id, actor_key_id, actor_label, action, target_type, target_id, before_json, after_json, created_at, org_id";
        // One org's entries, or those filed under no org (`org_id` NULL).
        let mut cb = QueryBuilder::<sqlx::Postgres>::new("SELECT COUNT(*) FROM audit_log WHERE org_id ");
        match org {
            crate::db::AuditLogOrg::Org(o) => cb.push("= ").push_bind(o.to_string()),
            crate::db::AuditLogOrg::NoOrg => cb.push("IS NULL"),
        };
        if let Some(a) = filter.actor_key_id { cb.push(" AND actor_key_id = ").push_bind(a.to_string()); }
        if let Some(a) = filter.action { cb.push(" AND action = ").push_bind(a.to_string()); }
        if let Some(f) = filter.from { cb.push(" AND created_at >= ").push_bind(f); }
        if let Some(t) = filter.to { cb.push(" AND created_at <= ").push_bind(t); }
        let (total,): (i64,) = cb.build_query_as().fetch_one(&self.pool).await?;

        let offset = crate::api::pagination::offset(page, per_page) as i64;
        let mut qb = QueryBuilder::<sqlx::Postgres>::new(format!("SELECT {COLS} FROM audit_log WHERE org_id "));
        match org {
            crate::db::AuditLogOrg::Org(o) => qb.push("= ").push_bind(o.to_string()),
            crate::db::AuditLogOrg::NoOrg => qb.push("IS NULL"),
        };
        if let Some(a) = filter.actor_key_id { qb.push(" AND actor_key_id = ").push_bind(a.to_string()); }
        if let Some(a) = filter.action { qb.push(" AND action = ").push_bind(a.to_string()); }
        if let Some(f) = filter.from { qb.push(" AND created_at >= ").push_bind(f); }
        if let Some(t) = filter.to { qb.push(" AND created_at <= ").push_bind(t); }
        qb.push(" ORDER BY created_at DESC LIMIT ").push_bind(per_page as i64).push(" OFFSET ").push_bind(offset);
        let rows: Vec<AuditLogRow> = qb.build_query_as().fetch_all(&self.pool).await?;
        Ok((rows.into_iter().map(audit_log_from_row).collect(), total as u64))
    }
}
