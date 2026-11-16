use std::path::{Path, PathBuf};
use std::str::FromStr;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::migrate::MigrateError;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::QueryBuilder;
use tracing::info;
use uuid::Uuid;

use crate::types::*;

use super::trait_def::DatabaseStore;

/// Full column list for api_keys selects — avoids repetition.
const API_KEY_COLS: &str = "id, name, key_hash, key_prefix, created_at, expires_at, is_active, \
    token_quota, tokens_used, rpm_limit, scopes, webhook_url, owner_user_id, org_id, app_id, public_id";

/// Full column list for request_logs selects — shared across backends.
pub(crate) const REQUEST_LOG_COLS: &str =
    "id, model, provider, status, media_type, cost_usd, latency_ms, error, metadata, created_at, cache_hit";

/// Full column list for generations selects.
const GENERATION_COLS: &str = "id, key_id, model, provider, media_type, status, progress, \
    provider_job_id, result_url, error_message, cost_usd, created_at, completed_at, metadata, \
    org_id, app_id, files_deleted_at, files_deleted_reason";

/// SQLite-backed database implementation.
pub struct SqliteDatabase {
    pool: SqlitePool,
    /// Tests only: how far "the database's clock" is set from this process's,
    /// standing in for a replica whose own clock is off.
    #[cfg(test)]
    clock_offset: chrono::Duration,
}

impl SqliteDatabase {
    /// Connect to SQLite and run migrations.
    ///
    /// A SQLite URL (`sqlite:…`) with `mode=rwc` creates its database, and the
    /// directory it goes in; without `mode=rwc` the file must exist, as SQLite
    /// has it. A database litegen can't open, or that the first migration
    /// can't write, is refused with an error that names its file and the fix.
    ///
    /// A URL for another database (`mysql://…`) is refused before sqlx reads
    /// it, naming only its scheme. A value with no scheme at all is read as a
    /// file name, as sqlx does, but it may be no path (a Postgres `host=…
    /// password=…` string), so no directory is made for it and errors don't
    /// repeat it.
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        refuse_other_schemes(database_url)?;
        let options = SqliteConnectOptions::from_str(database_url)?;
        let named = database_url.starts_with("sqlite:");
        let file = DatabaseFile {
            path: options.get_filename().to_path_buf(),
            creates: named && asks_to_create(database_url),
            named,
        };
        if file.creates {
            // Empty for an in-memory database, and for a file in the working directory.
            if let Some(dir) = file.path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                tokio::fs::create_dir_all(dir).await.map_err(|e| file.cannot_open(&format!("cannot create {}: {e}", absolute(dir))))?;
            }
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .connect_with(options)
            .await
            .map_err(|e| file.cannot_open(&cause(&e)))?;

        sqlx::migrate!("./migrations/sqlite").run(&pool).await.map_err(|e| file.migration_failed(e))?;
        info!("SQLite database connected and migrations applied");

        Ok(Self {
            pool,
            #[cfg(test)]
            clock_offset: chrono::Duration::zero(),
        })
    }

    /// Tests only: this database's clock, `offset` from this process's.
    #[cfg(test)]
    pub(crate) fn with_clock_offset(mut self, offset: chrono::Duration) -> Self {
        self.clock_offset = offset;
        self
    }

    /// The database's clock. SQLite runs in this process, on this host's
    /// clock, so it is the process's own (moved by the tests' offset).
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now() + self.clock_offset()
    }

    #[cfg(test)]
    fn clock_offset(&self) -> chrono::Duration {
        self.clock_offset
    }

    #[cfg(not(test))]
    fn clock_offset(&self) -> chrono::Duration {
        chrono::Duration::zero()
    }

    /// The underlying pool: used by the management store
    /// (`crate::management::sql`) and by unit tests that stamp rows directly.
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Write `after` as generation `id`'s metadata only if it is still `before`
    /// and the row is in flight: one compare-and-swap step. `true` when it took.
    async fn swap_metadata(&self, id: &str, before: Option<&str>, after: &serde_json::Value) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE generations SET metadata = ? \
                 WHERE id = ? AND status IN ('pending', 'processing') AND metadata IS ?",
        )
        .bind(serde_json::to_string(after).unwrap_or_else(|_| "{}".into()))
        .bind(id)
        .bind(before)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// A row's status and its `metadata` exactly as stored: the "before" a
    /// compare-and-swap on the column compares against.
    async fn status_and_metadata(&self, id: &str) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
        sqlx::query_as("SELECT status, metadata FROM generations WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }
}

/// Whether a SQLite URL asks for its database to be created: `mode=rwc` in its
/// query, which is how sqlx reads it.
fn asks_to_create(url: &str) -> bool {
    url.split_once('?').is_some_and(|(_, query)| query.split('&').any(|pair| pair == "mode=rwc"))
}

/// The database URLs litegen opens, as an error that refuses one says.
const URL_FORMS: &str = "use sqlite:// for SQLite, or postgres:// (postgresql://) for Postgres";

/// Refuse a URL for another database before sqlx reads it. sqlx takes all of
/// a URL but a `sqlite:` prefix as a file name, so `mysql://user:password@host/db`
/// would be a path: named in errors, password and all, and with `?mode=rwc`
/// made into folders and a new, empty SQLite database. The error names the
/// scheme only when RFC 3986 allows it (a letter, then letters, digits, `+`,
/// `-` or `.`; 32 at most), and nothing after it, which may hold a password.
fn refuse_other_schemes(url: &str) -> Result<(), sqlx::Error> {
    if url.starts_with("sqlite:") {
        return Ok(());
    }
    let Some((scheme, _)) = url.split_once("://") else { return Ok(()) };
    let is_scheme = scheme.len() <= 32
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    let which = if is_scheme { format!("the database URL's scheme `{scheme}`") } else { "this database URL's scheme".to_string() };
    Err(sqlx::Error::Configuration(format!("{which} is not one litegen opens: {URL_FORMS}").into()))
}

/// SQLITE_READONLY. Its extended codes share the low byte:
/// SQLITE_READONLY_DIRECTORY (1544) is a directory where the journal can't be
/// made.
const SQLITE_READONLY: i32 = 8;

/// The primary SQLite result code of `e`: the low byte of the extended code
/// sqlx reports.
fn primary_code(e: &sqlx::Error) -> Option<i32> {
    e.as_database_error()?.code()?.parse::<i32>().ok().map(|code| code & 0xff)
}

/// What went wrong, without sqlx's "error returned from database" around it.
fn cause(e: &sqlx::Error) -> String {
    e.as_database_error().map_or_else(|| e.to_string(), |db| db.to_string())
}

/// `path`, absolute, as an operator finds it.
fn absolute(path: &Path) -> String {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).display().to_string()
}

/// The file of a SQLite database, as an error about it names it: SQLite's own
/// errors ("unable to open database file") say neither which file nor what to
/// do.
struct DatabaseFile {
    path: PathBuf,
    /// A SQLite URL with `mode=rwc`: litegen creates the file and its directory.
    creates: bool,
    /// A SQLite URL (`sqlite:…`), so errors name its file. A value with no
    /// scheme is read as a file name too, but it may be no path at all, so
    /// errors never repeat it ([`READ_AS_A_FILE`]).
    named: bool,
}

/// What an error says, in place of the file, about a database value that is
/// read as a file name without being a SQLite URL.
const READ_AS_A_FILE: &str = "the database URL starts with neither sqlite: nor postgres://, so litegen read it as a SQLite \
     file name (not repeated here: it may hold a password)";

impl DatabaseFile {
    /// The file can't be opened, or its directory made.
    fn cannot_open(&self, cause: &str) -> sqlx::Error {
        if !self.named {
            return sqlx::Error::Configuration(
                format!("litegen cannot open its SQLite database ({cause}): {READ_AS_A_FILE}; {URL_FORMS}").into(),
            );
        }
        let create = if self.creates { "" } else { "; add ?mode=rwc to the URL to have litegen create it" };
        sqlx::Error::Configuration(
            format!(
                "litegen cannot open its SQLite database {} ({cause}); its directory must exist and be writable by the user \
                 litegen runs as (uid 1000 in the Docker image){create}, or set LITEGEN_DATABASE_URL",
                absolute(&self.path)
            )
            .into(),
        )
    }

