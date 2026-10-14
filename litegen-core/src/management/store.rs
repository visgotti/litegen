//! Persistence for the management API.
//!
//! A separate trait from [`DatabaseStore`](crate::db::DatabaseStore) so the
//! dozens of test doubles that implement that trait keep compiling: a store
//! that supports management returns `Some(self)` from
//! `DatabaseStore::management`, everything else returns `None` and the
//! management endpoints answer 501.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::types::*;

#[async_trait]
pub trait ManagementStore: Send + Sync {
    // ─── Limits ─────────────────────────────────────────────────────────
    async fn get_limits(&self, entity_type: EntityType, entity_id: &str) -> Result<Option<EntityLimits>, sqlx::Error>;
    /// Every limits row, optionally narrowed to one org and/or entity type.
    async fn list_limits(&self, org_id: Option<&str>, entity_type: Option<EntityType>) -> Result<Vec<EntityLimits>, sqlx::Error>;
    /// Insert or replace a row's configuration. `spend_usd`, `budget_reset_at`
    /// and `soft_budget_alerted_at` are written as given, so callers decide
    /// whether a window restarts.
    async fn upsert_limits(&self, limits: &EntityLimits) -> Result<(), sqlx::Error>;
    async fn delete_limits(&self, entity_type: EntityType, entity_id: &str) -> Result<bool, sqlx::Error>;
    /// Start a new window: zero spend, clear the soft alert, set the next reset.
    /// Conditional on the row still having `expected_reset_at`, so two callers
    /// that both saw the lapsed window reset it exactly once.
    async fn roll_limits_window(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        expected_reset_at: Option<DateTime<Utc>>,
        next_reset_at: Option<DateTime<Utc>>,
    ) -> Result<bool, sqlx::Error>;
    /// Atomically add `amount` to spend unless that would pass the hard cap.
    /// False when the cap would be exceeded or the row does not exist.
    async fn reserve_limits_spend(&self, entity_type: EntityType, entity_id: &str, amount: f64) -> Result<bool, sqlx::Error>;
    /// Add `delta` (negative refunds) to spend, unconditionally; returns the new spend.
    async fn charge_limits_spend(&self, entity_type: EntityType, entity_id: &str, delta: f64) -> Result<Option<f64>, sqlx::Error>;
    /// Record the soft-budget alert for this window. False if already recorded.
    async fn mark_soft_budget_alerted(&self, entity_type: EntityType, entity_id: &str, at: DateTime<Utc>) -> Result<bool, sqlx::Error>;

    // ─── End users ──────────────────────────────────────────────────────
    async fn get_end_user(&self, org_id: &str, end_user_id: &str) -> Result<Option<EndUser>, sqlx::Error>;
    async fn list_end_users(&self, org_id: &str) -> Result<Vec<EndUser>, sqlx::Error>;
    /// Insert or replace an end user (all fields as given).
    async fn upsert_end_user(&self, end_user: &EndUser) -> Result<(), sqlx::Error>;
    async fn delete_end_user(&self, org_id: &str, end_user_id: &str) -> Result<bool, sqlx::Error>;
    /// Create the end user on first sight, else bump `last_seen_at`.
    async fn touch_end_user(&self, org_id: &str, end_user_id: &str, at: DateTime<Utc>) -> Result<(), sqlx::Error>;

    // ─── Spend ──────────────────────────────────────────────────────────
    /// Stamp who a request-log row belongs to. A no-op for an unknown id.
    async fn attribute_request_log(&self, request_id: &str, attribution: &Attribution) -> Result<(), sqlx::Error>;
    async fn spend_report(&self, filter: &SpendFilter, group_by: SpendGroupBy) -> Result<Vec<SpendReportRow>, sqlx::Error>;
    async fn spend_logs(&self, filter: &SpendFilter, page: u32, per_page: u32) -> Result<(Vec<SpendLogEntry>, u64), sqlx::Error>;

    // ─── Runtime models ─────────────────────────────────────────────────
    async fn list_runtime_models(&self) -> Result<Vec<StoredRuntimeModel>, sqlx::Error>;
    async fn upsert_runtime_model(&self, id: &str, schema_json: &str, updated_by: Option<&str>) -> Result<(), sqlx::Error>;
    async fn delete_runtime_model(&self, id: &str) -> Result<bool, sqlx::Error>;
    /// Every model id with an explicit state, as (id, disabled).
    async fn list_model_states(&self) -> Result<Vec<(String, bool)>, sqlx::Error>;
    async fn set_model_state(&self, model_id: &str, disabled: bool) -> Result<(), sqlx::Error>;

    // ─── Provider configs ───────────────────────────────────────────────
    async fn list_provider_configs(&self) -> Result<Vec<StoredProviderConfig>, sqlx::Error>;
    async fn get_provider_config(&self, provider: &str) -> Result<Option<StoredProviderConfig>, sqlx::Error>;
    async fn upsert_provider_config(&self, config: &StoredProviderConfig) -> Result<(), sqlx::Error>;
    async fn delete_provider_config(&self, provider: &str) -> Result<bool, sqlx::Error>;

    // ─── Gateway settings ───────────────────────────────────────────────
    async fn get_setting(&self, key: &str) -> Result<Option<String>, sqlx::Error>;
    async fn set_setting(&self, key: &str, value_json: &str) -> Result<(), sqlx::Error>;
    async fn delete_setting(&self, key: &str) -> Result<bool, sqlx::Error>;
}
