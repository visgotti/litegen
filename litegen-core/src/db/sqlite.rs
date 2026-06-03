use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
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
    "id, model, provider, status, media_type, cost_usd, latency_ms, error, metadata, created_at";

/// Full column list for generations selects.
const GENERATION_COLS: &str = "id, key_id, model, provider, media_type, status, progress, \
    provider_job_id, result_url, error_message, cost_usd, created_at, completed_at, metadata";

/// SQLite-backed database implementation.
pub struct SqliteDatabase {
    pool: SqlitePool,
}

impl SqliteDatabase {
    /// Connect to SQLite and run migrations.
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;

        sqlx::migrate!("./migrations/sqlite").run(&pool).await?;
        info!("SQLite database connected and migrations applied");

        Ok(Self { pool })
    }

    /// Test-only accessor to the underlying pool so unit tests can stamp rows
    /// (e.g. set tenant columns) that the public API doesn't yet write.
    #[cfg(test)]
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[async_trait]
impl DatabaseStore for SqliteDatabase {
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
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO generations
                (id, key_id, model, provider, media_type, status, progress,
                 provider_job_id, cost_usd, created_at)
            VALUES (?, ?, ?, ?, ?, 'pending', 0, ?, ?, datetime('now'))
            "#,
        )
        .bind(id)
        .bind(key_id.map(|u| u.to_string()))
        .bind(model)
        .bind(provider)
        .bind(media_type)
        .bind(provider_job_id)
        .bind(cost_usd)
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
        let offset = (page.saturating_sub(1)) * per_page;
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
    ) -> Result<(), sqlx::Error> {
        let meta_str = metadata.map(|m| m.to_string());
        sqlx::query(
            r#"
            INSERT INTO request_logs (id, model, provider, status, media_type, cost_usd, latency_ms, error, metadata, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_request_logs(
        &self,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
        let offset = (page.saturating_sub(1)) * per_page;

        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM request_logs")
            .fetch_one(&self.pool)
            .await?;

        let rows = sqlx::query_as::<_, RequestLogRow>(
            r#"
            SELECT id, model, provider, status, media_type, cost_usd, latency_ms, error, metadata, created_at
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
        let offset = (page.saturating_sub(1)) * per_page;

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
        let id = Uuid::new_v4();
        let now = Utc::now();

        sqlx::query(
            r#"
            INSERT INTO api_keys (id, name, key_hash, key_prefix, created_at, is_active, token_quota, tokens_used, rpm_limit, scopes, webhook_url)
            VALUES (?, ?, ?, ?, ?, 1, ?, 0, ?, ?, ?)
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
            org_id: None,
            app_id: None,
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
        if let Some(v) = req.rpm_limit { q = q.bind(v as i64); }
        if let Some(ref v) = req.scopes { q = q.bind(v); }
        if let Some(ref v) = req.webhook_url { q = q.bind(v); }
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

    async fn get_stats(&self) -> Result<ProxyStats, sqlx::Error> {
        let totals: (i64, i64, i64, f64, f64) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*) as total,
                SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END) as success,
                SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) as failed,
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
        sqlx::query(
            r#"
            INSERT INTO audit_log
                (id, actor_key_id, actor_label, action, target_type, target_id,
                 before_json, after_json, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_audit_log(
        &self,
        filter: &AuditLogFilter,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<AuditLogEntry>, i64), sqlx::Error> {
        let offset = (page.saturating_sub(1)) * per_page;

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
             before_json, after_json, created_at FROM audit_log WHERE 1=1",
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
                 response_body, error_message, payload_json, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
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
        let offset = (page.saturating_sub(1)) * per_page;

        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM webhook_deliveries WHERE key_id = ?"
        )
        .bind(key_id)
        .fetch_one(&self.pool)
        .await?;

        let rows = sqlx::query_as::<_, WebhookDeliveryRow>(
            "SELECT id, key_id, generation_id, url, attempt_number, status_code, success, \
             response_body, error_message, payload_json, created_at \
             FROM webhook_deliveries WHERE key_id = ? \
             ORDER BY created_at DESC LIMIT ? OFFSET ?"
        )
        .bind(key_id)
        .bind(per_page as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await?;

        let deliveries = rows.into_iter().map(webhook_delivery_from_row).collect();
        Ok((deliveries, total.0))
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
                 output_kind, output_value, output_mime, output_truncated, error_message, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_request_artifact(&self, request_id: &str) -> Result<Option<RequestArtifact>, sqlx::Error> {
        let row = sqlx::query_as::<_, RequestArtifactRow>(
            "SELECT request_id, media_type, prompt, negative_prompt, params_json, refs_meta_json, \
             output_kind, output_value, output_mime, output_truncated, error_message, created_at \
             FROM request_artifacts WHERE request_id = ?"
        )
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(artifact_from_row))
    }

    // ─── Users ──────────────────────────────────────────────────────────

    async fn create_user(&self, user: &User) -> Result<(), sqlx::Error> {
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_user_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
             created_at, updated_at, last_login_at, is_active \
             FROM users WHERE email = ?"
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(user_from_row))
    }

    async fn get_user_by_id(&self, id: &str) -> Result<Option<User>, sqlx::Error> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
             created_at, updated_at, last_login_at, is_active \
             FROM users WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(user_from_row))
    }

    async fn get_user_by_oauth(&self, provider: &str, oauth_id: &str) -> Result<Option<User>, sqlx::Error> {
        let col = match provider {
            "github" => "oauth_github_id",
            "google" => "oauth_google_id",
            _ => return Ok(None),
        };
        let sql = format!(
            "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
             created_at, updated_at, last_login_at, is_active \
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
        let mut sets: Vec<&str> = Vec::new();
        if role.is_some() { sets.push("role = ?"); }
        if is_active.is_some() { sets.push("is_active = ?"); }
        if password_hash.is_some() { sets.push("password_hash = ?"); }
        sets.push("updated_at = datetime('now')");

        let sql = format!("UPDATE users SET {} WHERE id = ?", sets.join(", "));
        let mut q = sqlx::query(&sql);
        if let Some(r) = role { q = q.bind(r.as_str()); }
        if let Some(a) = is_active { q = q.bind(a as i32); }
        if let Some(ph) = password_hash { q = q.bind(ph); }
        q = q.bind(id);

        let result = q.execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_user_by_id(id).await
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

    async fn list_users(&self) -> Result<Vec<User>, sqlx::Error> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
             created_at, updated_at, last_login_at, is_active \
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
        sqlx::query("UPDATE users SET role = 'admin', updated_at = datetime('now') WHERE role = 'owner'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE users SET role = 'owner', updated_at = datetime('now') WHERE id = ?")
            .bind(new_owner_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    // ─── Sessions ───────────────────────────────────────────────────────

    async fn create_session(&self, s: &Session) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO sessions (id, user_id, created_at, expires_at, ip, user_agent, csrf_token) \
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&s.id)
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
            "SELECT id, user_id, created_at, expires_at, ip, user_agent, csrf_token \
             FROM sessions WHERE user_id = ? ORDER BY created_at DESC"
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
        sqlx::query(
            "INSERT INTO invitations (id, email, role, token, invited_by, expires_at, used_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&inv.id)
        .bind(&inv.email)
        .bind(inv.role.as_str())
        .bind(&inv.token)
        .bind(&inv.invited_by)
        .bind(inv.expires_at)
        .bind(inv.used_at)
        .bind(inv.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_invitation(&self, token: &str) -> Result<Option<Invitation>, sqlx::Error> {
        let row = sqlx::query_as::<_, InvitationRow>(
            "SELECT id, email, role, token, invited_by, expires_at, used_at, created_at \
             FROM invitations WHERE token = ?"
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(invitation_from_row))
    }

    async fn mark_invitation_used(&self, token: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE invitations SET used_at = datetime('now') WHERE token = ?")
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_invitation(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM invitations WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_invitations(&self) -> Result<Vec<Invitation>, sqlx::Error> {
        let rows = sqlx::query_as::<_, InvitationRow>(
            "SELECT id, email, role, token, invited_by, expires_at, used_at, created_at \
             FROM invitations ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(invitation_from_row).collect())
    }

    // ─── Password Resets ────────────────────────────────────────────────

    async fn create_password_reset(&self, r: &PasswordReset) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO password_resets (token, user_id, expires_at, used_at, created_at) \
             VALUES (?, ?, ?, ?, ?)"
        )
        .bind(&r.token)
        .bind(&r.user_id)
        .bind(r.expires_at)
        .bind(r.used_at)
        .bind(r.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_password_reset(&self, token: &str) -> Result<Option<PasswordReset>, sqlx::Error> {
        let row = sqlx::query_as::<_, PasswordResetRow>(
            "SELECT token, user_id, expires_at, used_at, created_at \
             FROM password_resets WHERE token = ?"
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(password_reset_from_row))
    }

    async fn mark_password_reset_used(&self, token: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE password_resets SET used_at = datetime('now') WHERE token = ?")
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
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
        .execute(&self.pool)
        .await?;
        Ok(())
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
        let result = sqlx::query("DELETE FROM organizations WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ─── Members ────────────────────────────────────────────────────────

    async fn add_org_member(
        &self,
        org_id: &str,
        user_id: &str,
        role: Role,
    ) -> Result<(), sqlx::Error> {
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
        sqlx::query("UPDATE organization_members SET role = ? WHERE org_id = ? AND user_id = ?")
            .bind(role.as_str())
            .bind(org_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn remove_org_member(&self, org_id: &str, user_id: &str) -> Result<(), sqlx::Error> {
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
        sqlx::query("UPDATE organization_members SET role = 'admin' WHERE org_id = ? AND role = 'owner'")
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        let promoted =
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

    // ─── Applications ───────────────────────────────────────────────────

    async fn create_application(&self, a: &Application) -> Result<(), sqlx::Error> {
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
        .execute(&self.pool)
        .await?;
        Ok(())
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
        let result = sqlx::query("DELETE FROM applications WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ─── Provider Credentials ───────────────────────────────────────────

    async fn upsert_provider_credential(
        &self,
        app_id: &str,
        provider: &str,
        ciphertext: &str,
        nonce: &str,
        display_hint: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO provider_credentials \
                (id, app_id, provider, ciphertext, nonce, display_hint, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, datetime('now'), datetime('now')) \
             ON CONFLICT (app_id, provider) DO UPDATE SET \
                ciphertext = excluded.ciphertext, \
                nonce = excluded.nonce, \
                display_hint = excluded.display_hint, \
                updated_at = datetime('now')",
        )
        .bind(id.to_string())
        .bind(app_id)
        .bind(provider)
        .bind(ciphertext)
        .bind(nonce)
        .bind(display_hint)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_provider_credential(
        &self,
        app_id: &str,
        provider: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT ciphertext, nonce FROM provider_credentials WHERE app_id = ? AND provider = ?",
        )
        .bind(app_id)
        .bind(provider)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn list_provider_credentials(
        &self,
        app_id: &str,
    ) -> Result<Vec<ProviderCredentialInfo>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ProviderCredentialRow>(
            "SELECT provider, display_hint, created_at FROM provider_credentials \
             WHERE app_id = ? ORDER BY provider ASC",
        )
        .bind(app_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(provider_credential_from_row).collect())
    }

    async fn delete_provider_credential(
        &self,
        app_id: &str,
        provider: &str,
    ) -> Result<bool, sqlx::Error> {
        let result =
            sqlx::query("DELETE FROM provider_credentials WHERE app_id = ? AND provider = ?")
                .bind(app_id)
                .bind(provider)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
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
    ) -> Result<ApiKey, sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO api_keys \
                (id, org_id, app_id, public_id, name, key_hash, key_prefix, is_active, \
                 tokens_used, token_quota, rpm_limit, scopes, webhook_url, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 1, 0, ?, ?, ?, ?, datetime('now'))",
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

    async fn list_generations_for_tenant(
        &self,
        org_id: &str,
        app_id: Option<&str>,
        page: u32,
        per_page: u32,
    ) -> Result<Vec<Generation>, sqlx::Error> {
        let offset = ((page.saturating_sub(1)) * per_page) as i64;
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

    async fn get_request_logs_for_tenant(
        &self,
        org_id: &str,
        app_id: Option<&str>,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
        let offset = ((page.saturating_sub(1)) * per_page) as i64;
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
        let offset = ((page.saturating_sub(1)) * per_page) as i64;
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM audit_log WHERE org_id = ?")
            .bind(org_id)
            .fetch_one(&self.pool)
            .await?;
        let rows = sqlx::query_as::<_, AuditLogRow>(
            "SELECT id, actor_key_id, actor_label, action, target_type, target_id, \
             before_json, after_json, created_at FROM audit_log WHERE org_id = ? \
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
    pub rpm_limit: Option<i64>,
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
    pub progress: i64,
    pub provider_job_id: Option<String>,
    pub result_url: Option<String>,
    pub error_message: Option<String>,
    pub cost_usd: f64,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub metadata: Option<String>,
}

pub(crate) fn generation_from_row(r: GenerationRow) -> Generation {
    Generation {
        id: r.id,
        key_id: r.key_id.as_deref().and_then(|s| Uuid::parse_str(s).ok()),
        model: r.model,
        provider: r.provider,
        media_type: r.media_type,
        status: parse_status(&r.status),
        progress: r.progress as i32,
        provider_job_id: r.provider_job_id,
        result_url: r.result_url,
        error_message: r.error_message,
        cost_usd: r.cost_usd,
        created_at: r.created_at,
        completed_at: r.completed_at,
        metadata: r.metadata.as_deref().and_then(|s| serde_json::from_str(s).ok()),
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
    }
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
    pub output_truncated: i64,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
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
        output_truncated: r.output_truncated != 0,
        error_message: r.error_message,
        created_at: r.created_at,
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
    pub is_active: i64,
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
        is_active: r.is_active != 0,
    }
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

// ─── Invitation row type ─────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
pub(crate) struct InvitationRow {
    pub id: String,
    pub email: String,
    pub role: String,
    pub token: String,
    pub invited_by: Option<String>,
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
}

pub(crate) fn password_reset_from_row(r: PasswordResetRow) -> PasswordReset {
    PasswordReset {
        token: r.token,
        user_id: r.user_id,
        expires_at: r.expires_at,
        used_at: r.used_at,
        created_at: r.created_at,
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