    /// A migration failed. SQLite opens a file it can't write read-only, so a
    /// file or directory litegen can't write fails here, at the first write,
    /// with SQLITE_READONLY: that is named like a file that can't be opened.
    /// Any other failure is the migration's own.
    fn migration_failed(&self, e: MigrateError) -> sqlx::Error {
        let read_only = match &e {
            MigrateError::Execute(inner) | MigrateError::ExecuteMigration(inner, _) => {
                (primary_code(inner) == Some(SQLITE_READONLY)).then(|| cause(inner))
            }
            _ => None,
        };
        let Some(cause) = read_only else { return sqlx::Error::Migrate(Box::new(e)) };
        let file = if self.named { format!(" {}", absolute(&self.path)) } else { String::new() };
        let why = if self.named { String::new() } else { format!(": {READ_AS_A_FILE}") };
        sqlx::Error::Configuration(
            format!(
                "litegen cannot write its SQLite database{file} ({cause}){why}; the file and its directory must be writable by \
                 the user litegen runs as (uid 1000 in the Docker image), or set LITEGEN_DATABASE_URL"
            )
            .into(),
        )
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

/// Claim an `Idempotency-Key` ([`DatabaseStore::idempotency_begin_held`]),
/// the record held by `held_by` for `lease`. A record in progress is free once
/// its lease lapsed; one with no lease (an older replica's, or one from before
/// migration 030) once it is older than `IDEMPOTENCY_IN_PROGRESS_STALE`. The
/// lease is SQLite's own `strftime` text, written and compared in SQL only.
async fn begin_idempotency(
    db: &SqliteDatabase,
    scope: &str,
    key: &str,
    request_hash: &str,
    held_by: &str,
    lease: chrono::Duration,
) -> Result<crate::db::IdempotencyBegin, sqlx::Error> {
    use crate::db::IdempotencyBegin;
    // The database's clock, as every replica's records are judged on it.
    let now = db.now();
    // Opportunistic purge: the index on created_at keeps this cheap.
    sqlx::query("DELETE FROM idempotency_keys WHERE created_at < ?")
        .bind(now - crate::db::IDEMPOTENCY_TTL)
        .execute(&db.pool)
        .await?;
    for _ in 0..4 {
        let inserted = sqlx::query(
            "INSERT INTO idempotency_keys (scope, idem_key, request_hash, state, created_at, held_by, held_until) \
             VALUES (?, ?, ?, 'in_progress', ?, ?, strftime('%Y-%m-%d %H:%M:%f', 'now', ?)) ON CONFLICT (scope, idem_key) DO NOTHING",
        )
        .bind(scope)
        .bind(key)
        .bind(request_hash)
        .bind(now)
        .bind(held_by)
        .bind(sqlite_seconds(lease))
        .execute(&db.pool)
        .await?;
        if inserted.rows_affected() > 0 {
            return Ok(IdempotencyBegin::Started);
        }
        #[allow(clippy::type_complexity)]
        let row: Option<(String, String, Option<i64>, Option<String>, Option<Vec<u8>>, chrono::DateTime<chrono::Utc>, Option<bool>)> = sqlx::query_as(
            "SELECT request_hash, state, CAST(response_status AS BIGINT), response_headers, response_body, created_at, \
                    held_until < strftime('%Y-%m-%d %H:%M:%f', 'now') \
             FROM idempotency_keys WHERE scope = ? AND idem_key = ?",
        )
        .bind(scope)
        .bind(key)
        .fetch_optional(&db.pool)
        .await?;
        let Some((hash, state, status, headers, body, created_at, lapsed)) = row else { continue };
        // In progress: a leased record until its lease lapses, one with
        // no lease (NULL) for the stale window.
        let abandoned = match lapsed {
            Some(lapsed) => lapsed,
            None => now - created_at > crate::db::IDEMPOTENCY_IN_PROGRESS_STALE,
        };
        let expired = now - created_at > crate::db::IDEMPOTENCY_TTL || (state == "in_progress" && abandoned);
        if expired {
            // Only the record we looked at: a fresh one a racer just made stays.
            sqlx::query("DELETE FROM idempotency_keys WHERE scope = ? AND idem_key = ? AND created_at = ?")
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
impl DatabaseStore for SqliteDatabase {
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
            VALUES (?, ?, ?, ?, ?, 'pending', 0, ?, ?, COALESCE(?, ?), COALESCE(?, ?), datetime('now'))
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
            SET status = ?, progress = ?, result_url = ?,
                error_message = ?, completed_at = ?
            WHERE id = ?
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
        sqlx::query("UPDATE generations SET metadata = ? WHERE id = ?")
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
            "UPDATE idempotency_keys SET state = 'completed', response_status = ?, response_headers = ?, response_body = ? \
             WHERE scope = ? AND idem_key = ? AND held_by = ?",
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
        sqlx::query("DELETE FROM idempotency_keys WHERE scope = ? AND idem_key = ? AND state = 'in_progress' AND held_by = ?")
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
            let Some((status, before)) = self.status_and_metadata(id).await? else {
                return Ok(crate::db::GenerationWrite::Missing);
            };
            if !is_in_flight(&status) {
                return Ok(crate::db::GenerationWrite::AlreadyTerminal);
            }
            let merged = crate::db::merge_metadata(parse_metadata(before.as_deref()).as_ref(), patch);
            let res = sqlx::query(
                "UPDATE generations SET metadata = ? \
                 WHERE id = ? AND status IN ('pending', 'processing') AND metadata IS ?",
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
            let Some((status, before)) = self.status_and_metadata(id).await? else { return Ok(false) };
            if !is_in_flight(&status) {
                return Ok(false);
            }
            let meta = parse_metadata(before.as_deref());
            let claims = meta.as_ref().and_then(|m| m.get("stage_claims"));
            if claims.and_then(|c| c.get(stage)).is_some() {
                return Ok(false);
            }
            let mut claims = claims.filter(|c| c.is_object()).cloned().unwrap_or_else(|| serde_json::json!({}));
            claims[stage] = crate::db::stage_claim_record(claimant, self.now());
            let merged = crate::db::merge_metadata(meta.as_ref(), &serde_json::json!({ "stage_claims": claims }));
            let res = sqlx::query(
                "UPDATE generations SET metadata = ? \
                 WHERE id = ? AND status IN ('pending', 'processing') AND metadata IS ?",
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
            let Some((status, before)) = self.status_and_metadata(id).await? else { return Ok(true) };
            if !is_in_flight(&status) {
                return Ok(false);
            }
            let meta = parse_metadata(before.as_deref());
            let now = self.now();
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
            let Some((status, before)) = self.status_and_metadata(id).await? else { return Ok(()) };
            let meta = parse_metadata(before.as_deref());
            let held = meta.as_ref().and_then(|m| m.get("leases")).and_then(|l| l.get(work)).and_then(|l| l.get("by")).and_then(|b| b.as_str());
            if !is_in_flight(&status) || held != Some(claimant) {
                return Ok(());
            }
            let merged = crate::db::merge_metadata(meta.as_ref(), &crate::db::leases_patch(meta.as_ref(), work, None, self.now()));
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
                let Some((current, before)) = self.status_and_metadata(id).await? else {
                    return Ok(crate::db::GenerationWrite::Missing);
                };
                if !is_in_flight(&current) {
                    return Ok(crate::db::GenerationWrite::AlreadyTerminal);
                }
                let merged = crate::db::merge_metadata(parse_metadata(before.as_deref()).as_ref(), patch);
                let res = sqlx::query(
                    r#"
                    UPDATE generations
                    SET status = ?, progress = ?, result_url = ?,
                        error_message = ?, completed_at = ?, metadata = ?
                    WHERE id = ? AND status IN ('pending', 'processing') AND metadata IS ?
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
        // `COALESCE(?, metadata)` leaves the column alone when no metadata is
        // supplied, so status and assets still land in ONE statement.
        let meta = metadata.map(|m| serde_json::to_string(m).unwrap_or_else(|_| "null".into()));
        let res = sqlx::query(
            r#"
            UPDATE generations
            SET status = ?, progress = ?, result_url = ?,
                error_message = ?, completed_at = ?, metadata = COALESCE(?, metadata)
            WHERE id = ? AND status IN ('pending', 'processing')
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
        // Zero rows means one of two very different things; only the extra
        // lookup (rare — it runs only when the guard bit) tells them apart.
        let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM generations WHERE id = ?")
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
        let sql = format!("SELECT {} FROM generations WHERE id = ?", GENERATION_COLS);
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
                "SELECT {} FROM generations WHERE key_id = ? OR key_id IS NULL \
                 ORDER BY created_at DESC LIMIT ? OFFSET ?",
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
                "SELECT {} FROM generations ORDER BY created_at DESC LIMIT ? OFFSET ?",
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
                "SELECT COUNT(*) FROM generations WHERE key_id = ? OR key_id IS NULL",
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
        // Use RETURNING to atomically update and retrieve. SQLite >= 3.35 supports RETURNING.
        let sql = format!(
            "UPDATE generations \
             SET status = 'cancelled', completed_at = datetime('now') \
             WHERE id = ? AND status IN ('pending', 'processing') \
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
             ORDER BY created_at ASC LIMIT ?",
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
        // One statement, so SQLite's single writer makes the claim atomic.
        // `poll_claimed_until` is only ever written and compared as
        // strftime's millisecond text on SQLite's own clock (never one bound
        // from the app), so both sides of the comparison share one format.
        // The rows polled longest ago go first (a row never polled counts as
        // claimed at its creation), so a replica with more active rows than
        // `limit` takes them in turn instead of re-taking the oldest every
        // tick; `julianday` reads both columns' text.
        let sql = format!(
            "UPDATE generations \
             SET poll_claimed_by = ?1, \
                 poll_claimed_until = strftime('%Y-%m-%d %H:%M:%f', 'now', ?2) \
             WHERE id IN ( \
                 SELECT id FROM generations \
                 WHERE status IN ('pending', 'processing') \
                   AND (poll_claimed_until IS NULL \
                        OR poll_claimed_until < strftime('%Y-%m-%d %H:%M:%f', 'now') \
                        OR poll_claimed_by = ?1) \
                 ORDER BY COALESCE(julianday(poll_claimed_until), julianday(created_at, ?2)) ASC, \
                          created_at ASC \
                 LIMIT ?3) \
             RETURNING {}",
            GENERATION_COLS
        );
        // A date-time modifier of fractional seconds, e.g. `+30.000 seconds`.
        let hold = format!("{:+.3} seconds", hold.num_milliseconds() as f64 / 1000.0);
        let rows = sqlx::query_as::<_, GenerationRow>(&sql)
            .bind(claimant)
            .bind(hold)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(generation_from_row).collect())
    }

    async fn release_poll_claims(&self, claimant: &str) -> Result<u64, sqlx::Error> {
        // Only active rows: a claim is never cleared when its row turns
        // terminal, so without the status clause every finished row this
        // process ever polled would be rewritten and counted.
        let freed = sqlx::query(
            "UPDATE generations SET poll_claimed_by = NULL, poll_claimed_until = NULL \
             WHERE poll_claimed_by = ? AND status IN ('pending', 'processing')",
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
        // On SQLite's own clock and in the claim's text format; the ids go in
        // as one JSON array.
        let hold = format!("{:+.3} seconds", hold.num_milliseconds() as f64 / 1000.0);
        let renewed = sqlx::query(
            "UPDATE generations SET poll_claimed_until = strftime('%Y-%m-%d %H:%M:%f', 'now', ?2) \
             WHERE poll_claimed_by = ?1 AND status IN ('pending', 'processing') \
               AND id IN (SELECT value FROM json_each(?3))",
        )
            .bind(claimant)
            .bind(hold)
            .bind(serde_json::Value::from(ids.to_vec()).to_string())
            .execute(&self.pool)
            .await?;
        Ok(renewed.rows_affected())
    }

    async fn note_poll_skip_warning(&self, id: &str, mark: i64) -> Result<bool, sqlx::Error> {
        // One guarded statement: of two polls at one mark, one changes the row.
        let noted = sqlx::query(
            "UPDATE generations SET poll_skip_warned_mark = ?2 \
             WHERE id = ?1 AND status IN ('pending', 'processing') \
               AND (poll_skip_warned_mark IS NULL OR poll_skip_warned_mark < ?2)",
        )
            .bind(id)
            .bind(mark)
            .execute(&self.pool)
            .await?;
        Ok(noted.rows_affected() > 0)
    }

    async fn clock_now(&self) -> Result<chrono::DateTime<chrono::Utc>, sqlx::Error> {
        Ok(self.now())
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
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, COALESCE(?, ?), COALESCE(?, ?), datetime('now'))
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
        // Re-stamp the latency of an ASYNC row when it settles.
        //
        // `/v1/stats` percentiles sample `latency_ms` over rows with
        // `status = 'completed'`. A video/3D submit logs `pending` with the
        // ENQUEUE time (a few ms) as its latency; once those rows settle they
        // join that window and drag p50/p95/p99 down to submit time, which is
        // not a latency anyone asked for. Recording the end-to-end time instead
        // keeps ONE meaning for the metric across every family: how long the
        // caller waited for a usable result. A row that was never `pending` —
        // every sync image row — keeps the latency its handler measured, so
        // `status` here is the pre-update value (SQL evaluates SET expressions
        // against the old row).
        sqlx::query(
            r#"
            UPDATE request_logs
            SET status = ?, error = ?,
                latency_ms = CASE WHEN status = 'pending'
                    THEN MAX(0, CAST((julianday('now') - julianday(created_at)) * 86400000.0 AS INTEGER))
                    ELSE latency_ms END
            WHERE id = ?
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
             WHERE id = ? AND cost_usd = ? AND cost_usd > 0 AND status IN ('failed', 'cancelled')",
        )
        .bind(id)
        .bind(cost_usd)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;
        if claimed {
            sqlx::query("UPDATE request_logs SET cost_usd = 0 WHERE id = ?").bind(id).execute(&self.pool).await?;
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
            LIMIT ? OFFSET ?
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

        // Build the WHERE clause conditionally using QueryBuilder.
        // We build count and data queries separately so we can reuse the filter logic.
        let mut count_qb: QueryBuilder<sqlx::Sqlite> =
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

        let mut data_qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(format!(
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
        let now = Utc::now();
        // Back-compat method: untenanted keys land in the default org/app so they
        // remain visible to tenant-scoped listing in single-tenant mode.
        let org_id = crate::api::middleware::DEFAULT_ORG_ID;
        let app_id = crate::api::middleware::DEFAULT_APP_ID;

        sqlx::query(
            r#"
            INSERT INTO api_keys (id, name, key_hash, key_prefix, created_at, is_active, token_quota, tokens_used, rpm_limit, scopes, webhook_url, org_id, app_id)
            VALUES (?, ?, ?, ?, ?, 1, ?, 0, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(id.to_string())
        .bind(name)
        .bind(key_hash)
        .bind(key_prefix)
        .bind(now)
        .bind(token_quota)
        .bind(rpm_limit.map(|v| v as i64))
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
        let sql = format!("SELECT {} FROM api_keys WHERE id = ?", API_KEY_COLS);
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
        let mut sets: Vec<&str> = Vec::new();
        if req.name.is_some() { sets.push("name = ?"); }
        if req.token_quota.is_some() { sets.push("token_quota = ?"); }
        if req.rpm_limit.is_some() { sets.push("rpm_limit = ?"); }
        if req.scopes.is_some() { sets.push("scopes = ?"); }
        if req.webhook_url.is_some() { sets.push("webhook_url = ?"); }
        if req.expires_at.is_some() { sets.push("expires_at = ?"); }
        if req.is_active.is_some() { sets.push("is_active = ?"); }

        if sets.is_empty() {
            return self.get_api_key(id).await;
        }

        let sql = format!("UPDATE api_keys SET {} WHERE id = ?", sets.join(", "));
        let mut q = sqlx::query(&sql);
        if let Some(ref v) = req.name { q = q.bind(v); }
        if let Some(v) = req.token_quota { q = q.bind(v); }
        if let Some(v) = req.rpm_limit { q = q.bind(v.map(i64::from)); }
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
            "SELECT {} FROM api_keys WHERE key_hash = ? AND is_active = 1",
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
             LEFT JOIN organization_members m ON m.user_id = k.owner_user_id AND m.org_id = COALESCE(k.org_id, ?) \
             WHERE k.key_hash = ? AND k.is_active = 1",
            key_cols_of_k()
        );
        let row = sqlx::query_as::<_, ApiKeyOwnerRow>(&sql)
            .bind(null_org)
            .bind(key_hash)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(api_key_owner_from_row))
    }

    async fn atomic_charge_tokens(
        &self,
        id: &Uuid,
        cost_usd: f64,
    ) -> Result<f64, sqlx::Error> {
        // Wrap the increment-and-read in a single transaction so concurrent
        // charges don't double-read the same intermediate state. The
        // RETURNING clause hands us the post-update value atomically.
        let mut tx = self.pool.begin().await?;
        let row: Option<(f64,)> = sqlx::query_as(
            "UPDATE api_keys SET tokens_used = tokens_used + ? \
             WHERE id = ? RETURNING tokens_used"
        )
        .bind(cost_usd)
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        row.map(|r| r.0).ok_or(sqlx::Error::RowNotFound)
    }

    async fn reserve_tokens(&self, id: &Uuid, amount: f64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE api_keys SET tokens_used = tokens_used + ? \
             WHERE id = ? AND (token_quota IS NULL OR tokens_used + ? <= token_quota)",
        )
        .bind(amount)
        .bind(id.to_string())
        .bind(amount)
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
            "SELECT {} FROM api_keys WHERE owner_user_id = ? ORDER BY created_at DESC",
            API_KEY_COLS
        );
        let rows = sqlx::query_as::<_, ApiKeyRow>(&sql)
            .bind(owner_user_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(api_key_from_row).collect())
    }

    async fn set_api_key_owner(&self, id: &Uuid, owner_user_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE api_keys SET owner_user_id = ? WHERE id = ?")
            .bind(owner_user_id)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn revoke_api_key(&self, id: &Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("UPDATE api_keys SET is_active = 0 WHERE id = ?")
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
            "UPDATE api_keys SET public_id = ?, key_hash = ?, key_prefix = ?, \
             owner_user_id = COALESCE(?, owner_user_id) WHERE id = ?",
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
                COALESCE(AVG(latency_ms), 0.0) as avg_latency
            FROM request_logs
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        let model_stats = sqlx::query_as::<_, ModelUsageRow>(
            r#"
            SELECT model, COUNT(*) as requests, COALESCE(SUM(cost_usd), 0.0) as cost_usd, COALESCE(AVG(latency_ms), 0.0) as avg_latency_ms
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
                   COALESCE(AVG(latency_ms), 0.0) as avg_latency_ms
            FROM request_logs
            GROUP BY provider
            ORDER BY requests DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let rpm: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM request_logs WHERE created_at > datetime('now', '-1 minutes')",
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
        // Shared tenant predicate appended to each aggregate query, narrowed to
        // requests whose metadata holds every pair asked for (L17).
        let mut tenant_clause = String::from(if app_id.is_some() {
            "org_id = ? AND app_id = ?"
        } else {
            "org_id = ?"
        });
        for _ in metadata {
            tenant_clause.push_str(" AND json_extract(metadata, ?) = ?");
        }
        // Helper to bind the tenant params consistently.
        macro_rules! bind_tenant {
            ($q:expr) => {{
                let mut q = $q.bind(org_id);
                if let Some(a) = app_id {
                    q = q.bind(a);
                }
                for (k, v) in metadata {
                    q = q.bind(metadata_path(k)).bind(v);
                }
                q
            }};
        }

        let totals_sql = format!(
            "SELECT COUNT(*) as total, \
             COALESCE(SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END), 0) as success, \
             COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0) as failed, \
             COALESCE(SUM(cost_usd), 0.0) as total_cost, \
             COALESCE(AVG(latency_ms), 0.0) as avg_latency \
             FROM request_logs WHERE {tenant_clause}"
        );
        let totals: (i64, i64, i64, f64, f64) =
            bind_tenant!(sqlx::query_as::<_, (i64, i64, i64, f64, f64)>(&totals_sql))
                .fetch_one(&self.pool)
                .await?;

        let model_sql = format!(
            "SELECT model, COUNT(*) as requests, COALESCE(SUM(cost_usd), 0.0) as cost_usd, \
             COALESCE(AVG(latency_ms), 0.0) as avg_latency_ms \
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
             COALESCE(AVG(latency_ms), 0.0) as avg_latency_ms \
             FROM request_logs WHERE {tenant_clause} \
             GROUP BY provider ORDER BY requests DESC"
        );
        let provider_stats = bind_tenant!(sqlx::query_as::<_, ProviderUsageRow>(&provider_sql))
            .fetch_all(&self.pool)
            .await?;

        let rpm_sql = format!(
            "SELECT COUNT(*) FROM request_logs \
             WHERE {tenant_clause} AND created_at > datetime('now', '-1 minutes')"
        );
        let rpm: (i64,) = bind_tenant!(sqlx::query_as::<_, (i64,)>(&rpm_sql))
            .fetch_one(&self.pool)
            .await?;

        // Reuse the global percentiles window (tenant-specific percentiles not required).
        let percentiles = self.latency_percentiles(60).await?;
        Ok(build_proxy_stats(totals, model_stats, provider_stats, rpm.0, percentiles))
    }

    async fn latency_percentiles(&self, since_minutes: i64) -> Result<LatencyPercentiles, sqlx::Error> {
        let modifier = format!("-{} minutes", since_minutes);
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT latency_ms FROM request_logs \
             WHERE created_at > datetime('now', ?) AND status = 'completed' \
             ORDER BY latency_ms \
             LIMIT 10000",
        )
        .bind(&modifier)
        .fetch_all(&self.pool)
        .await?;

        Ok(compute_percentiles(rows, since_minutes))
    }

    async fn insert_audit_log(&self, entry: &AuditLogEntry) -> Result<(), sqlx::Error> {
        // `org_id` as the server filed it (`api::audit::audit_org`): NULL is no org.
        sqlx::query(
            r#"
            INSERT INTO audit_log
                (id, actor_key_id, actor_label, action, target_type, target_id,
                 before_json, after_json, org_id, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
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
        let mut qb = QueryBuilder::<sqlx::Sqlite>::new(
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

        let mut count_qb: QueryBuilder<sqlx::Sqlite> =
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

        let mut data_qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
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
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'), ?11,
                    COALESCE((SELECT org_id FROM webhook_outbox WHERE id = ?11), (SELECT org_id FROM generations WHERE id = ?3)),
                    COALESCE((SELECT app_id FROM webhook_outbox WHERE id = ?11), (SELECT app_id FROM generations WHERE id = ?3)))
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
            "SELECT COUNT(*) FROM webhook_deliveries WHERE key_id = ?"
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
             WHERE d.key_id = ? \
             ORDER BY d.created_at DESC LIMIT ? OFFSET ?"
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
    // Times are written and compared as SQLite's own strftime millisecond
    // text, as the poll claims are: never a time bound from the app.

    fn keeps_webhook_outbox(&self) -> bool {
        true
    }

    async fn webhook_outbox_of(&self, generation_id: &str) -> Result<Option<crate::db::OutboxRecord>, sqlx::Error> {
        let sql = format!("SELECT {WEBHOOK_OUTBOX_COLS} FROM webhook_outbox WHERE generation_id = ?");
        let row = sqlx::query_as::<_, WebhookOutboxRow>(&sql).bind(generation_id).fetch_optional(&self.pool).await?;
        Ok(row.map(webhook_outbox_from_row))
    }

    async fn claim_webhook_delivery_of(
        &self,
        generation_id: &str,
        claimant: &str,
        hold: chrono::Duration,
    ) -> Result<Option<crate::db::OutboxDelivery>, sqlx::Error> {
        let row = sqlx::query_as::<_, OutboxClaimRow>(
            "UPDATE webhook_outbox \
             SET claimed_by = ?2, claimed_until = strftime('%Y-%m-%d %H:%M:%f', 'now', ?3) \
             WHERE generation_id = ?1 AND status = 'pending' AND attempts = 0 \
               AND (claimed_until IS NULL OR claimed_until < strftime('%Y-%m-%d %H:%M:%f', 'now')) \
             RETURNING id, generation_id, key_id, org_id, attempts, redelivered_at IS NOT NULL AS redelivered",
        )
        .bind(generation_id)
        .bind(claimant)
        .bind(sqlite_seconds(hold))
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
        // One statement, so SQLite's single writer makes the claim atomic.
        // The due deliveries nobody holds are ranked within their org, the
        // ones `claimant` holds already counted, so it takes at most `per_org`
        // of one org; then the longest due go first.
        let rows = sqlx::query_as::<_, OutboxClaimRow>(
            "UPDATE webhook_outbox \
             SET claimed_by = ?1, claimed_until = strftime('%Y-%m-%d %H:%M:%f', 'now', ?2) \
             WHERE id IN ( \
                 WITH held AS ( \
                     SELECT COALESCE(org_id, '') AS org, COUNT(*) AS n FROM webhook_outbox \
                     WHERE claimed_by = ?1 AND status = 'pending' AND claimed_until >= strftime('%Y-%m-%d %H:%M:%f', 'now') \
                     GROUP BY COALESCE(org_id, '') \
                 ), due AS ( \
                     SELECT o.id, o.next_attempt_at, o.created_at, \
                            ROW_NUMBER() OVER (PARTITION BY COALESCE(o.org_id, '') ORDER BY o.next_attempt_at, o.created_at, o.id) \
                              + COALESCE((SELECT h.n FROM held h WHERE h.org = COALESCE(o.org_id, '')), 0) AS place \
                     FROM webhook_outbox o \
                     WHERE o.status = 'pending' \
                       AND o.next_attempt_at <= strftime('%Y-%m-%d %H:%M:%f', 'now') \
                       AND (o.claimed_until IS NULL OR o.claimed_until < strftime('%Y-%m-%d %H:%M:%f', 'now')) \
                 ) \
                 SELECT id FROM due WHERE place <= ?4 ORDER BY next_attempt_at, created_at, id LIMIT ?3) \
             RETURNING id, generation_id, key_id, org_id, attempts, redelivered_at IS NOT NULL AS redelivered",
        )
        .bind(claimant)
        .bind(sqlite_seconds(hold))
        .bind(limit as i64)
        .bind(per_org as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn webhook_delivery_due(&self) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM webhook_outbox \
             WHERE status = 'pending' AND next_attempt_at <= strftime('%Y-%m-%d %H:%M:%f', 'now') \
               AND (claimed_until IS NULL OR claimed_until < strftime('%Y-%m-%d %H:%M:%f', 'now')))",
        )
        .fetch_one(&self.pool)
        .await
    }

    async fn webhook_answered_for(&self, generation_id: &str) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM webhook_deliveries WHERE generation_id = ? AND success = 1)")
            .bind(generation_id)
            .fetch_one(&self.pool)
            .await
    }

    async fn renew_webhook_claims(&self, claimant: &str, ids: &[String], hold: chrono::Duration) -> Result<u64, sqlx::Error> {
        if ids.is_empty() {
            return Ok(0);
        }
        let renewed = sqlx::query(
            "UPDATE webhook_outbox SET claimed_until = strftime('%Y-%m-%d %H:%M:%f', 'now', ?2) \
             WHERE claimed_by = ?1 AND status = 'pending' AND id IN (SELECT value FROM json_each(?3))",
        )
        .bind(claimant)
        .bind(sqlite_seconds(hold))
        .bind(serde_json::Value::from(ids.to_vec()).to_string())
        .execute(&self.pool)
        .await?;
        Ok(renewed.rows_affected())
    }

    async fn release_webhook_claims(&self, claimant: &str, keep: &[String]) -> Result<u64, sqlx::Error> {
        let freed = sqlx::query(
            "UPDATE webhook_outbox SET claimed_by = NULL, claimed_until = NULL \
             WHERE claimed_by = ?1 AND status = 'pending' AND id NOT IN (SELECT value FROM json_each(?2))",
        )
        .bind(claimant)
        .bind(serde_json::Value::from(keep.to_vec()).to_string())
        .execute(&self.pool)
        .await?;
        Ok(freed.rows_affected())
    }

    async fn finish_webhook_attempt(&self, id: &str, claimant: &str, attempt: &crate::db::OutboxAttempt<'_>) -> Result<bool, sqlx::Error> {
        let (status, after) = (attempt.outcome.status(), attempt.outcome.retry_after());
        let done = sqlx::query(
            "UPDATE webhook_outbox \
             SET status = ?3, attempts = ?4, \
                 next_attempt_at = CASE WHEN ?5 IS NULL THEN next_attempt_at ELSE strftime('%Y-%m-%d %H:%M:%f', 'now', ?5) END, \
                 claimed_by = NULL, claimed_until = NULL, \
                 last_status_code = ?6, last_error = ?7, payload_json = COALESCE(?8, payload_json), \
                 finished_at = CASE WHEN ?3 = 'pending' THEN NULL ELSE strftime('%Y-%m-%d %H:%M:%f', 'now') END \
             WHERE id = ?1 AND claimed_by = ?2 AND status = 'pending'",
        )
        .bind(id)
        .bind(claimant)
        .bind(status)
        .bind(attempt.attempts)
        .bind(after.map(sqlite_seconds))
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
             SET status = 'pending', attempts = 0, next_attempt_at = strftime('%Y-%m-%d %H:%M:%f', 'now'), \
                 claimed_by = NULL, claimed_until = NULL, last_status_code = NULL, last_error = NULL, finished_at = NULL, \
                 redelivered_at = strftime('%Y-%m-%d %H:%M:%f', 'now') \
             WHERE id = ?1 AND key_id = ?2 AND status IN ('delivered', 'failed') \
               AND (redelivered_at IS NULL OR redelivered_at <= strftime('%Y-%m-%d %H:%M:%f', 'now', ?3)) \
             RETURNING {WEBHOOK_OUTBOX_COLS}"
        );
        let redelivered = sqlx::query_as::<_, WebhookOutboxRow>(&sql)
            .bind(delivery_id)
            .bind(key_id)
            .bind(sqlite_seconds(-cooldown))
            .fetch_optional(&self.pool)
            .await?;
        if let Some(row) = redelivered {
            return Ok(crate::db::Redelivery::Redelivered(webhook_outbox_from_row(row)));
        }
        let sql = format!("SELECT {WEBHOOK_OUTBOX_COLS} FROM webhook_outbox WHERE id = ? AND key_id = ?");
        let row = sqlx::query_as::<_, WebhookOutboxRow>(&sql).bind(delivery_id).bind(key_id).fetch_optional(&self.pool).await?;
        Ok(crate::db::trait_def::redelivery_refused(row.map(webhook_outbox_from_row), cooldown))
    }

    async fn purge_webhook_log(&self, days: u32, limit: u32) -> Result<crate::db::WebhookLogPurged, sqlx::Error> {
        let before = format!("-{days} days");
        // Each by its index: a delivery's end and an attempt's time are
        // written and compared in one text form each.
        let deliveries = sqlx::query(
            "DELETE FROM webhook_outbox WHERE id IN ( \
                 SELECT id FROM webhook_outbox \
                 WHERE finished_at < strftime('%Y-%m-%d %H:%M:%f', 'now', ?1) AND status IN ('delivered', 'failed') LIMIT ?2)",
        )
        .bind(&before)
        .bind(i64::from(limit))
        .execute(&self.pool)
        .await?
        .rows_affected();
        let attempts = sqlx::query(
            "DELETE FROM webhook_deliveries WHERE id IN ( \
                 SELECT id FROM webhook_deliveries WHERE created_at < datetime('now', ?1) LIMIT ?2)",
        )
        .bind(&before)
        .bind(i64::from(limit))
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(crate::db::WebhookLogPurged { deliveries, attempts })
    }

    // Both tables' `created_at` is `datetime('now')` text, so a cutoff in
    // that form compares by the `created_at` index (`request_log_time`).

    async fn purge_request_artifacts(&self, before: chrono::DateTime<chrono::Utc>, limit: u32) -> Result<u64, sqlx::Error> {
        let done = sqlx::query(
            "DELETE FROM request_artifacts WHERE request_id IN ( \
                 SELECT request_id FROM request_artifacts WHERE created_at < ?1 ORDER BY created_at LIMIT ?2)",
        )
        .bind(request_log_time(before))
        .bind(i64::from(limit))
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected())
    }

    async fn purge_request_logs(&self, before: chrono::DateTime<chrono::Utc>, limit: u32) -> Result<u64, sqlx::Error> {
        // The rows first, then the tags of exactly those rows, in one
        // transaction: SQLite has no foreign key to take the tags, and a tag
        // written later finds no row (`attribute_request_log`).
        let mut tx = self.pool.begin().await?;
        let ids: Vec<String> = sqlx::query_scalar(
            "DELETE FROM request_logs WHERE id IN ( \
                 SELECT id FROM request_logs WHERE created_at < ?1 ORDER BY created_at LIMIT ?2) RETURNING id",
        )
        .bind(request_log_time(before))
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        if !ids.is_empty() {
            let ids_json = serde_json::to_string(&ids).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
            sqlx::query("DELETE FROM request_log_tags WHERE request_id IN (SELECT value FROM json_each(?1))")
                .bind(ids_json)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(ids.len() as u64)
    }

    fn single_writer(&self) -> bool {
        true
    }

    // ─── A deleted org's or app's files (migration 031) ──────────────────
    // Times are SQLite's own strftime millisecond text, as the outbox's.

    async fn claim_tenant_file_deletions(
        &self,
        of: Option<(&str, &str)>,
        limit: u32,
        hold: chrono::Duration,
    ) -> Result<Vec<crate::db::TenantFileDeletion>, sqlx::Error> {
        // One statement, so SQLite's single writer makes the claim atomic.
        let (scope, tenant) = of.unzip();
        let rows = sqlx::query_as::<_, TenantFileDeletionRow>(
            "UPDATE tenant_file_deletions \
             SET attempts = attempts + 1, next_attempt_at = strftime('%Y-%m-%d %H:%M:%f', 'now', ?3) \
             WHERE id IN ( \
                 SELECT id FROM tenant_file_deletions \
                 WHERE next_attempt_at <= strftime('%Y-%m-%d %H:%M:%f', 'now') \
                   AND (?1 IS NULL OR (scope = ?1 AND tenant_id = ?2)) \
                 ORDER BY next_attempt_at, id LIMIT ?4) \
             RETURNING id, scope, tenant_id, generation_id, url, attempts",
        )
        .bind(scope)
        .bind(tenant)
        .bind(sqlite_seconds(hold))
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn finish_tenant_file_deletion(&self, id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM tenant_file_deletions WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(done.rows_affected() > 0)
    }

    async fn retry_tenant_file_deletion(&self, id: i64, attempts: i32, after: chrono::Duration, error: &str) -> Result<bool, sqlx::Error> {
        let done = sqlx::query(
            "UPDATE tenant_file_deletions SET next_attempt_at = strftime('%Y-%m-%d %H:%M:%f', 'now', ?3), last_error = ?4 \
             WHERE id = ?1 AND attempts = ?2",
        )
        .bind(id)
        .bind(attempts)
        .bind(sqlite_seconds(after))
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
            "DELETE FROM request_artifacts WHERE request_id = ?",
            "DELETE FROM request_log_tags WHERE request_id = ?",
            "DELETE FROM request_logs WHERE id = ?",
        ] {
            sqlx::query(stmt).bind(request_id).execute(&mut *tx).await?;
        }
        if let Some(generation_id) = generation_id {
            sqlx::query("DELETE FROM generations WHERE id = ?").bind(generation_id).execute(&mut *tx).await?;
        }
        tx.commit().await
    }

    async fn mark_cache_hit(&self, request_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE request_logs SET cache_hit = 1 WHERE id = ?").bind(request_id).execute(&self.pool).await?;
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
            INSERT OR REPLACE INTO request_artifacts
                (request_id, media_type, prompt, negative_prompt, params_json, refs_meta_json,
                 output_kind, output_value, output_mime, output_truncated, error_message, created_at,
                 org_id, app_id)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'),
                    COALESCE(?, ?), COALESCE(?, ?))
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
        .bind(a.output_truncated as i32)
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
        let row = sqlx::query_as::<_, RequestArtifactRow>(
            "SELECT request_id, media_type, prompt, negative_prompt, params_json, refs_meta_json, \
             output_kind, output_value, output_mime, output_truncated, error_message, created_at, \
             org_id, app_id \
             FROM request_artifacts WHERE request_id = ?"
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
             FROM users WHERE email = ?"
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
             FROM users WHERE {} = ?",
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
        let mut sets: Vec<&str> = Vec::new();
        if role.is_some() { sets.push("role = ?"); }
        if is_active.is_some() { sets.push("is_active = ?"); }
        // Reactivating moves the sign-in epoch on (review round 5, finding
        // 2): a session stamped while the account was deactivated doesn't
        // come back with it. (The right side reads the row before the write.)
        if is_active == Some(true) { sets.push("sign_in_epoch = CASE WHEN is_active THEN sign_in_epoch ELSE sign_in_epoch + 1 END"); }
        if password_hash.is_some() { sets.push("password_hash = ?"); }
        sets.push("updated_at = datetime('now')");

        let sql = format!("UPDATE users SET {} WHERE id = ?", sets.join(", "));
        let mut q = sqlx::query(&sql);
        if let Some(r) = role { q = q.bind(r.as_str()); }
        if let Some(a) = is_active { q = q.bind(a as i32); }
        if let Some(ph) = password_hash { q = q.bind(ph); }
        q = q.bind(id);

        let mut tx = self.pool.begin().await?;
        // The role in `member_org` too (review round 7, finding 3), where the
        // single-tenant permissions come from, in this transaction: a failure
        // leaves both roles as they were. The membership first, as on
        // Postgres. Never an Owner's: an account made the Owner since the
        // caller checked stays the Owner, and this fails with nothing written.
        if let (Some(r), Some(org)) = (role, member_org) {
            let moved = sqlx::query("UPDATE organization_members SET role = ? WHERE org_id = ? AND user_id = ? AND role <> 'owner'")
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
        // same transaction (review round 6, finding 2).
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
        sqlx::query("UPDATE users SET last_login_at = datetime('now'), updated_at = datetime('now') WHERE id = ?")
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
        // The claim on the one `first_owner` row (migration 034) is the
        // transaction's first statement and a write, so it takes SQLite's
        // write lock: another first signup waits for this one to end, then
        // finds the row. Returning before the commit rolls everything back.
        let claimed = sqlx::query("INSERT INTO first_owner (id, user_id) VALUES (1, ?) ON CONFLICT (id) DO NOTHING")
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
             VALUES (?, ?, ?, datetime('now'))",
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
        let sql = format!("UPDATE users SET {} = ?, updated_at = datetime('now') WHERE id = ?", col);
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
        sqlx::query("UPDATE users SET role = 'admin', updated_at = datetime('now') WHERE role = 'owner'")
            .execute(&mut *tx)
            .await?;
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE users SET role = 'owner', updated_at = datetime('now') WHERE id = ?")
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
             VALUES (?, ?, ?, ?, ?, ?, ?, COALESCE((SELECT sign_in_epoch FROM users WHERE id = ?), 0))"
        )
        .bind(crate::auth::tokens::token_hash(&s.id))
        .bind(&s.user_id)
        .bind(s.created_at)
        .bind(s.expires_at)
        .bind(&s.ip)
        .bind(&s.user_agent)
        .bind(&s.csrf_token)
        .bind(&s.user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn create_session_at_epoch(&self, s: &Session, epoch: i64) -> Result<(), sqlx::Error> {
        // Kept by its token's hash (migration 044).
        sqlx::query(
            "INSERT INTO sessions (id, user_id, created_at, expires_at, ip, user_agent, csrf_token, sign_in_epoch) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
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
        sqlx::query_scalar("SELECT sign_in_epoch FROM users WHERE id = ?").bind(user_id).fetch_optional(&self.pool).await
    }

    async fn session_epoch(&self, id: &str) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar("SELECT sign_in_epoch FROM sessions WHERE id = ?").bind(id).fetch_optional(&self.pool).await
    }

    async fn change_own_password(&self, user_id: &str, password_hash: &str, verified_hash: &str, keep_session: &str) -> Result<crate::db::PasswordChange, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Only while nothing moved since the current password was checked
        // (review round 5, finding 1): the account still holds the hash it was
        // checked against, is active, and is at the sending session's epoch,
        // all in this one statement. A reset, a deactivation or another
        // change that committed meanwhile makes it write nothing.
        let changed = sqlx::query(
            "UPDATE users SET password_hash = ?, updated_at = datetime('now') \
             WHERE id = ? AND password_hash = ? AND is_active \
             AND sign_in_epoch = (SELECT sign_in_epoch FROM sessions WHERE id = ? AND user_id = ?)",
        )
        .bind(password_hash)
        .bind(user_id)
        .bind(verified_hash)
        .bind(keep_session)
        .bind(user_id)
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
             FROM sessions WHERE id = ?"
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
             WHERE s.id = ? AND (u.id IS NULL OR u.sign_in_epoch = s.sign_in_epoch)"
        );
        let row = sqlx::query_as::<_, SessionUserRow>(&sql).bind(crate::auth::tokens::token_hash(cookie)).fetch_optional(&self.pool).await?;
        Ok(row.map(session_user_from_row))
    }

    async fn delete_session(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_user_sessions(&self, user_id: &str, except_id: Option<&str>) -> Result<u64, sqlx::Error> {
        let result = if let Some(exc) = except_id {
            sqlx::query("DELETE FROM sessions WHERE user_id = ? AND id != ?")
                .bind(user_id)
                .bind(exc)
                .execute(&self.pool)
                .await?
        } else {
            sqlx::query("DELETE FROM sessions WHERE user_id = ?")
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
             WHERE s.user_id = ? AND s.sign_in_epoch = u.sign_in_epoch ORDER BY s.created_at DESC"
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
        sqlx::query("UPDATE sessions SET expires_at = ? WHERE id = ?")
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
        let (guard, sign_in) = live_sign_in_guard(from);
        let sql = format!(
            "INSERT INTO invitations (id, email, role, token, token_hash, invited_by, org_id, expires_at, used_at, created_at) \
             SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?{guard}"
        );
        let mut q = sqlx::query(&sql)
            .bind(&inv.id)
            .bind(&inv.email)
            .bind(inv.role.as_str())
            .bind(&hash)
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
             FROM invitations WHERE token_hash = ? OR (token_hash IS NULL AND token = ?)"
        )
        .bind(crate::auth::tokens::token_hash(token))
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(invitation_from_row))
    }

    async fn mark_invitation_used(&self, token: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE invitations SET used_at = datetime('now') \
             WHERE (token_hash = ? OR (token_hash IS NULL AND token = ?)) AND used_at IS NULL",
        )
        .bind(crate::auth::tokens::token_hash(token))
        .bind(token)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    async fn invitation_accepted(&self, org_id: &str, id: &str) -> Result<bool, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM invitations WHERE id = ? AND org_id = ? AND used_at IS NOT NULL")
            .bind(id)
            .bind(org_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    async fn accept_invitation(&self, token: &str, new_user: Option<&User>, user_id: &str, org_id: &str, role: Role) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Consumed first, as the transaction's first write: a revoke or a
        // second acceptance either ran before (nothing to consume, nothing
        // written) or waits for this one.
        let consumed = sqlx::query(
            "UPDATE invitations SET used_at = datetime('now') \
             WHERE (token_hash = ? OR (token_hash IS NULL AND token = ?)) AND used_at IS NULL",
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
            "INSERT INTO organization_members (org_id, user_id, role, created_at) VALUES (?, ?, ?, datetime('now')) \
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
        sqlx::query("DELETE FROM invitations WHERE id = ?")
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
             FROM invitations WHERE org_id = ? AND used_at IS NULL ORDER BY created_at DESC"
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(invitation_from_row).collect())
    }

    async fn revoke_invitation(&self, org_id: &str, id: &str) -> Result<Option<Invitation>, sqlx::Error> {
        let row = sqlx::query_as::<_, InvitationRow>(
            "DELETE FROM invitations WHERE id = ? AND org_id = ? AND used_at IS NULL \
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
        // Only the token's hash is kept (migration 041). The link goes in
        // first, only while the issuer's sign-in is live, checked in the same
        // statement (review round 6, finding 2); then it voids the account's
        // older unused ones. A refused link changes nothing.
        let hash = crate::auth::tokens::token_hash(&r.token);
        let (guard, sign_in) = live_sign_in_guard(from);
        let sql = format!(
            "INSERT INTO password_resets (token, user_id, expires_at, used_at, created_at, issued_by) \
             SELECT ?, ?, ?, ?, ?, ?{guard}"
        );
        let mut tx = self.pool.begin().await?;
        let mut q = sqlx::query(&sql)
            .bind(&hash)
            .bind(&r.user_id)
            .bind(r.expires_at)
            .bind(r.used_at)
            .bind(r.created_at)
            .bind(&r.issued_by);
        if let Some(id) = sign_in {
            q = q.bind(id);
        }
        if q.execute(&mut *tx).await?.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query("DELETE FROM password_resets WHERE user_id = ? AND used_at IS NULL AND token <> ?")
            .bind(&r.user_id)
            .bind(&hash)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn get_password_reset(&self, token: &str) -> Result<Option<PasswordReset>, sqlx::Error> {
        let row = sqlx::query_as::<_, PasswordResetRow>(
            "SELECT token, user_id, expires_at, used_at, created_at, issued_by \
             FROM password_resets WHERE token = ?"
        )
        .bind(crate::auth::tokens::token_hash(token))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(password_reset_from_row))
    }

    async fn confirm_password_reset(&self, token: &str, password_hash: &str, mode: crate::config::Mode) -> Result<crate::db::ResetConfirm, sqlx::Error> {
        use crate::db::ResetConfirm;
        let mut tx = self.pool.begin().await?;
        // The single-use CAS first; returning before the commit undoes it.
        // From here the transaction holds SQLite's writer lock, so no role,
        // membership or active write lands until it ends.
        let claimed: Option<(String, Option<String>)> = sqlx::query_as(
            "UPDATE password_resets SET used_at = datetime('now') WHERE token = ? AND used_at IS NULL RETURNING user_id, issued_by",
        )
        .bind(crate::auth::tokens::token_hash(token))
        .fetch_optional(&mut *tx)
        .await?;
        let Some((user_id, issued_by)) = claimed else { return Ok(ResetConfirm::Unusable) };
        let Some(active): Option<bool> = sqlx::query_scalar("SELECT is_active FROM users WHERE id = ?").bind(&user_id).fetch_optional(&mut *tx).await? else {
            return Ok(ResetConfirm::Unusable);
        };
        // The issuer may still issue this link, now (review round 4,
        // findings 4 and 5); the master key's links carry no issuer.
        if let Some(issuer) = issued_by {
            let issuer_active: Option<bool> = sqlx::query_scalar("SELECT is_active FROM users WHERE id = ?").bind(&issuer).fetch_optional(&mut *tx).await?;
            let roles_of = |rows: Vec<(String, String)>| rows.into_iter().filter_map(|(org, role)| Role::parse(&role).map(|r| (org, r))).collect::<Vec<_>>();
            let issuer_roles: Vec<(String, String)> = sqlx::query_as("SELECT org_id, role FROM organization_members WHERE user_id = ?").bind(&issuer).fetch_all(&mut *tx).await?;
            let account_roles: Vec<(String, String)> = sqlx::query_as("SELECT org_id, role FROM organization_members WHERE user_id = ?").bind(&user_id).fetch_all(&mut *tx).await?;
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
        sqlx::query("UPDATE users SET password_hash = ?, is_active = 1, updated_at = datetime('now') WHERE id = ?")
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
            "UPDATE password_resets SET used_at = datetime('now') WHERE token = ? AND used_at IS NULL",
        )
        .bind(crate::auth::tokens::token_hash(token))
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    // ─── Login Attempts ─────────────────────────────────────────────────

    async fn record_login_attempt(&self, email: &str, success: bool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO login_attempts (email, attempted_at, success) VALUES (?, ?, ?)"
        )
        .bind(email)
        .bind(chrono::Utc::now())
        .bind(success as i32)
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
             WHERE email = ? AND success = 0 AND attempted_at >= ? \
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
             VALUES (?, ?, ?, datetime('now'))",
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
            "SELECT id, name, slug, plan, status, created_at, updated_at FROM organizations WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(organization_from_row))
    }

    async fn get_org_by_slug(&self, slug: &str) -> Result<Option<Organization>, sqlx::Error> {
        let row = sqlx::query_as::<_, OrganizationRow>(
            "SELECT id, name, slug, plan, status, created_at, updated_at FROM organizations WHERE slug = ?",
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
             WHERE m.user_id = ? ORDER BY o.created_at ASC",
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
                "UPDATE organizations SET name = ?, updated_at = datetime('now') WHERE id = ?",
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
        // Cascade in one transaction, in FK-dependency order. Several tenant
        // tables reference organizations/applications WITHOUT ON DELETE CASCADE
        // (and on sqlite the ADD COLUMN tenant refs carry no FK at all), so a
        // bare `DELETE FROM organizations` fails the FK check once an org has its
        // auto-created default app + owner membership. Delete children first.
        let mut tx = self.pool.begin().await?;
        // org-scoped rows (generations references api_keys → before api_keys).
        for stmt in [
            "DELETE FROM webhook_deliveries WHERE org_id = ?",
            "DELETE FROM webhook_outbox WHERE org_id = ?",
            "DELETE FROM request_artifacts WHERE org_id = ?",
            // Its requests' spend tags, before the rows they hang off.
            "DELETE FROM request_log_tags WHERE request_id IN (SELECT id FROM request_logs WHERE org_id = ?)",
            "DELETE FROM request_logs WHERE org_id = ?",
        ] {
            sqlx::query(stmt).bind(id).execute(&mut *tx).await?;
        }
        // The files its generations kept, recorded before their rows go
        // (migration 031): deleted from litegen's store after the commit,
        // until they're gone. The deletes above hold the write lock.
        record_tenant_files(&mut tx, "org", id).await?;
        for stmt in [
            "DELETE FROM generations WHERE org_id = ?",
            "DELETE FROM api_keys WHERE org_id = ?",
            "DELETE FROM audit_log WHERE org_id = ?",
            "DELETE FROM invitations WHERE org_id = ?",
            "DELETE FROM org_provider_credentials WHERE org_id = ?",
            "DELETE FROM org_allowed_models WHERE org_id = ?",
            // Migrations 042/043's triggers delete unused reset links here (see 043's header).
            "DELETE FROM organization_members WHERE org_id = ?",
        ] {
            sqlx::query(stmt).bind(id).execute(&mut *tx).await?;
        }
        // app-scoped rows for this org's apps, then the apps themselves.
        for stmt in [
            "DELETE FROM app_model_access_ids WHERE app_id IN (SELECT id FROM applications WHERE org_id = ?)",
            "DELETE FROM app_model_access WHERE app_id IN (SELECT id FROM applications WHERE org_id = ?)",
            "DELETE FROM app_storage_credentials WHERE app_id IN (SELECT id FROM applications WHERE org_id = ?)",
            "DELETE FROM app_file_access WHERE app_id IN (SELECT id FROM applications WHERE org_id = ?)",
        ] {
            sqlx::query(stmt).bind(id).execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM applications WHERE org_id = ?").bind(id).execute(&mut *tx).await?;
        let result = sqlx::query("DELETE FROM organizations WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
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
             VALUES (?, ?, ?, datetime('now'))",
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
            "SELECT role FROM organization_members WHERE org_id = ? AND user_id = ?",
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
             WHERE m.org_id = ? ORDER BY m.created_at ASC",
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
        sqlx::query("UPDATE organization_members SET role = ? WHERE org_id = ? AND user_id = ?")
            .bind(role.as_str())
            .bind(org_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn remove_org_member(&self, org_id: &str, user_id: &str) -> Result<(), sqlx::Error> {
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("DELETE FROM organization_members WHERE org_id = ? AND user_id = ?")
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
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE organization_members SET role = 'admin' WHERE org_id = ? AND role = 'owner'")
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        let promoted =
            // Migrations 042/043's triggers delete unused reset links here (see 043's header).
            sqlx::query("UPDATE organization_members SET role = 'owner' WHERE org_id = ? AND user_id = ?")
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
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE organization_members SET role = 'admin' WHERE org_id = ? AND role = 'owner'")
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        let promoted =
            // Migrations 042/043's triggers delete unused reset links here (see 043's header).
            sqlx::query("UPDATE organization_members SET role = 'owner' WHERE org_id = ? AND user_id = ?")
                .bind(org_id)
                .bind(new_owner_user_id)
                .execute(&mut *tx)
                .await?;
        if promoted.rows_affected() == 0 {
            // Not a member: dropping the tx rolls the demotion back.
            return Err(sqlx::Error::RowNotFound);
        }
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE users SET role = 'admin', updated_at = datetime('now') WHERE role = 'owner' AND id <> ?")
            .bind(new_owner_user_id)
            .execute(&mut *tx)
            .await?;
        // Migrations 042/043's triggers delete unused reset links here (see 043's header).
        sqlx::query("UPDATE users SET role = 'owner', updated_at = datetime('now') WHERE id = ?")
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
            "SELECT id, org_id, name, slug, status, created_at, updated_at FROM applications WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(application_from_row))
    }

    async fn list_apps_for_org(&self, org_id: &str) -> Result<Vec<Application>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ApplicationRow>(
            "SELECT id, org_id, name, slug, status, created_at, updated_at \
             FROM applications WHERE org_id = ? ORDER BY created_at ASC",
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
                "UPDATE applications SET name = ?, updated_at = datetime('now') WHERE id = ?",
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
        // Cascade in one transaction, in FK-dependency order. On Postgres the
        // app_id columns reference applications WITHOUT ON DELETE CASCADE, so a
        // bare `DELETE FROM applications` fails once the app has been used; on
        // sqlite (no FK) it instead orphans the children. Either way we delete
        // the app-scoped children first (generations references api_keys →
        // before api_keys), mirroring delete_organization.
        let mut tx = self.pool.begin().await?;
        for stmt in [
            "DELETE FROM webhook_deliveries WHERE app_id = ?",
            "DELETE FROM webhook_outbox WHERE app_id = ?",
            "DELETE FROM request_artifacts WHERE app_id = ?",
            // Its requests' spend tags, before the rows they hang off.
            "DELETE FROM request_log_tags WHERE request_id IN (SELECT id FROM request_logs WHERE app_id = ?)",
            "DELETE FROM request_logs WHERE app_id = ?",
        ] {
            sqlx::query(stmt).bind(id).execute(&mut *tx).await?;
        }
        // The files its generations kept, recorded before their rows go, as
        // delete_organization does.
        record_tenant_files(&mut tx, "app", id).await?;
        for stmt in [
            "DELETE FROM generations WHERE app_id = ?",
            "DELETE FROM api_keys WHERE app_id = ?",
            "DELETE FROM app_model_access_ids WHERE app_id = ?",
            "DELETE FROM app_model_access WHERE app_id = ?",
            "DELETE FROM app_storage_credentials WHERE app_id = ?",
            "DELETE FROM app_file_access WHERE app_id = ?",
        ] {
            sqlx::query(stmt).bind(id).execute(&mut *tx).await?;
        }
        let result = sqlx::query("DELETE FROM applications WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
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
             VALUES (?, ?, ?, ?, ?, ?, datetime('now'), datetime('now')) \
             ON CONFLICT (org_id, provider) DO UPDATE SET \
                ciphertext = excluded.ciphertext, nonce = excluded.nonce, \
                display_hint = excluded.display_hint, updated_at = datetime('now')",
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
            "SELECT ciphertext, nonce FROM org_provider_credentials WHERE org_id = ? AND provider = ?",
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
             WHERE org_id = ? ORDER BY provider ASC",
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
            sqlx::query("DELETE FROM org_provider_credentials WHERE org_id = ? AND provider = ?")
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
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), datetime('now')) \
             ON CONFLICT (app_id) DO UPDATE SET \
                backend = excluded.backend, \
                bucket_name = excluded.bucket_name, \
                region = excluded.region, \
                endpoint_url = excluded.endpoint_url, \
                custom_public_url = excluded.custom_public_url, \
                path_prefix = excluded.path_prefix, \
                access_key_id_hint = excluded.access_key_id_hint, \
                secret_ciphertext = excluded.secret_ciphertext, \
                secret_nonce = excluded.secret_nonce, \
                credential_mode = excluded.credential_mode, \
                role_arn = excluded.role_arn, \
                fallback_to_litegen_store = excluded.fallback_to_litegen_store, \
                updated_at = datetime('now')",
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
             FROM app_storage_credentials WHERE app_id = ?",
        )
        .bind(app_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn delete_app_storage(&self, app_id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM app_storage_credentials WHERE app_id = ?")
            .bind(app_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn get_app_file_access(&self, app_id: &str) -> Result<Option<crate::types::AppFileAccessRow>, sqlx::Error> {
        sqlx::query_as::<_, crate::types::AppFileAccessRow>(
            "SELECT app_id, mode, owner_scope, signed_url_ttl_seconds, keep_outputs, retention_days, signed_links, updated_at FROM app_file_access WHERE app_id = ?",
        )
        .bind(app_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn generation_file_access(&self, id: &str) -> Result<Option<(Option<String>, Option<crate::types::AppFileAccessRow>)>, sqlx::Error> {
        let sql = format!("SELECT {GENERATION_ACCESS_COLS} FROM generations g LEFT JOIN app_file_access a ON a.app_id = g.app_id WHERE g.id = ?");
        let row = sqlx::query_as::<_, GenerationAccessRow>(&sql).bind(id).fetch_optional(&self.pool).await?;
        Ok(row.map(generation_access_from_row))
    }

    async fn put_app_file_access(&self, input: &crate::types::AppFileAccessUpsert) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO app_file_access (app_id, mode, owner_scope, signed_url_ttl_seconds, keep_outputs, retention_days, signed_links, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now')) \
             ON CONFLICT (app_id) DO UPDATE SET \
                mode = excluded.mode, \
                owner_scope = excluded.owner_scope, \
                signed_url_ttl_seconds = excluded.signed_url_ttl_seconds, \
                keep_outputs = excluded.keep_outputs, \
                retention_days = excluded.retention_days, \
                signed_links = excluded.signed_links, \
                updated_at = datetime('now')",
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
                let sql = format!("SELECT {cols} FROM generations WHERE app_id = ? AND files_deleted_at IS NULL AND status IN ('completed', 'failed', 'cancelled') AND julianday(created_at) < julianday(?) ORDER BY files_delete_attempted_at ASC NULLS FIRST, created_at ASC LIMIT ?", cols = GENERATION_COLS);
                sqlx::query_as::<_, GenerationRow>(&sql).bind(app).bind(cutoff.to_rfc3339()).bind(limit).fetch_all(&self.pool).await?
            }
            None => {
                let sql = format!("SELECT {cols} FROM generations WHERE (app_id IS NULL OR app_id NOT IN (SELECT app_id FROM app_file_access WHERE retention_days IS NOT NULL)) AND files_deleted_at IS NULL AND status IN ('completed', 'failed', 'cancelled') AND julianday(created_at) < julianday(?) ORDER BY files_delete_attempted_at ASC NULLS FIRST, created_at ASC LIMIT ?", cols = GENERATION_COLS);
                sqlx::query_as::<_, GenerationRow>(&sql).bind(cutoff.to_rfc3339()).bind(limit).fetch_all(&self.pool).await?
            }
        };
        Ok(rows.into_iter().map(generation_from_row).collect())
    }

    async fn mark_files_deleted(&self, id: &str, at: chrono::DateTime<chrono::Utc>, reason: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE generations SET files_deleted_at = ?, files_deleted_reason = ? WHERE id = ? AND files_deleted_at IS NULL")
            .bind(at)
            .bind(reason)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn clear_artifact_output(&self, request_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE request_artifacts SET output_kind = 'deleted', output_value = NULL, output_truncated = 0 \
             WHERE request_id = ? AND output_kind IN ('b64', 'url')",
        )
        .bind(request_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn note_files_delete_failed(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE generations SET files_delete_attempted_at = strftime('%Y-%m-%d %H:%M:%f', 'now') WHERE id = ? AND files_deleted_at IS NULL")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn redact_generation_request(&self, id: &str) -> Result<(), sqlx::Error> {
        let before: Option<Option<String>> = sqlx::query_scalar("SELECT metadata FROM generations WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        if let Some(Some(text)) = before {
            if let Ok(mut meta) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(obj) = meta.as_object_mut() {
                    obj.remove("request");
                }
                // Only over what was read: a write in between keeps its own.
                sqlx::query("UPDATE generations SET metadata = ? WHERE id = ? AND metadata = ?")
                    .bind(meta.to_string())
                    .bind(id)
                    .bind(&text)
                    .execute(&self.pool)
                    .await?;
            }
        }
        sqlx::query("DELETE FROM request_artifacts WHERE request_id = ?").bind(id).execute(&self.pool).await?;
        // The artifacts of the cache hits it answered name it, and carry its
        // prompt and params: the same request.
        sqlx::query("DELETE FROM request_artifacts WHERE output_kind = 'generation' AND output_value = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn revoke_links(&self, scope: &str, scope_id: &str, at: chrono::DateTime<chrono::Utc>) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO link_revocations (scope, scope_id, revoked_at) VALUES (?, ?, ?) ON CONFLICT (scope, scope_id) DO UPDATE SET revoked_at = max(link_revocations.revoked_at, excluded.revoked_at)")
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
        let times: Vec<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar("SELECT revoked_at FROM link_revocations WHERE (scope = 'gateway' AND scope_id = '') OR (scope = 'app' AND scope_id = ?) OR (scope = 'generation' AND scope_id = ?)")
            .bind(app_id.unwrap_or(""))
            .bind(generation_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(times.into_iter().max())
    }

    async fn delete_app_file_access(&self, app_id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM app_file_access WHERE app_id = ?")
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
        sqlx::query("INSERT INTO server_secrets (name, value) VALUES (?, ?) ON CONFLICT (name) DO NOTHING")
            .bind(name)
            .bind(generate())
            .execute(&self.pool)
            .await?;
        sqlx::query_scalar("SELECT value FROM server_secrets WHERE name = ?")
            .bind(name)
            .fetch_one(&self.pool)
            .await
    }

    // ─── Model Allowlists ───────────────────────────────────────────────────────

    async fn get_org_allowed_models(&self, org_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT model_id FROM org_allowed_models WHERE org_id = ? ORDER BY model_id")
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
        sqlx::query("DELETE FROM org_allowed_models WHERE org_id = ?")
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        for model_id in models {
            sqlx::query("INSERT INTO org_allowed_models (org_id, model_id) VALUES (?, ?)")
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
            sqlx::query_as("SELECT mode FROM app_model_access WHERE app_id = ?")
                .bind(app_id)
                .fetch_optional(&self.pool)
                .await?;
        let mode = mode_row.map(|(m,)| m).unwrap_or_else(|| "all".to_string());
        let ids: Vec<(String,)> =
            sqlx::query_as("SELECT model_id FROM app_model_access_ids WHERE app_id = ? ORDER BY model_id")
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
            "INSERT INTO app_model_access (app_id, mode) VALUES (?, ?) \
             ON CONFLICT (app_id) DO UPDATE SET mode = excluded.mode",
        )
        .bind(app_id)
        .bind(mode)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM app_model_access_ids WHERE app_id = ?")
            .bind(app_id)
            .execute(&mut *tx)
            .await?;
        for model_id in models {
            sqlx::query("INSERT INTO app_model_access_ids (app_id, model_id) VALUES (?, ?)")
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
             VALUES (?, ?, ?, ?, ?, ?, ?, 1, 0, ?, ?, ?, ?, ?, datetime('now'))",
        )
        .bind(id.to_string())
        .bind(org_id)
        .bind(app_id)
        .bind(public_id)
        .bind(name)
        .bind(key_hash)
        .bind(key_prefix)
        .bind(token_quota)
        .bind(rpm_limit.map(|v| v as i64))
        .bind(scopes)
        .bind(webhook_url)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        self.get_api_key(&id).await?.ok_or(sqlx::Error::RowNotFound)
    }

    async fn list_api_keys_for_app(&self, app_id: &str) -> Result<Vec<ApiKey>, sqlx::Error> {
        let sql = format!(
            "SELECT {} FROM api_keys WHERE app_id = ? ORDER BY created_at DESC",
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
            "SELECT {} FROM generations WHERE org_id = ? AND (? IS NULL OR app_id = ?) \
             AND status IN ('pending', 'processing') ORDER BY created_at ASC",
            GENERATION_COLS
        );
        let rows = sqlx::query_as::<_, GenerationRow>(&sql)
            .bind(org_id)
            .bind(app_id)
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
                    "SELECT {} FROM generations WHERE org_id = ? AND app_id = ? \
                     ORDER BY created_at DESC LIMIT ? OFFSET ?",
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
                    "SELECT {} FROM generations WHERE org_id = ? \
                     ORDER BY created_at DESC LIMIT ? OFFSET ?",
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
                "SELECT COUNT(*) FROM generations WHERE org_id = ? AND app_id = ?",
            )
            .bind(org_id)
            .bind(app)
            .fetch_one(&self.pool)
            .await?,
            None => sqlx::query_as("SELECT COUNT(*) FROM generations WHERE org_id = ?")
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
                    "SELECT COUNT(*) FROM request_logs WHERE org_id = ? AND app_id = ?",
                )
                .bind(org_id)
                .bind(app)
                .fetch_one(&self.pool)
                .await?;
                let sql = format!(
                    "SELECT {COLS} FROM request_logs WHERE org_id = ? AND app_id = ? \
                     ORDER BY created_at DESC LIMIT ? OFFSET ?"
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
                    sqlx::query_as("SELECT COUNT(*) FROM request_logs WHERE org_id = ?")
                        .bind(org_id)
                        .fetch_one(&self.pool)
                        .await?;
                let sql = format!(
                    "SELECT {COLS} FROM request_logs WHERE org_id = ? \
                     ORDER BY created_at DESC LIMIT ? OFFSET ?"
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
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM audit_log WHERE org_id = ?")
            .bind(org_id)
            .fetch_one(&self.pool)
            .await?;
        let rows = sqlx::query_as::<_, AuditLogRow>(
            "SELECT id, actor_key_id, actor_label, action, target_type, target_id, \
             before_json, after_json, created_at, org_id FROM audit_log WHERE org_id = ? \
             ORDER BY created_at DESC LIMIT ? OFFSET ?",
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
        // SQLite stores created_at as fixed-width "YYYY-MM-DD HH:MM:SS" (via
        // datetime('now')), so binding the range as the same-format string makes
        // a lexicographic comparison equivalent to a chronological one.
        let fmt = |dt: chrono::DateTime<chrono::Utc>| dt.format("%Y-%m-%d %H:%M:%S").to_string();

        let mut cb = QueryBuilder::<sqlx::Sqlite>::new("SELECT COUNT(*) FROM request_logs WHERE org_id = ");
        cb.push_bind(org_id.to_string());
        if let Some(app) = app_id { cb.push(" AND app_id = ").push_bind(app.to_string()); }
        if let Some(m) = filter.model { cb.push(" AND model = ").push_bind(m.to_string()); }
        if let Some(p) = filter.provider { cb.push(" AND provider = ").push_bind(p.to_string()); }
        if let Some(s) = filter.status { cb.push(" AND status = ").push_bind(s.to_string()); }
        if let Some(f) = filter.from { cb.push(" AND created_at >= ").push_bind(fmt(f)); }
        if let Some(t) = filter.to { cb.push(" AND created_at <= ").push_bind(fmt(t)); }
        for (k, v) in filter.metadata { cb.push(" AND json_extract(metadata, ").push_bind(metadata_path(k)).push(") = ").push_bind(v.clone()); }
        let (total,): (i64,) = cb.build_query_as().fetch_one(&self.pool).await?;

        let offset = crate::api::pagination::offset(page, per_page) as i64;
        let mut qb = QueryBuilder::<sqlx::Sqlite>::new(format!("SELECT {REQUEST_LOG_COLS} FROM request_logs WHERE org_id = "));
        qb.push_bind(org_id.to_string());
        if let Some(app) = app_id { qb.push(" AND app_id = ").push_bind(app.to_string()); }
        if let Some(m) = filter.model { qb.push(" AND model = ").push_bind(m.to_string()); }
        if let Some(p) = filter.provider { qb.push(" AND provider = ").push_bind(p.to_string()); }
        if let Some(s) = filter.status { qb.push(" AND status = ").push_bind(s.to_string()); }
        if let Some(f) = filter.from { qb.push(" AND created_at >= ").push_bind(fmt(f)); }
        if let Some(t) = filter.to { qb.push(" AND created_at <= ").push_bind(fmt(t)); }
        for (k, v) in filter.metadata { qb.push(" AND json_extract(metadata, ").push_bind(metadata_path(k)).push(") = ").push_bind(v.clone()); }
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
        let fmt = |dt: chrono::DateTime<chrono::Utc>| dt.format("%Y-%m-%d %H:%M:%S").to_string();
        const COLS: &str = "id, actor_key_id, actor_label, action, target_type, target_id, before_json, after_json, created_at, org_id";

        // One org's entries, or those filed under no org (`org_id` NULL).
        let mut cb = QueryBuilder::<sqlx::Sqlite>::new("SELECT COUNT(*) FROM audit_log WHERE org_id ");
        match org {
            crate::db::AuditLogOrg::Org(o) => cb.push("= ").push_bind(o.to_string()),
            crate::db::AuditLogOrg::NoOrg => cb.push("IS NULL"),
        };
        if let Some(a) = filter.actor_key_id { cb.push(" AND actor_key_id = ").push_bind(a.to_string()); }
        if let Some(a) = filter.action { cb.push(" AND action = ").push_bind(a.to_string()); }
        if let Some(f) = filter.from { cb.push(" AND created_at >= ").push_bind(fmt(f)); }
        if let Some(t) = filter.to { cb.push(" AND created_at <= ").push_bind(fmt(t)); }
        let (total,): (i64,) = cb.build_query_as().fetch_one(&self.pool).await?;

        let offset = crate::api::pagination::offset(page, per_page) as i64;
        let mut qb = QueryBuilder::<sqlx::Sqlite>::new(format!("SELECT {COLS} FROM audit_log WHERE org_id "));
        match org {
            crate::db::AuditLogOrg::Org(o) => qb.push("= ").push_bind(o.to_string()),
            crate::db::AuditLogOrg::NoOrg => qb.push("IS NULL"),
        };
        if let Some(a) = filter.actor_key_id { qb.push(" AND actor_key_id = ").push_bind(a.to_string()); }
        if let Some(a) = filter.action { qb.push(" AND action = ").push_bind(a.to_string()); }
        if let Some(f) = filter.from { qb.push(" AND created_at >= ").push_bind(fmt(f)); }
        if let Some(t) = filter.to { qb.push(" AND created_at <= ").push_bind(fmt(t)); }
        qb.push(" ORDER BY created_at DESC LIMIT ").push_bind(per_page as i64).push(" OFFSET ").push_bind(offset);
        let rows: Vec<AuditLogRow> = qb.build_query_as().fetch_all(&self.pool).await?;
        Ok((rows.into_iter().map(audit_log_from_row).collect(), total as u64))
    }
}

// ─── Row Types (shared with Postgres via FromRow) ───────────────────────────

#[derive(sqlx::FromRow)]
pub(crate) struct RequestLogRow {
    pub id: String,
    pub model: String,
    pub provider: String,
    pub status: String,
    pub media_type: String,
    pub cost_usd: f64,
    pub latency_ms: i64,
    pub error: Option<String>,
    pub metadata: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Migration 036; false where a select leaves it out.
    #[sqlx(default)]
    pub cache_hit: bool,
}

#[derive(sqlx::FromRow)]
pub(crate) struct ApiKeyRow {
    pub id: String,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub is_active: bool,
    pub token_quota: Option<f64>,
    pub tokens_used: f64,
    pub rpm_limit: Option<i32>,
    pub scopes: String,
    pub webhook_url: Option<String>,
    pub owner_user_id: Option<String>,
    pub org_id: Option<String>,
    pub app_id: Option<String>,
    pub public_id: Option<String>,
}

#[derive(sqlx::FromRow)]
pub(crate) struct ModelUsageRow {
    pub model: String,
    pub requests: i64,
    pub cost_usd: f64,
    pub avg_latency_ms: f64,
}

#[derive(sqlx::FromRow)]
pub(crate) struct ProviderUsageRow {
    pub provider: String,
    pub requests: i64,
    pub failures: i64,
    pub cost_usd: f64,
    pub avg_latency_ms: f64,
}

// ─── Shared conversion helpers ──────────────────────────────────────────────

pub(crate) fn request_log_from_row(r: RequestLogRow) -> RequestLog {
    RequestLog {
        id: r.id,
        model: r.model,
        provider: r.provider,
        status: parse_status(&r.status),
        media_type: parse_media_type(&r.media_type),
        cost_usd: r.cost_usd,
        latency_ms: r.latency_ms as u64,
        created_at: r.created_at,
        error: r.error,
        metadata: r
            .metadata
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok()),
        cache_hit: r.cache_hit,
    }
}

pub(crate) fn api_key_from_row(r: ApiKeyRow) -> ApiKey {
    ApiKey {
        id: Uuid::parse_str(&r.id).unwrap_or_default(),
        name: r.name,
        key_hash: r.key_hash,
        key_prefix: r.key_prefix,
        created_at: r.created_at,
        expires_at: r.expires_at,
        is_active: r.is_active,
        token_quota: r.token_quota,
        tokens_used: r.tokens_used,
        rpm_limit: r.rpm_limit.map(|v| v as u32),
        scopes: r.scopes,
        webhook_url: r.webhook_url,
        owner_user_id: r.owner_user_id,
        org_id: r.org_id,
        app_id: r.app_id,
        public_id: r.public_id,
    }
}

pub(crate) fn build_proxy_stats(
    totals: (i64, i64, i64, f64, f64),
    model_stats: Vec<ModelUsageRow>,
    provider_stats: Vec<ProviderUsageRow>,
    rpm: i64,
    latency_percentiles: LatencyPercentiles,
) -> ProxyStats {
    ProxyStats {
        total_requests: totals.0 as u64,
        successful_requests: totals.1 as u64,
        failed_requests: totals.2 as u64,
        total_cost_usd: totals.3,
        avg_latency_ms: totals.4,
        requests_per_minute: rpm as f64,
        models_used: model_stats
            .into_iter()
            .map(|r| ModelUsageStat {
                model: r.model,
                requests: r.requests as u64,
                cost_usd: r.cost_usd,
                avg_latency_ms: r.avg_latency_ms,
            })
            .collect(),
        providers_used: provider_stats
            .into_iter()
            .map(|r| ProviderUsageStat {
                provider: r.provider,
                requests: r.requests as u64,
                failures: r.failures as u64,
                cost_usd: r.cost_usd,
                avg_latency_ms: r.avg_latency_ms,
            })
            .collect(),
        latency_percentiles,
    }
}

/// Compute p50/p95/p99 from a sorted vec of (latency_ms,) rows.
pub(crate) fn compute_percentiles(rows: Vec<(i64,)>, window_minutes: i64) -> LatencyPercentiles {
    let n = rows.len();
    if n == 0 {
        return LatencyPercentiles { p50_ms: 0.0, p95_ms: 0.0, p99_ms: 0.0, sample_count: 0, window_minutes };
    }
    let values: Vec<f64> = rows.into_iter().map(|(v,)| v as f64).collect();
    let percentile = |pct: f64| -> f64 {
        // Nearest-rank method
        let rank = ((pct / 100.0) * n as f64).ceil() as usize;
        let idx = rank.saturating_sub(1).min(n - 1);
        values[idx]
    };
    LatencyPercentiles {
        p50_ms: percentile(50.0),
        p95_ms: percentile(95.0),
        p99_ms: percentile(99.0),
        sample_count: n as u64,
        window_minutes,
    }
}

// ─── Generation row type ─────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
pub(crate) struct GenerationRow {
    pub id: String,
    pub key_id: Option<String>,
    pub model: String,
    pub provider: String,
    pub media_type: String,
    pub status: String,
    pub progress: i32,
    pub provider_job_id: Option<String>,
    pub result_url: Option<String>,
    pub error_message: Option<String>,
    pub cost_usd: f64,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub metadata: Option<String>,
    pub org_id: Option<String>,
    pub app_id: Option<String>,
    pub files_deleted_at: Option<DateTime<Utc>>,
    pub files_deleted_reason: Option<String>,
}

pub(crate) fn generation_from_row(r: GenerationRow) -> Generation {
    Generation {
        id: r.id,
        key_id: r.key_id.as_deref().and_then(|s| Uuid::parse_str(s).ok()),
        model: r.model,
        provider: r.provider,
        media_type: r.media_type,
        status: parse_status(&r.status),
        progress: r.progress,
        provider_job_id: r.provider_job_id,
        result_url: r.result_url,
        error_message: r.error_message,
        cost_usd: r.cost_usd,
        created_at: r.created_at,
        completed_at: r.completed_at,
        metadata: r.metadata.as_deref().and_then(|s| serde_json::from_str(s).ok()),
        org_id: r.org_id,
        app_id: r.app_id,
        files_deleted_at: r.files_deleted_at,
        files_deleted_reason: r.files_deleted_reason,
    }
}

// ─── Audit Log row type ──────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
pub(crate) struct AuditLogRow {
    pub id: String,
    pub actor_key_id: Option<String>,
    pub actor_label: String,
    pub action: String,
    pub target_type: String,
    pub target_id: String,
    pub before_json: Option<String>,
    pub after_json: Option<String>,
    pub created_at: DateTime<Utc>,
    pub org_id: Option<String>,
}

pub(crate) fn audit_log_from_row(r: AuditLogRow) -> AuditLogEntry {
    AuditLogEntry {
        id: r.id,
        actor_key_id: r.actor_key_id,
        actor_label: r.actor_label,
        action: r.action,
        target_type: r.target_type,
        target_id: r.target_id,
        before_json: r.before_json,
        after_json: r.after_json,
        created_at: r.created_at,
        org_id: r.org_id,
    }
}

// ─── Webhook Delivery row type ───────────────────────────────────────────────

#[derive(sqlx::FromRow)]
pub(crate) struct WebhookDeliveryRow {
    pub id: String,
    pub key_id: String,
    pub generation_id: String,
    pub url: String,
    pub attempt_number: i32,
    pub status_code: Option<i32>,
    pub success: i32,
    pub response_body: Option<String>,
    pub error_message: Option<String>,
    pub payload_json: String,
    pub created_at: DateTime<Utc>,
    pub delivery_id: Option<String>,
}

pub(crate) fn webhook_delivery_from_row(r: WebhookDeliveryRow) -> WebhookDelivery {
    WebhookDelivery {
        id: r.id,
        key_id: r.key_id,
        generation_id: r.generation_id,
        url: r.url,
        attempt_number: r.attempt_number,
        status_code: r.status_code,
        success: r.success != 0,
        response_body: r.response_body,
        error_message: r.error_message,
        payload_json: r.payload_json,
        created_at: r.created_at,
        delivery_id: r.delivery_id,
    }
}

// ─── Webhook outbox row types (migration 029) ────────────────────────────────

/// A `webhook_outbox` row's columns, as both backends select them.
pub(crate) const WEBHOOK_OUTBOX_COLS: &str = "id, generation_id, key_id, org_id, app_id, status, attempts, next_attempt_at, \
    claimed_by, claimed_until, last_status_code, last_error, payload_json, created_at, finished_at, redelivered_at";

#[derive(sqlx::FromRow)]
pub(crate) struct WebhookOutboxRow {
    pub id: String,
    pub generation_id: String,
    pub key_id: String,
    pub org_id: Option<String>,
    pub app_id: Option<String>,
    pub status: String,
    pub attempts: i32,
    pub next_attempt_at: DateTime<Utc>,
    pub claimed_by: Option<String>,
    pub claimed_until: Option<DateTime<Utc>>,
    pub last_status_code: Option<i32>,
    pub last_error: Option<String>,
    pub payload_json: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub redelivered_at: Option<DateTime<Utc>>,
}

pub(crate) fn webhook_outbox_from_row(r: WebhookOutboxRow) -> crate::db::OutboxRecord {
    crate::db::OutboxRecord {
        id: r.id,
        generation_id: r.generation_id,
        key_id: r.key_id,
        org_id: r.org_id,
        app_id: r.app_id,
        status: r.status,
        attempts: r.attempts,
        next_attempt_at: r.next_attempt_at,
        claimed_by: r.claimed_by,
        claimed_until: r.claimed_until,
        last_status_code: r.last_status_code,
        last_error: r.last_error,
        payload_json: r.payload_json,
        created_at: r.created_at,
        finished_at: r.finished_at,
        redelivered_at: r.redelivered_at,
    }
}

/// A delivery a claim took: the columns its `RETURNING` lists.
#[derive(sqlx::FromRow)]
pub(crate) struct OutboxClaimRow {
    pub id: String,
    pub generation_id: String,
    pub key_id: String,
    pub org_id: Option<String>,
    pub attempts: i32,
    pub redelivered: bool,
}

impl From<OutboxClaimRow> for crate::db::OutboxDelivery {
    fn from(r: OutboxClaimRow) -> Self {
        Self { id: r.id, generation_id: r.generation_id, key_id: r.key_id, org_id: r.org_id, attempts: r.attempts, redelivered: r.redelivered }
    }
}

/// `t` as the request log writes its `created_at` (`datetime('now')`:
/// `2026-11-15 08:05:09`, UTC), so a comparison with it uses the index.
fn request_log_time(t: chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// `d` as a SQLite date-time modifier of fractional seconds, e.g. `+30.000 seconds`.
fn sqlite_seconds(d: chrono::Duration) -> String {
    format!("{:+.3} seconds", d.num_milliseconds() as f64 / 1000.0)
}

/// A file deletion a claim took (migration 031): the columns its `RETURNING`
/// lists, on both backends.
#[derive(sqlx::FromRow)]
pub(crate) struct TenantFileDeletionRow {
    pub id: i64,
    pub scope: String,
    pub tenant_id: String,
    pub generation_id: String,
    pub url: String,
    pub attempts: i32,
}

impl From<TenantFileDeletionRow> for crate::db::TenantFileDeletion {
    fn from(r: TenantFileDeletionRow) -> Self {
        Self { id: r.id, scope: r.scope, tenant_id: r.tenant_id, generation_id: r.generation_id, url: r.url, attempts: r.attempts }
    }
}

/// How many file deletions one insert records: four bound values each, well
/// under either backend's limit on a statement's.
pub(crate) const TENANT_FILES_PER_INSERT: usize = 500;

/// Record, in `tx`, each file litegen kept for the generations of org
/// (`scope` "org") or app ("app") `id` whose files aren't deleted yet
/// ([`super::trait_def::kept_file_urls`]), for `proxy::tenant_files` to delete
/// from litegen's store once their rows are gone (migration 031). Run in the
/// transaction that deletes the rows, after a write has taken SQLite's write
/// lock, so no generation changes between this read and their delete.
async fn record_tenant_files(tx: &mut sqlx::SqliteConnection, scope: &str, id: &str) -> Result<(), sqlx::Error> {
    use futures::TryStreamExt;
    let column = if scope == "org" { "org_id" } else { "app_id" };
    let sql = format!("SELECT id, result_url, metadata FROM generations WHERE {column} = ? AND files_deleted_at IS NULL");
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
/// due now, [`TENANT_FILES_PER_INSERT`] to a statement.
async fn insert_tenant_files(conn: &mut sqlx::SqliteConnection, scope: &str, tenant: &str, files: &[(String, String)]) -> Result<(), sqlx::Error> {
    for chunk in files.chunks(TENANT_FILES_PER_INSERT) {
        let mut insert = QueryBuilder::new("INSERT INTO tenant_file_deletions (scope, tenant_id, generation_id, url, attempts, next_attempt_at, created_at) ");
        insert.push_values(chunk, |mut row, (generation, url)| {
            row.push_bind(scope)
                .push_bind(tenant)
                .push_bind(generation.as_str())
                .push_bind(url.as_str())
                .push("0")
                .push("strftime('%Y-%m-%d %H:%M:%f', 'now')")
                .push("strftime('%Y-%m-%d %H:%M:%f', 'now')");
        });
        insert.build().execute(&mut *conn).await?;
    }
    Ok(())
}

// ─── Request Artifact row type ───────────────────────────────────────────────

#[derive(sqlx::FromRow)]
pub struct RequestArtifactRow {
    pub request_id: String,
    pub media_type: String,
    pub prompt: Option<String>,
    pub negative_prompt: Option<String>,
    pub params_json: Option<String>,
    pub refs_meta_json: Option<String>,
    pub output_kind: String,
    pub output_value: Option<String>,
    pub output_mime: Option<String>,
    pub output_truncated: bool,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub org_id: Option<String>,
    pub app_id: Option<String>,
}

pub fn artifact_from_row(r: RequestArtifactRow) -> RequestArtifact {
    RequestArtifact {
        request_id: r.request_id,
        media_type: r.media_type,
        prompt: r.prompt,
        negative_prompt: r.negative_prompt,
        params_json: r.params_json.as_deref().and_then(|s| serde_json::from_str(s).ok()),
        refs_meta_json: r.refs_meta_json.as_deref().and_then(|s| serde_json::from_str(s).ok()),
        output_kind: r.output_kind,
        output_value: r.output_value,
        output_mime: r.output_mime,
        output_truncated: r.output_truncated,
        error_message: r.error_message,
        created_at: r.created_at,
        org_id: r.org_id,
        app_id: r.app_id,
    }
}

fn parse_status(s: &str) -> GenerationStatus {
    match s {
        "completed" => GenerationStatus::Completed,
        "failed" => GenerationStatus::Failed,
        "processing" => GenerationStatus::Processing,
        "cancelled" => GenerationStatus::Cancelled,
        _ => GenerationStatus::Pending,
    }
}

fn parse_media_type(s: &str) -> MediaType {
    match s {
        "video" => MediaType::Video,
        "model3d" => MediaType::Model3d,
        _ => MediaType::Image,
    }
}

// ─── User row type ───────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
pub(crate) struct UserRow {
    pub id: String,
    pub email: String,
    pub password_hash: Option<String>,
    pub role: String,
    pub oauth_github_id: Option<String>,
    pub oauth_google_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub is_active: bool,
    pub sign_in_epoch: i64,
}

pub(crate) fn user_from_row(r: UserRow) -> User {
    User {
        id: r.id,
        email: r.email,
        password_hash: r.password_hash,
        role: Role::parse(&r.role).unwrap_or(Role::Viewer),
        oauth_github_id: r.oauth_github_id,
        oauth_google_id: r.oauth_google_id,
        created_at: r.created_at,
        updated_at: r.updated_at,
        last_login_at: r.last_login_at,
        is_active: r.is_active,
        sign_in_epoch: r.sign_in_epoch,
    }
}

/// The clause that makes an insert (`INSERT … SELECT <values>`) land only
/// while the sign-in `from` is live (review round 6, finding 2), and the id
/// it binds after the values; nothing for the master key. SQLite runs one
/// writer at a time, so a reset, a deactivation or a password change commits
/// wholly before or after the statement.
fn live_sign_in_guard(from: crate::db::SignIn<'_>) -> (&'static str, Option<&str>) {
    use crate::db::SignIn;
    match from {
        SignIn::MasterKey => ("", None),
        SignIn::Session(id) => (
            " WHERE EXISTS (SELECT 1 FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.id = ? AND u.is_active AND s.sign_in_epoch = u.sign_in_epoch)",
            Some(id),
        ),
        SignIn::Grant(id) => (
            " WHERE EXISTS (SELECT 1 FROM mcp_oauth_grants g JOIN users u ON u.id = g.user_id \
             WHERE g.id = ? AND g.revoked_at IS NULL AND u.is_active AND g.sign_in_epoch = u.sign_in_epoch)",
            Some(id),
        ),
    }
}

/// Delete the pending invitations, in `org_id` or every org, that go to
/// `email` or that `invited_by` sent, through `executor` (the pool, or a
/// deactivation's or a confirm's transaction), and return them: the one
/// statement behind every revoke (review round 6, finding 6).
async fn revoke_pending_invitations_in<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(
    executor: E,
    org_id: Option<&str>,
    email: Option<&str>,
    invited_by: Option<&str>,
) -> Result<Vec<Invitation>, sqlx::Error> {
    let rows: Vec<InvitationRow> = sqlx::query_as(
        "DELETE FROM invitations WHERE used_at IS NULL AND (? IS NULL OR org_id = ?) AND (email = ? OR invited_by = ?) \
         RETURNING id, email, role, token, invited_by, org_id, expires_at, used_at, created_at",
    )
    .bind(org_id)
    .bind(org_id)
    .bind(email)
    .bind(invited_by)
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(invitation_from_row).collect())
}

/// The account `id` through `executor`: the pool (`get_user_by_id`) or a
/// transaction (`update_user_with_revoked`'s read-back before its commit).
async fn fetch_user<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(executor: E, id: &str) -> Result<Option<User>, sqlx::Error> {
    let row = sqlx::query_as::<_, UserRow>(
        "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
         created_at, updated_at, last_login_at, is_active, sign_in_epoch \
         FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    Ok(row.map(user_from_row))
}

/// Ends every sign-in `user_id` has, in `tx`: its sessions (all but
/// `keep_session`), its OAuth grants with their tokens, and its OAuth codes
/// not yet redeemed, and moves its sign-in epoch on, so a sign-in begun
/// before and landing after is refused too. Deactivating an account,
/// confirming a reset and changing one's own password do it in the
/// transaction that makes the change (review round 3b, S1 and S2; round 4,
/// findings 2 and 6), so either all of it lands or none. `keep_session` is
/// re-stamped with the new epoch.
async fn end_sign_ins(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, user_id: &str, keep_session: Option<&str>) -> Result<(), sqlx::Error> {
    let now = chrono::Utc::now();
    sqlx::query("UPDATE users SET sign_in_epoch = sign_in_epoch + 1 WHERE id = ?").bind(user_id).execute(&mut **tx).await?;
    match keep_session {
        Some(keep) => {
            sqlx::query("DELETE FROM sessions WHERE user_id = ? AND id <> ?").bind(user_id).bind(keep).execute(&mut **tx).await?;
            sqlx::query("UPDATE sessions SET sign_in_epoch = (SELECT sign_in_epoch FROM users WHERE id = ?) WHERE id = ? AND user_id = ?")
                .bind(user_id)
                .bind(keep)
                .bind(user_id)
                .execute(&mut **tx)
                .await?;
        }
        None => {
            sqlx::query("DELETE FROM sessions WHERE user_id = ?").bind(user_id).execute(&mut **tx).await?;
        }
    }
    sqlx::query("UPDATE mcp_oauth_tokens SET revoked_at = ? WHERE revoked_at IS NULL AND grant_id IN (SELECT id FROM mcp_oauth_grants WHERE user_id = ?)")
        .bind(now)
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE mcp_oauth_grants SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
        .bind(now)
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM mcp_oauth_requests WHERE user_id = ? AND consumed_at IS NULL").bind(user_id).execute(&mut **tx).await?;
    // And the unused links it issued, from a stolen session say (review
    // round 5, finding 4).
    sqlx::query("DELETE FROM password_resets WHERE issued_by = ? AND used_at IS NULL").bind(user_id).execute(&mut **tx).await?;
    Ok(())
}

/// Insert `user` through `executor`: the pool (`create_user`) or the first
/// signup's transaction (`create_first_owner`).
async fn insert_user<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(executor: E, user: &User) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO users
            (id, email, password_hash, role, oauth_github_id, oauth_google_id,
             created_at, updated_at, last_login_at, is_active)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
    .bind(user.is_active as i32)
    .execute(executor)
    .await?;
    Ok(())
}

/// Insert `o` through `executor`: the pool (`create_organization`) or a
/// transaction (`create_org_with_owner`).
async fn insert_organization<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(executor: E, o: &Organization) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO organizations (id, name, slug, plan, status, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
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
async fn insert_application<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(executor: E, a: &Application) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO applications (id, org_id, name, slug, status, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
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

// ─── Session row type ────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
pub(crate) struct SessionRow {
    pub id: String,
    pub user_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub csrf_token: String,
}

pub(crate) fn session_from_row(r: SessionRow) -> Session {
    Session {
        id: r.id,
        user_id: r.user_id,
        created_at: r.created_at,
        expires_at: r.expires_at,
        ip: r.ip,
        user_agent: r.user_agent,
        csrf_token: r.csrf_token,
    }
}

// ─── One-statement auth reads (review 2026-11-11, P14), both backends ────────

/// [`API_KEY_COLS`] read from `api_keys k`: built once, not on every
/// authenticated request (payloads review #12).
pub(crate) fn key_cols_of_k() -> &'static str {
    static COLS: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| API_KEY_COLS.split(',').map(|c| format!("k.{}", c.trim())).collect::<Vec<_>>().join(", "));
    &COLS
}

/// A key and what the joined `users`/`organization_members` rows say about
/// its maker (both NULL when nothing matched).
#[derive(sqlx::FromRow)]
pub(crate) struct ApiKeyOwnerRow {
    #[sqlx(flatten)]
    pub key: ApiKeyRow,
    pub owner_active: Option<bool>,
    pub owner_role: Option<String>,
}

pub(crate) fn api_key_owner_from_row(r: ApiKeyOwnerRow) -> (ApiKey, crate::db::KeyOwner) {
    let owner = crate::db::trait_def::key_owner_of(r.key.owner_user_id.as_deref(), r.owner_active, r.owner_role.as_deref());
    (api_key_from_row(r.key), owner)
}

/// The session columns, then the user's as `u_*` (NULL when the user is gone).
pub(crate) const SESSION_USER_COLS: &str = "s.id, s.user_id, s.created_at, s.expires_at, s.ip, s.user_agent, s.csrf_token, \
    u.id AS u_id, u.email AS u_email, u.password_hash AS u_password_hash, u.role AS u_role, \
    u.oauth_github_id AS u_oauth_github_id, u.oauth_google_id AS u_oauth_google_id, u.created_at AS u_created_at, \
    u.updated_at AS u_updated_at, u.last_login_at AS u_last_login_at, u.is_active AS u_is_active, \
    u.sign_in_epoch AS u_sign_in_epoch";

#[derive(sqlx::FromRow)]
pub(crate) struct SessionUserRow {
    #[sqlx(flatten)]
    pub session: SessionRow,
    pub u_id: Option<String>,
    pub u_email: Option<String>,
    pub u_password_hash: Option<String>,
    pub u_role: Option<String>,
    pub u_oauth_github_id: Option<String>,
    pub u_oauth_google_id: Option<String>,
    pub u_created_at: Option<DateTime<Utc>>,
    pub u_updated_at: Option<DateTime<Utc>>,
    pub u_last_login_at: Option<DateTime<Utc>>,
    pub u_is_active: Option<bool>,
    pub u_sign_in_epoch: Option<i64>,
}

pub(crate) fn session_user_from_row(r: SessionUserRow) -> (Session, Option<User>) {
    let user = match (r.u_id, r.u_email, r.u_role, r.u_created_at, r.u_updated_at, r.u_is_active) {
        (Some(id), Some(email), Some(role), Some(created_at), Some(updated_at), Some(is_active)) => Some(user_from_row(UserRow {
            id,
            email,
            password_hash: r.u_password_hash,
            role,
            oauth_github_id: r.u_oauth_github_id,
            oauth_google_id: r.u_oauth_google_id,
            created_at,
            updated_at,
            last_login_at: r.u_last_login_at,
            is_active,
            sign_in_epoch: r.u_sign_in_epoch.unwrap_or(0),
        })),
        _ => None,
    };
    (session_from_row(r.session), user)
}

/// A generation's app, then that app's `app_file_access` row (all NULL when
/// it has none).
pub(crate) const GENERATION_ACCESS_COLS: &str = "g.app_id AS g_app_id, a.app_id, a.mode, a.owner_scope, \
    a.signed_url_ttl_seconds, a.keep_outputs, a.retention_days, a.signed_links, a.updated_at";

#[derive(sqlx::FromRow)]
pub(crate) struct GenerationAccessRow {
    pub g_app_id: Option<String>,
    pub app_id: Option<String>,
    pub mode: Option<String>,
    pub owner_scope: Option<String>,
    pub signed_url_ttl_seconds: Option<i64>,
    pub keep_outputs: Option<bool>,
    pub retention_days: Option<i64>,
    pub signed_links: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub(crate) fn generation_access_from_row(r: GenerationAccessRow) -> (Option<String>, Option<crate::types::AppFileAccessRow>) {
    let access = match (r.app_id, r.updated_at) {
        (Some(app_id), Some(updated_at)) => Some(crate::types::AppFileAccessRow {
            app_id,
            mode: r.mode,
            owner_scope: r.owner_scope,
            signed_url_ttl_seconds: r.signed_url_ttl_seconds,
            keep_outputs: r.keep_outputs,
            retention_days: r.retention_days,
            signed_links: r.signed_links,
            updated_at,
        }),
        _ => None,
    };
    (r.g_app_id, access)
}

// ─── Invitation row type ─────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
pub(crate) struct InvitationRow {
    pub id: String,
    pub email: String,
    pub role: String,
    pub token: String,
    pub invited_by: Option<String>,
    pub org_id: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub(crate) fn invitation_from_row(r: InvitationRow) -> Invitation {
    Invitation {
        id: r.id,
        email: r.email,
        role: Role::parse(&r.role).unwrap_or(Role::Viewer),
        token: r.token,
        invited_by: r.invited_by,
        org_id: r
            .org_id
            .unwrap_or_else(|| crate::api::middleware::DEFAULT_ORG_ID.to_string()),
        expires_at: r.expires_at,
        used_at: r.used_at,
        created_at: r.created_at,
    }
}

// ─── PasswordReset row type ──────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
pub(crate) struct PasswordResetRow {
    pub token: String,
    pub user_id: String,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub issued_by: Option<String>,
}

pub(crate) fn password_reset_from_row(r: PasswordResetRow) -> PasswordReset {
    PasswordReset {
        token: r.token,
        user_id: r.user_id,
        expires_at: r.expires_at,
        used_at: r.used_at,
        created_at: r.created_at,
        issued_by: r.issued_by,
    }
}

// ─── Tenancy row types (shared with Postgres via FromRow) ────────────────────

#[derive(sqlx::FromRow)]
pub(crate) struct OrganizationRow {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub plan: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) fn organization_from_row(r: OrganizationRow) -> Organization {
    Organization {
        id: r.id,
        name: r.name,
        slug: r.slug,
        plan: r.plan,
        status: r.status,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}

/// Organization row joined with the caller's membership role.
#[derive(sqlx::FromRow)]
pub(crate) struct OrgWithRoleRow {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub plan: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub role: String,
}

pub(crate) fn org_with_role_from_row(r: OrgWithRoleRow) -> (Organization, Role) {
    let role = Role::parse(&r.role).unwrap_or(Role::Viewer);
    (
        Organization {
            id: r.id,
            name: r.name,
            slug: r.slug,
            plan: r.plan,
            status: r.status,
            created_at: r.created_at,
            updated_at: r.updated_at,
        },
        role,
    )
}

#[derive(sqlx::FromRow)]
pub(crate) struct ApplicationRow {
    pub id: String,
    pub org_id: String,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) fn application_from_row(r: ApplicationRow) -> Application {
    Application {
        id: r.id,
        org_id: r.org_id,
        name: r.name,
        slug: r.slug,
        status: r.status,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct OrgMemberRow {
    pub org_id: String,
    pub user_id: String,
    pub email: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

pub(crate) fn org_member_from_row(r: OrgMemberRow) -> OrganizationMember {
    OrganizationMember {
        org_id: r.org_id,
        user_id: r.user_id,
        email: r.email,
        role: Role::parse(&r.role).unwrap_or(Role::Viewer),
        created_at: r.created_at,
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct ProviderCredentialRow {
    pub provider: String,
    pub display_hint: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub(crate) fn provider_credential_from_row(r: ProviderCredentialRow) -> ProviderCredentialInfo {
    ProviderCredentialInfo {
        provider: r.provider,
        display_hint: r.display_hint,
        created_at: r.created_at,
    }
}

/// The JSON path to one `metadata` key. Keys are validated to letters,
/// digits, `_`, `.` and `-` (`validator::is_metadata_key`), so quoting the
/// key is enough to read a dotted one as a single key.
fn metadata_path(key: &str) -> String {
    format!("$.\"{key}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn org_provider_credential_roundtrip() {
        let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        // org FK target
        sqlx::query("INSERT INTO organizations (id, name, slug, created_at) VALUES ('o1','Org1','org1',datetime('now'))")
            .execute(db.pool()).await.unwrap();

        db.upsert_org_provider_credential("o1", "openai", "CT", "NONCE", Some("…1234")).await.unwrap();
        assert_eq!(db.get_org_provider_credential("o1", "openai").await.unwrap(), Some(("CT".into(), "NONCE".into())));

        let list = db.list_org_provider_credentials("o1").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].provider, "openai");
        assert_eq!(list[0].display_hint.as_deref(), Some("…1234"));

        // upsert overwrites
        db.upsert_org_provider_credential("o1", "openai", "CT2", "N2", Some("…9999")).await.unwrap();
        assert_eq!(db.get_org_provider_credential("o1", "openai").await.unwrap(), Some(("CT2".into(), "N2".into())));

        assert!(db.delete_org_provider_credential("o1", "openai").await.unwrap());
        assert_eq!(db.get_org_provider_credential("o1", "openai").await.unwrap(), None);
        assert!(!db.delete_org_provider_credential("o1", "openai").await.unwrap());
    }

    #[tokio::test]
    async fn resolves_org_credential_for_request() {
        let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO organizations (id, name, slug, created_at) VALUES ('o1','O','o1',datetime('now'))")
            .execute(db.pool()).await.unwrap();
        db.upsert_org_provider_credential("o1", "mock", "CT", "N", Some("…1")).await.unwrap();
        // get_org_provider_credential is what the resolver now calls:
        assert!(db.get_org_provider_credential("o1", "mock").await.unwrap().is_some());
        assert!(db.get_org_provider_credential("o2", "mock").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn migration_creates_org_cred_table_and_drops_app_table() {
        // Fresh in-memory DB with all migrations (incl. 013) applied.
        let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        let pool = db.pool();

        // New org-scoped table exists and starts empty.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM org_provider_credentials").fetch_one(pool).await.unwrap();
        assert_eq!(n, 0, "fresh org cred table starts empty");

        // Old per-app table is gone.
        let dropped = sqlx::query("SELECT 1 FROM provider_credentials LIMIT 1").fetch_optional(pool).await;
        assert!(dropped.is_err(), "provider_credentials table should be dropped by migration 013");
    }
}
