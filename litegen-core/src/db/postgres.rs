use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::QueryBuilder;
use tracing::info;
use uuid::Uuid;

use crate::types::*;

use super::sqlite::{
    api_key_from_row, audit_log_from_row, build_proxy_stats, generation_from_row,
    invitation_from_row, password_reset_from_row, request_log_from_row, session_from_row,
    user_from_row, webhook_delivery_from_row, ApiKeyRow, AuditLogRow, GenerationRow,
    InvitationRow, ModelUsageRow, PasswordResetRow, ProviderUsageRow, RequestLogRow,
    SessionRow, UserRow, WebhookDeliveryRow,
};
use super::trait_def::DatabaseStore;

/// Full column list for api_keys selects (Postgres).
const API_KEY_COLS: &str = "id, name, key_hash, key_prefix, created_at, expires_at, is_active, \
    token_quota, tokens_used, rpm_limit, scopes, webhook_url, owner_user_id";

/// Full column list for generations selects (Postgres).
const GENERATION_COLS: &str = "id, key_id, model, provider, media_type, status, progress, \
    provider_job_id, result_url, error_message, cost_usd, created_at, completed_at, metadata";

/// PostgreSQL-backed database implementation.
pub struct PostgresDatabase {
    pool: PgPool,
}

impl PostgresDatabase {
    /// Connect to PostgreSQL and run migrations.
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;

        sqlx::migrate!("./migrations/postgres").run(&pool).await?;
        info!("PostgreSQL database connected and migrations applied");

        Ok(Self { pool })
    }
}

#[async_trait]
impl DatabaseStore for PostgresDatabase {
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
            VALUES ($1, $2, $3, $4, $5, 'pending', 0, $6, $7, NOW())
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
        let offset = (page.saturating_sub(1)) * per_page;
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
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
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
        let offset = (page.saturating_sub(1)) * per_page;

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

        let mut data_qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(
            "SELECT id, model, provider, status, media_type, cost_usd, latency_ms, error, metadata, created_at \
             FROM request_logs WHERE 1=1",
        );
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
        let now = chrono::Utc::now();

        sqlx::query(
            r#"
            INSERT INTO api_keys (id, name, key_hash, key_prefix, created_at, is_active, token_quota, tokens_used, rpm_limit, scopes, webhook_url)
            VALUES ($1, $2, $3, $4, $5, TRUE, $6, 0, $7, $8, $9)
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
            "SELECT {} FROM api_keys WHERE key_hash = $1 AND is_active = TRUE",
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
            "SELECT COUNT(*) FROM request_logs WHERE created_at > NOW() - INTERVAL '1 minute'",
        )
        .fetch_one(&self.pool)
        .await?;

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
        sqlx::query(
            r#"
            INSERT INTO audit_log
                (id, actor_key_id, actor_label, action, target_type, target_id,
                 before_json, after_json, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
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
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())
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
            "SELECT COUNT(*) FROM webhook_deliveries WHERE key_id = $1"
        )
        .bind(key_id)
        .fetch_one(&self.pool)
        .await?;

        let rows = sqlx::query_as::<_, WebhookDeliveryRow>(
            "SELECT id, key_id, generation_id, url, attempt_number, status_code, success, \
             response_body, error_message, payload_json, created_at \
             FROM webhook_deliveries WHERE key_id = $1 \
             ORDER BY created_at DESC LIMIT $2 OFFSET $3"
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
            INSERT INTO request_artifacts
                (request_id, media_type, prompt, negative_prompt, params_json, refs_meta_json,
                 output_kind, output_value, output_mime, output_truncated, error_message, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW())
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_request_artifact(&self, request_id: &str) -> Result<Option<RequestArtifact>, sqlx::Error> {
        use super::sqlite::{RequestArtifactRow, artifact_from_row};
        let row = sqlx::query_as::<_, RequestArtifactRow>(
            "SELECT request_id, media_type, prompt, negative_prompt, params_json, refs_meta_json, \
             output_kind, output_value, output_mime, output_truncated::int AS output_truncated, \
             error_message, created_at \
             FROM request_artifacts WHERE request_id = $1"
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_user_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
             created_at, updated_at, last_login_at, is_active::int AS is_active \
             FROM users WHERE email = $1"
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(user_from_row))
    }

    async fn get_user_by_id(&self, id: &str) -> Result<Option<User>, sqlx::Error> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
             created_at, updated_at, last_login_at, is_active::int AS is_active \
             FROM users WHERE id = $1"
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
             created_at, updated_at, last_login_at, is_active::int AS is_active \
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
        let mut sets: Vec<String> = Vec::new();
        let mut param_idx: usize = 1;

        if role.is_some() { sets.push(format!("role = ${}", param_idx)); param_idx += 1; }
        if is_active.is_some() { sets.push(format!("is_active = ${}", param_idx)); param_idx += 1; }
        if password_hash.is_some() { sets.push(format!("password_hash = ${}", param_idx)); param_idx += 1; }
        sets.push("updated_at = NOW()".to_string());

        let sql = format!("UPDATE users SET {} WHERE id = ${}", sets.join(", "), param_idx);
        let mut q = sqlx::query(&sql);
        if let Some(r) = role { q = q.bind(r.as_str()); }
        if let Some(a) = is_active { q = q.bind(a); }
        if let Some(ph) = password_hash { q = q.bind(ph); }
        q = q.bind(id);

        let result = q.execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_user_by_id(id).await
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

    async fn list_users(&self) -> Result<Vec<User>, sqlx::Error> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, role, oauth_github_id, oauth_google_id, \
             created_at, updated_at, last_login_at, is_active::int AS is_active \
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
        sqlx::query("UPDATE users SET role = 'admin', updated_at = NOW() WHERE role = 'owner'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE users SET role = 'owner', updated_at = NOW() WHERE id = $1")
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
             VALUES ($1, $2, $3, $4, $5, $6, $7)"
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
             FROM sessions WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(session_from_row))
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
            "SELECT id, user_id, created_at, expires_at, ip, user_agent, csrf_token \
             FROM sessions WHERE user_id = $1 ORDER BY created_at DESC"
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
        sqlx::query(
            "INSERT INTO invitations (id, email, role, token, invited_by, expires_at, used_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
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
             FROM invitations WHERE token = $1"
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(invitation_from_row))
    }

    async fn mark_invitation_used(&self, token: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE invitations SET used_at = NOW() WHERE token = $1")
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_invitation(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM invitations WHERE id = $1")
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
             VALUES ($1, $2, $3, $4, $5)"
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
             FROM password_resets WHERE token = $1"
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(password_reset_from_row))
    }

    async fn mark_password_reset_used(&self, token: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE password_resets SET used_at = NOW() WHERE token = $1")
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
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
}
