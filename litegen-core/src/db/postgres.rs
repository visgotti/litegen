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
use super::trait_def::DatabaseStore;

/// Full column list for api_keys selects (Postgres).
const API_KEY_COLS: &str = "id, name, key_hash, key_prefix, created_at, expires_at, is_active, \
    token_quota, tokens_used, rpm_limit, scopes, webhook_url, owner_user_id, org_id, app_id, public_id";

/// Full column list for generations selects (Postgres).
const GENERATION_COLS: &str = "id, key_id, model, provider, media_type, status, progress, \
    provider_job_id, result_url, error_message, cost_usd, created_at, completed_at, metadata, \
    org_id, app_id";

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
        if let Some(v) = req.rpm_limit { q = q.bind(v as i32); }
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

    async fn rotate_api_key(
        &self,
        id: &Uuid,
        public_id: &str,
        key_hash: &str,
        key_prefix: &str,
    ) -> Result<Option<ApiKey>, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE api_keys SET public_id = $1, key_hash = $2, key_prefix = $3 WHERE id = $4",
        )
        .bind(public_id)
        .bind(key_hash)
        .bind(key_prefix)
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
    ) -> Result<ProxyStats, sqlx::Error> {
        // Tenant predicate; $1 = org_id, $2 = app_id when present.
        let tenant_clause = if app_id.is_some() {
            "org_id = $1 AND app_id = $2"
        } else {
            "org_id = $1"
        };
        macro_rules! bind_tenant {
            ($q:expr) => {{
                let q = $q.bind(org_id);
                match app_id {
                    Some(a) => q.bind(a),
                    None => q,
                }
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
        sqlx::query(
            r#"
            INSERT INTO audit_log
                (id, actor_key_id, actor_label, action, target_type, target_id,
                 before_json, after_json, org_id, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, $10), NOW())
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
        .bind(crate::api::middleware::DEFAULT_ORG_ID)
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
             created_at, updated_at, last_login_at, is_active \
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
             created_at, updated_at, last_login_at, is_active \
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
             created_at, updated_at, last_login_at, is_active \
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
            "INSERT INTO invitations (id, email, role, token, invited_by, org_id, expires_at, used_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
        )
        .bind(&inv.id)
        .bind(&inv.email)
        .bind(inv.role.as_str())
        .bind(&inv.token)
        .bind(&inv.invited_by)
        .bind(&inv.org_id)
        .bind(inv.expires_at)
        .bind(inv.used_at)
        .bind(inv.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_invitation(&self, token: &str) -> Result<Option<Invitation>, sqlx::Error> {
        let row = sqlx::query_as::<_, InvitationRow>(
            "SELECT id, email, role, token, invited_by, org_id, expires_at, used_at, created_at \
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
            "SELECT id, email, role, token, invited_by, org_id, expires_at, used_at, created_at \
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

    // ─── Organizations ──────────────────────────────────────────────────

    async fn create_organization(&self, o: &Organization) -> Result<(), sqlx::Error> {
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
        .execute(&self.pool)
        .await?;
        Ok(())
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
        let result = sqlx::query("DELETE FROM organizations WHERE id = $1")
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
        sqlx::query("UPDATE organization_members SET role = $1 WHERE org_id = $2 AND user_id = $3")
            .bind(role.as_str())
            .bind(org_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn remove_org_member(&self, org_id: &str, user_id: &str) -> Result<(), sqlx::Error> {
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
        sqlx::query("UPDATE organization_members SET role = 'admin' WHERE org_id = $1 AND role = 'owner'")
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        let promoted =
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

    // ─── Applications ───────────────────────────────────────────────────

    async fn create_application(&self, a: &Application) -> Result<(), sqlx::Error> {
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
        .execute(&self.pool)
        .await?;
        Ok(())
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
        let result = sqlx::query("DELETE FROM applications WHERE id = $1")
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
             VALUES ($1, $2, $3, $4, $5, $6, NOW(), NOW()) \
             ON CONFLICT (app_id, provider) DO UPDATE SET \
                ciphertext = EXCLUDED.ciphertext, \
                nonce = EXCLUDED.nonce, \
                display_hint = EXCLUDED.display_hint, \
                updated_at = NOW()",
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
            "SELECT ciphertext, nonce FROM provider_credentials WHERE app_id = $1 AND provider = $2",
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
             WHERE app_id = $1 ORDER BY provider ASC",
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
            sqlx::query("DELETE FROM provider_credentials WHERE app_id = $1 AND provider = $2")
                .bind(app_id)
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
                 created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW(), NOW()) \
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_app_storage(&self, app_id: &str) -> Result<Option<AppStorageRow>, sqlx::Error> {
        let row = sqlx::query_as::<_, AppStorageRow>(
            "SELECT backend, bucket_name, region, endpoint_url, custom_public_url, \
                    path_prefix, access_key_id_hint, secret_ciphertext, secret_nonce, updated_at \
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
             VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE, 0, $8, $9, $10, $11, NOW())",
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
        let offset = ((page.saturating_sub(1)) * per_page) as i64;
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
}
