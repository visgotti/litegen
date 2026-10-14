//! [`ManagementStore`] for the SQLite and Postgres backends.
//!
//! One body, expanded twice by `impl_management_store!`. Static SQL is written
//! with `?` placeholders and passed through `$sql` (identity for SQLite,
//! `?` → `$n` for Postgres); dynamic queries use `QueryBuilder`, which numbers
//! placeholders itself. The only other per-backend pieces are how a calendar
//! day is extracted from `request_logs.created_at` and how a timestamp is bound
//! against it (SQLite stores that column as `YYYY-MM-DD HH:MM:SS` text).

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::store::ManagementStore;
use super::types::*;
use crate::db::{PostgresDatabase, SqliteDatabase};

#[derive(sqlx::FromRow)]
struct LimitsRow {
    entity_type: String,
    entity_id: String,
    org_id: Option<String>,
    max_budget_usd: Option<f64>,
    soft_budget_usd: Option<f64>,
    budget_duration: Option<String>,
    spend_usd: f64,
    budget_reset_at: Option<DateTime<Utc>>,
    soft_budget_alerted_at: Option<DateTime<Utc>>,
    allowed_models: Option<String>,
    max_parallel_requests: Option<i64>,
    rpm_limit: Option<i64>,
    metadata: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const LIMITS_COLS: &str = "entity_type, entity_id, org_id, max_budget_usd, soft_budget_usd, budget_duration, \
    spend_usd, budget_reset_at, soft_budget_alerted_at, allowed_models, max_parallel_requests, rpm_limit, \
    metadata, created_at, updated_at";

fn limits_from_row(r: LimitsRow) -> Option<EntityLimits> {
    Some(EntityLimits {
        entity_type: EntityType::parse(&r.entity_type)?,
        entity_id: r.entity_id,
        org_id: r.org_id,
        max_budget_usd: r.max_budget_usd,
        soft_budget_usd: r.soft_budget_usd,
        budget_duration: r.budget_duration,
        spend_usd: r.spend_usd,
        budget_reset_at: r.budget_reset_at,
        soft_budget_alerted_at: r.soft_budget_alerted_at,
        allowed_models: r.allowed_models.as_deref().and_then(|s| serde_json::from_str(s).ok()),
        max_parallel_requests: r.max_parallel_requests.and_then(|v| u32::try_from(v).ok()),
        rpm_limit: r.rpm_limit.and_then(|v| u32::try_from(v).ok()),
        metadata: r.metadata.as_deref().and_then(|s| serde_json::from_str(s).ok()),
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

#[derive(sqlx::FromRow)]
struct EndUserRow {
    org_id: String,
    end_user_id: String,
    alias: Option<String>,
    blocked: bool,
    metadata: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_seen_at: Option<DateTime<Utc>>,
}

const END_USER_COLS: &str = "org_id, end_user_id, alias, blocked, metadata, created_at, updated_at, last_seen_at";

fn end_user_from_row(r: EndUserRow) -> EndUser {
    EndUser {
        id: r.end_user_id,
        org_id: r.org_id,
        alias: r.alias,
        blocked: r.blocked,
        metadata: r.metadata.as_deref().and_then(|s| serde_json::from_str(s).ok()),
        created_at: r.created_at,
        updated_at: r.updated_at,
        last_seen_at: r.last_seen_at,
    }
}

#[derive(sqlx::FromRow)]
struct SpendRow {
    grp: Option<String>,
    requests: i64,
    failed: i64,
    cost: f64,
}

#[derive(sqlx::FromRow)]
struct SpendLogRow {
    id: String,
    created_at: DateTime<Utc>,
    model: String,
    provider: String,
    media_type: String,
    status: String,
    cost_usd: f64,
    latency_ms: i64,
    org_id: Option<String>,
    app_id: Option<String>,
    key_id: Option<String>,
    user_id: Option<String>,
    end_user: Option<String>,
}

#[derive(sqlx::FromRow)]
struct RuntimeModelRow {
    id: String,
    schema_json: String,
    updated_by: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ProviderConfigRow {
    provider: String,
    ciphertext: Option<String>,
    nonce: Option<String>,
    display_hint: Option<String>,
    settings_json: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const PROVIDER_CONFIG_COLS: &str = "provider, ciphertext, nonce, display_hint, settings_json, created_at, updated_at";

fn provider_config_from_row(r: ProviderConfigRow) -> StoredProviderConfig {
    StoredProviderConfig {
        provider: r.provider,
        ciphertext: r.ciphertext,
        nonce: r.nonce,
        display_hint: r.display_hint,
        settings_json: r.settings_json,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}

/// The request-log column a spend report groups by (tag is a join, handled apart).
fn group_column(g: SpendGroupBy) -> &'static str {
    match g {
        SpendGroupBy::Model => "rl.model",
        SpendGroupBy::Provider => "rl.provider",
        SpendGroupBy::MediaType => "rl.media_type",
        SpendGroupBy::Status => "rl.status",
        SpendGroupBy::Org => "rl.org_id",
        SpendGroupBy::App => "rl.app_id",
        SpendGroupBy::Key => "rl.key_id",
        SpendGroupBy::User => "rl.user_id",
        SpendGroupBy::EndUser => "rl.end_user",
        SpendGroupBy::Day | SpendGroupBy::Tag => unreachable!("handled by the caller"),
    }
}

fn identity_sql(s: &str) -> String {
    s.to_string()
}

/// Rewrite `?` placeholders as Postgres `$1, $2, …`.
fn pg_sql(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut n = 0;
    for c in s.chars() {
        if c == '?' {
            n += 1;
            out.push('$');
            out.push_str(&n.to_string());
        } else {
            out.push(c);
        }
    }
    out
}

fn sqlite_ts(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn pg_ts(dt: DateTime<Utc>) -> DateTime<Utc> {
    dt
}

fn json_opt<T: serde::Serialize>(v: &Option<T>) -> Option<String> {
    v.as_ref().and_then(|v| serde_json::to_string(v).ok())
}

macro_rules! push_spend_filters {
    ($qb:expr, $f:expr, $ts:path) => {{
        let f = $f;
        $qb.push(" WHERE 1=1");
        if let Some(v) = &f.org_id { $qb.push(" AND rl.org_id = ").push_bind(v.clone()); }
        if let Some(v) = &f.app_id { $qb.push(" AND rl.app_id = ").push_bind(v.clone()); }
        if let Some(v) = f.from { $qb.push(" AND rl.created_at >= ").push_bind($ts(v)); }
        if let Some(v) = f.to { $qb.push(" AND rl.created_at <= ").push_bind($ts(v)); }
        if let Some(v) = &f.key_id { $qb.push(" AND rl.key_id = ").push_bind(v.clone()); }
        if let Some(v) = &f.user_id { $qb.push(" AND rl.user_id = ").push_bind(v.clone()); }
        if let Some(v) = &f.end_user { $qb.push(" AND rl.end_user = ").push_bind(v.clone()); }
        if let Some(v) = &f.model { $qb.push(" AND rl.model = ").push_bind(v.clone()); }
        if let Some(v) = &f.provider { $qb.push(" AND rl.provider = ").push_bind(v.clone()); }
        if let Some(v) = &f.tag {
            $qb.push(" AND EXISTS (SELECT 1 FROM request_log_tags ft WHERE ft.request_id = rl.id AND ft.tag = ")
                .push_bind(v.clone())
                .push(")");
        }
    }};
}

macro_rules! impl_management_store {
    ($ty:ty, $db:ty, $sql:path, $ts:path, $day_expr:expr) => {
        #[async_trait]
        impl ManagementStore for $ty {
            // ─── Limits ─────────────────────────────────────────────────
            async fn get_limits(&self, entity_type: EntityType, entity_id: &str) -> Result<Option<EntityLimits>, sqlx::Error> {
                let sql = $sql(&format!("SELECT {LIMITS_COLS} FROM entity_limits WHERE entity_type = ? AND entity_id = ?"));
                let row = sqlx::query_as::<_, LimitsRow>(&sql)
                    .bind(entity_type.as_str())
                    .bind(entity_id)
                    .fetch_optional(self.pool())
                    .await?;
                Ok(row.and_then(limits_from_row))
            }

            async fn list_limits(&self, org_id: Option<&str>, entity_type: Option<EntityType>) -> Result<Vec<EntityLimits>, sqlx::Error> {
                let mut qb = sqlx::QueryBuilder::<$db>::new(format!("SELECT {LIMITS_COLS} FROM entity_limits WHERE 1=1"));
                if let Some(o) = org_id { qb.push(" AND org_id = ").push_bind(o.to_string()); }
                if let Some(t) = entity_type { qb.push(" AND entity_type = ").push_bind(t.as_str()); }
                qb.push(" ORDER BY entity_type, entity_id");
                let rows: Vec<LimitsRow> = qb.build_query_as().fetch_all(self.pool()).await?;
                Ok(rows.into_iter().filter_map(limits_from_row).collect())
            }

            async fn upsert_limits(&self, l: &EntityLimits) -> Result<(), sqlx::Error> {
                let sql = $sql(
                    "INSERT INTO entity_limits (entity_type, entity_id, org_id, max_budget_usd, soft_budget_usd, \
                     budget_duration, spend_usd, budget_reset_at, soft_budget_alerted_at, allowed_models, \
                     max_parallel_requests, rpm_limit, metadata, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT (entity_type, entity_id) DO UPDATE SET \
                     org_id = excluded.org_id, max_budget_usd = excluded.max_budget_usd, \
                     soft_budget_usd = excluded.soft_budget_usd, budget_duration = excluded.budget_duration, \
                     spend_usd = excluded.spend_usd, budget_reset_at = excluded.budget_reset_at, \
                     soft_budget_alerted_at = excluded.soft_budget_alerted_at, allowed_models = excluded.allowed_models, \
                     max_parallel_requests = excluded.max_parallel_requests, rpm_limit = excluded.rpm_limit, \
                     metadata = excluded.metadata, updated_at = excluded.updated_at",
                );
                sqlx::query(&sql)
                    .bind(l.entity_type.as_str())
                    .bind(&l.entity_id)
                    .bind(&l.org_id)
                    .bind(l.max_budget_usd)
                    .bind(l.soft_budget_usd)
                    .bind(&l.budget_duration)
                    .bind(l.spend_usd)
                    .bind(l.budget_reset_at)
                    .bind(l.soft_budget_alerted_at)
                    .bind(json_opt(&l.allowed_models))
                    .bind(l.max_parallel_requests.map(i64::from))
                    .bind(l.rpm_limit.map(i64::from))
                    .bind(json_opt(&l.metadata))
                    .bind(l.created_at)
                    .bind(l.updated_at)
                    .execute(self.pool())
                    .await?;
                Ok(())
            }

            async fn delete_limits(&self, entity_type: EntityType, entity_id: &str) -> Result<bool, sqlx::Error> {
                let sql = $sql("DELETE FROM entity_limits WHERE entity_type = ? AND entity_id = ?");
                let r = sqlx::query(&sql).bind(entity_type.as_str()).bind(entity_id).execute(self.pool()).await?;
                Ok(r.rows_affected() > 0)
            }

            async fn roll_limits_window(
                &self,
                entity_type: EntityType,
                entity_id: &str,
                expected_reset_at: Option<DateTime<Utc>>,
                next_reset_at: Option<DateTime<Utc>>,
            ) -> Result<bool, sqlx::Error> {
                // `IS` compares NULL-safely on SQLite; Postgres spells it IS NOT DISTINCT FROM.
                let cmp = if stringify!($db).contains("Postgres") { "IS NOT DISTINCT FROM" } else { "IS" };
                let sql = $sql(&format!(
                    "UPDATE entity_limits SET spend_usd = 0, soft_budget_alerted_at = NULL, budget_reset_at = ?, updated_at = ? \
                     WHERE entity_type = ? AND entity_id = ? AND budget_reset_at {cmp} ?"
                ));
                let r = sqlx::query(&sql)
                    .bind(next_reset_at)
                    .bind(Utc::now())
                    .bind(entity_type.as_str())
                    .bind(entity_id)
                    .bind(expected_reset_at)
                    .execute(self.pool())
                    .await?;
                Ok(r.rows_affected() > 0)
            }

            async fn reserve_limits_spend(&self, entity_type: EntityType, entity_id: &str, amount: f64) -> Result<bool, sqlx::Error> {
                let sql = $sql(
                    "UPDATE entity_limits SET spend_usd = spend_usd + ? \
                     WHERE entity_type = ? AND entity_id = ? \
                     AND (max_budget_usd IS NULL OR spend_usd + ? <= max_budget_usd)",
                );
                let r = sqlx::query(&sql)
                    .bind(amount)
                    .bind(entity_type.as_str())
                    .bind(entity_id)
                    .bind(amount)
                    .execute(self.pool())
                    .await?;
                Ok(r.rows_affected() == 1)
            }

            async fn charge_limits_spend(&self, entity_type: EntityType, entity_id: &str, delta: f64) -> Result<Option<f64>, sqlx::Error> {
                let sql = $sql(
                    "UPDATE entity_limits SET spend_usd = spend_usd + ? \
                     WHERE entity_type = ? AND entity_id = ? RETURNING spend_usd",
                );
                let row: Option<(f64,)> = sqlx::query_as(&sql)
                    .bind(delta)
                    .bind(entity_type.as_str())
                    .bind(entity_id)
                    .fetch_optional(self.pool())
                    .await?;
                Ok(row.map(|r| r.0))
            }

            async fn mark_soft_budget_alerted(&self, entity_type: EntityType, entity_id: &str, at: DateTime<Utc>) -> Result<bool, sqlx::Error> {
                let sql = $sql(
                    "UPDATE entity_limits SET soft_budget_alerted_at = ? \
                     WHERE entity_type = ? AND entity_id = ? AND soft_budget_alerted_at IS NULL",
                );
                let r = sqlx::query(&sql).bind(at).bind(entity_type.as_str()).bind(entity_id).execute(self.pool()).await?;
                Ok(r.rows_affected() > 0)
            }

            // ─── End users ──────────────────────────────────────────────
            async fn get_end_user(&self, org_id: &str, end_user_id: &str) -> Result<Option<EndUser>, sqlx::Error> {
                let sql = $sql(&format!("SELECT {END_USER_COLS} FROM end_users WHERE org_id = ? AND end_user_id = ?"));
                let row = sqlx::query_as::<_, EndUserRow>(&sql).bind(org_id).bind(end_user_id).fetch_optional(self.pool()).await?;
                Ok(row.map(end_user_from_row))
            }

            async fn list_end_users(&self, org_id: &str) -> Result<Vec<EndUser>, sqlx::Error> {
                let sql = $sql(&format!("SELECT {END_USER_COLS} FROM end_users WHERE org_id = ? ORDER BY end_user_id"));
                let rows = sqlx::query_as::<_, EndUserRow>(&sql).bind(org_id).fetch_all(self.pool()).await?;
                Ok(rows.into_iter().map(end_user_from_row).collect())
            }

            async fn upsert_end_user(&self, u: &EndUser) -> Result<(), sqlx::Error> {
                let sql = $sql(
                    "INSERT INTO end_users (org_id, end_user_id, alias, blocked, metadata, created_at, updated_at, last_seen_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT (org_id, end_user_id) DO UPDATE SET alias = excluded.alias, blocked = excluded.blocked, \
                     metadata = excluded.metadata, updated_at = excluded.updated_at, last_seen_at = excluded.last_seen_at",
                );
                sqlx::query(&sql)
                    .bind(&u.org_id)
                    .bind(&u.id)
                    .bind(&u.alias)
                    .bind(u.blocked)
                    .bind(json_opt(&u.metadata))
                    .bind(u.created_at)
                    .bind(u.updated_at)
                    .bind(u.last_seen_at)
                    .execute(self.pool())
                    .await?;
                Ok(())
            }

            async fn delete_end_user(&self, org_id: &str, end_user_id: &str) -> Result<bool, sqlx::Error> {
                let sql = $sql("DELETE FROM end_users WHERE org_id = ? AND end_user_id = ?");
                let r = sqlx::query(&sql).bind(org_id).bind(end_user_id).execute(self.pool()).await?;
                Ok(r.rows_affected() > 0)
            }

            async fn touch_end_user(&self, org_id: &str, end_user_id: &str, at: DateTime<Utc>) -> Result<(), sqlx::Error> {
                let sql = $sql(
                    "INSERT INTO end_users (org_id, end_user_id, blocked, created_at, updated_at, last_seen_at) \
                     VALUES (?, ?, ?, ?, ?, ?) \
                     ON CONFLICT (org_id, end_user_id) DO UPDATE SET last_seen_at = excluded.last_seen_at",
                );
                sqlx::query(&sql)
                    .bind(org_id)
                    .bind(end_user_id)
                    .bind(false)
                    .bind(at)
                    .bind(at)
                    .bind(at)
                    .execute(self.pool())
                    .await?;
                Ok(())
            }

            // ─── Spend ──────────────────────────────────────────────────
            async fn attribute_request_log(&self, request_id: &str, a: &Attribution) -> Result<(), sqlx::Error> {
                let sql = $sql("UPDATE request_logs SET key_id = ?, user_id = ?, end_user = ? WHERE id = ?");
                sqlx::query(&sql)
                    .bind(&a.key_id)
                    .bind(&a.user_id)
                    .bind(&a.end_user)
                    .bind(request_id)
                    .execute(self.pool())
                    .await?;
                if !a.tags.is_empty() {
                    let ins = $sql(
                        "INSERT INTO request_log_tags (request_id, tag) VALUES (?, ?) ON CONFLICT (request_id, tag) DO NOTHING",
                    );
                    for tag in &a.tags {
                        sqlx::query(&ins).bind(request_id).bind(tag).execute(self.pool()).await?;
                    }
                }
                Ok(())
            }

            async fn spend_report(&self, filter: &SpendFilter, group_by: SpendGroupBy) -> Result<Vec<SpendReportRow>, sqlx::Error> {
                let (select, join, order) = match group_by {
                    SpendGroupBy::Day => (format!("{} AS grp", $day_expr), "", "grp ASC"),
                    SpendGroupBy::Tag => (
                        "t.tag AS grp".to_string(),
                        " JOIN request_log_tags t ON t.request_id = rl.id",
                        "cost DESC, grp ASC",
                    ),
                    g => (format!("{} AS grp", group_column(g)), "", "cost DESC, grp ASC"),
                };
                let mut qb = sqlx::QueryBuilder::<$db>::new(format!(
                    "SELECT {select}, COUNT(*) AS requests, \
                     COALESCE(SUM(CASE WHEN rl.status = 'failed' THEN 1 ELSE 0 END), 0) AS failed, \
                     COALESCE(SUM(rl.cost_usd), 0.0) AS cost \
                     FROM request_logs rl{join}"
                ));
                push_spend_filters!(qb, filter, $ts);
                qb.push(format!(" GROUP BY grp ORDER BY {order} LIMIT 1000"));
                let rows: Vec<SpendRow> = qb.build_query_as().fetch_all(self.pool()).await?;
                Ok(rows
                    .into_iter()
                    .map(|r| SpendReportRow {
                        group: r.grp,
                        requests: r.requests.max(0) as u64,
                        failed_requests: r.failed.max(0) as u64,
                        cost_usd: r.cost,
                    })
                    .collect())
            }

            async fn spend_logs(&self, filter: &SpendFilter, page: u32, per_page: u32) -> Result<(Vec<SpendLogEntry>, u64), sqlx::Error> {
                let mut cq = sqlx::QueryBuilder::<$db>::new("SELECT COUNT(*) FROM request_logs rl");
                push_spend_filters!(cq, filter, $ts);
                let (total,): (i64,) = cq.build_query_as().fetch_one(self.pool()).await?;

                let offset = crate::api::pagination::offset(page, per_page) as i64;
                let mut qb = sqlx::QueryBuilder::<$db>::new(
                    "SELECT rl.id, rl.created_at, rl.model, rl.provider, rl.media_type, rl.status, rl.cost_usd, \
                     rl.latency_ms, rl.org_id, rl.app_id, rl.key_id, rl.user_id, rl.end_user FROM request_logs rl",
                );
                push_spend_filters!(qb, filter, $ts);
                qb.push(" ORDER BY rl.created_at DESC, rl.id DESC LIMIT ")
                    .push_bind(per_page as i64)
                    .push(" OFFSET ")
                    .push_bind(offset);
                let rows: Vec<SpendLogRow> = qb.build_query_as().fetch_all(self.pool()).await?;

                let mut tags: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
                if !rows.is_empty() {
                    let mut tq = sqlx::QueryBuilder::<$db>::new("SELECT request_id, tag FROM request_log_tags WHERE request_id IN (");
                    let mut sep = tq.separated(", ");
                    for r in &rows {
                        sep.push_bind(r.id.clone());
                    }
                    sep.push_unseparated(") ORDER BY tag");
                    let pairs: Vec<(String, String)> = tq.build_query_as().fetch_all(self.pool()).await?;
                    for (rid, tag) in pairs {
                        tags.entry(rid).or_default().push(tag);
                    }
                }

                let data = rows
                    .into_iter()
                    .map(|r| SpendLogEntry {
                        tags: tags.remove(&r.id).unwrap_or_default(),
                        id: r.id,
                        created_at: r.created_at,
                        model: r.model,
                        provider: r.provider,
                        media_type: r.media_type,
                        status: r.status,
                        cost_usd: r.cost_usd,
                        latency_ms: r.latency_ms,
                        org_id: r.org_id,
                        app_id: r.app_id,
                        key_id: r.key_id,
                        user_id: r.user_id,
                        end_user: r.end_user,
                    })
                    .collect();
                Ok((data, total.max(0) as u64))
            }

            // ─── Runtime models ─────────────────────────────────────────
            async fn list_runtime_models(&self) -> Result<Vec<StoredRuntimeModel>, sqlx::Error> {
                let rows = sqlx::query_as::<_, RuntimeModelRow>(
                    "SELECT id, schema_json, updated_by, created_at, updated_at FROM runtime_models ORDER BY id",
                )
                .fetch_all(self.pool())
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| StoredRuntimeModel {
                        id: r.id,
                        schema_json: r.schema_json,
                        updated_by: r.updated_by,
                        created_at: r.created_at,
                        updated_at: r.updated_at,
                    })
                    .collect())
            }

            async fn upsert_runtime_model(&self, id: &str, schema_json: &str, updated_by: Option<&str>) -> Result<(), sqlx::Error> {
                let now = Utc::now();
                let sql = $sql(
                    "INSERT INTO runtime_models (id, schema_json, updated_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?) \
                     ON CONFLICT (id) DO UPDATE SET schema_json = excluded.schema_json, \
                     updated_by = excluded.updated_by, updated_at = excluded.updated_at",
                );
                sqlx::query(&sql).bind(id).bind(schema_json).bind(updated_by).bind(now).bind(now).execute(self.pool()).await?;
                Ok(())
            }

            async fn delete_runtime_model(&self, id: &str) -> Result<bool, sqlx::Error> {
                let sql = $sql("DELETE FROM runtime_models WHERE id = ?");
                let r = sqlx::query(&sql).bind(id).execute(self.pool()).await?;
                Ok(r.rows_affected() > 0)
            }

            async fn list_model_states(&self) -> Result<Vec<(String, bool)>, sqlx::Error> {
                sqlx::query_as::<_, (String, bool)>("SELECT model_id, disabled FROM model_states ORDER BY model_id")
                    .fetch_all(self.pool())
                    .await
            }

            async fn set_model_state(&self, model_id: &str, disabled: bool) -> Result<(), sqlx::Error> {
                let sql = $sql(
                    "INSERT INTO model_states (model_id, disabled, updated_at) VALUES (?, ?, ?) \
                     ON CONFLICT (model_id) DO UPDATE SET disabled = excluded.disabled, updated_at = excluded.updated_at",
                );
                sqlx::query(&sql).bind(model_id).bind(disabled).bind(Utc::now()).execute(self.pool()).await?;
                Ok(())
            }

            // ─── Provider configs ───────────────────────────────────────
            async fn list_provider_configs(&self) -> Result<Vec<StoredProviderConfig>, sqlx::Error> {
                let rows = sqlx::query_as::<_, ProviderConfigRow>(&format!(
                    "SELECT {PROVIDER_CONFIG_COLS} FROM provider_configs ORDER BY provider"
                ))
                .fetch_all(self.pool())
                .await?;
                Ok(rows.into_iter().map(provider_config_from_row).collect())
            }

            async fn get_provider_config(&self, provider: &str) -> Result<Option<StoredProviderConfig>, sqlx::Error> {
                let sql = $sql(&format!("SELECT {PROVIDER_CONFIG_COLS} FROM provider_configs WHERE provider = ?"));
                let row = sqlx::query_as::<_, ProviderConfigRow>(&sql).bind(provider).fetch_optional(self.pool()).await?;
                Ok(row.map(provider_config_from_row))
            }

            async fn upsert_provider_config(&self, c: &StoredProviderConfig) -> Result<(), sqlx::Error> {
                let sql = $sql(
                    "INSERT INTO provider_configs (provider, ciphertext, nonce, display_hint, settings_json, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT (provider) DO UPDATE SET ciphertext = excluded.ciphertext, nonce = excluded.nonce, \
                     display_hint = excluded.display_hint, settings_json = excluded.settings_json, updated_at = excluded.updated_at",
                );
                sqlx::query(&sql)
                    .bind(&c.provider)
                    .bind(&c.ciphertext)
                    .bind(&c.nonce)
                    .bind(&c.display_hint)
                    .bind(&c.settings_json)
                    .bind(c.created_at)
                    .bind(c.updated_at)
                    .execute(self.pool())
                    .await?;
                Ok(())
            }

            async fn delete_provider_config(&self, provider: &str) -> Result<bool, sqlx::Error> {
                let sql = $sql("DELETE FROM provider_configs WHERE provider = ?");
                let r = sqlx::query(&sql).bind(provider).execute(self.pool()).await?;
                Ok(r.rows_affected() > 0)
            }

            // ─── Gateway settings ───────────────────────────────────────
            async fn get_setting(&self, key: &str) -> Result<Option<String>, sqlx::Error> {
                let sql = $sql("SELECT value_json FROM gateway_settings WHERE key = ?");
                let row: Option<(String,)> = sqlx::query_as(&sql).bind(key).fetch_optional(self.pool()).await?;
                Ok(row.map(|r| r.0))
            }

            async fn set_setting(&self, key: &str, value_json: &str) -> Result<(), sqlx::Error> {
                let sql = $sql(
                    "INSERT INTO gateway_settings (key, value_json, updated_at) VALUES (?, ?, ?) \
                     ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
                );
                sqlx::query(&sql).bind(key).bind(value_json).bind(Utc::now()).execute(self.pool()).await?;
                Ok(())
            }

            async fn delete_setting(&self, key: &str) -> Result<bool, sqlx::Error> {
                let sql = $sql("DELETE FROM gateway_settings WHERE key = ?");
                let r = sqlx::query(&sql).bind(key).execute(self.pool()).await?;
                Ok(r.rows_affected() > 0)
            }
        }
    };
}

impl_management_store!(SqliteDatabase, sqlx::Sqlite, identity_sql, sqlite_ts, "substr(rl.created_at, 1, 10)");
impl_management_store!(
    PostgresDatabase,
    sqlx::Postgres,
    pg_sql,
    pg_ts,
    "to_char(rl.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD')"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_placeholders_are_numbered_in_order() {
        assert_eq!(pg_sql("a = ? AND b = ? OR c = ?"), "a = $1 AND b = $2 OR c = $3");
        assert_eq!(pg_sql("no params"), "no params");
    }

    #[test]
    fn sqlite_timestamps_match_the_request_log_format() {
        use chrono::TimeZone;
        let t = Utc.with_ymd_and_hms(2026, 10, 12, 8, 5, 9).unwrap();
        assert_eq!(sqlite_ts(t), "2026-10-12 08:05:09");
    }
}
