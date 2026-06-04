use async_trait::async_trait;
use uuid::Uuid;

use crate::types::*;

/// Trait defining the database operations. Implementations exist for SQLite and PostgreSQL.
#[async_trait]
#[allow(clippy::too_many_arguments)] // DB trait methods need many params; would need wrapper structs to reduce
pub trait DatabaseStore: Send + Sync {
    // ─── Generations ────────────────────────────────────────────────────────────

    /// Insert a new generation row when a video generation is submitted.
    /// `org_id`/`app_id` stamp the tenant; pass `None` when there is no tenant context.
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
    ) -> Result<(), sqlx::Error>;

    /// Update the status (and related fields) of an existing generation.
    async fn update_generation_status(
        &self,
        id: &str,
        status: &str,
        progress: i32,
        result_url: Option<&str>,
        error: Option<&str>,
        completed_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), sqlx::Error>;

    /// Fetch a generation by its local ID.
    async fn get_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error>;

    /// List pending/processing generations for the background poller (capped at `limit`).
    async fn list_active_generations(&self, limit: u32) -> Result<Vec<Generation>, sqlx::Error>;

    /// Paginated list of generations.
    /// When `key_id` is `Some`, returns rows owned by that key OR rows with `key_id IS NULL`
    /// (master-key rows). When `key_id` is `None` (caller is master key), returns all rows.
    // superseded by list_generations_for_tenant for the API (kept for tests/back-compat)
    async fn list_generations(
        &self,
        key_id: Option<&Uuid>,
        page: u32,
        per_page: u32,
    ) -> Result<Vec<Generation>, sqlx::Error>;

    /// Count of generations, with the same ownership filter as `list_generations`.
    // superseded by count_generations_for_tenant for the API (kept for tests/back-compat)
    async fn count_generations(&self, key_id: Option<&Uuid>) -> Result<i64, sqlx::Error>;

    /// Soft-cancel a generation: sets `status = 'cancelled'` and `completed_at = NOW()`.
    /// Only rows with `status IN ('pending', 'processing')` are affected.
    /// Returns the updated row, or `None` if the id doesn't exist or isn't cancellable.
    async fn cancel_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error>;

    // ─── Request Logs ───────────────────────────────────────────────────

    /// `org_id`/`app_id` stamp the tenant; pass `None` when there is no tenant context.
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
    ) -> Result<(), sqlx::Error>;

    // superseded by get_request_logs_for_tenant for the API (kept for tests/back-compat)
    async fn get_request_logs(
        &self,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error>;

    /// Filtered, paginated request logs. Any `None` filter field is ignored.
    // superseded by get_request_logs_for_tenant for the API (kept for tests/back-compat)
    async fn get_request_logs_filtered(
        &self,
        filters: &LogFilters,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error>;

    // ─── API Keys ───────────────────────────────────────────────────────

    async fn create_api_key(
        &self,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
        token_quota: Option<f64>,
        rpm_limit: Option<u32>,
        scopes: &str,
        webhook_url: Option<&str>,
    ) -> Result<ApiKey, sqlx::Error>;

    async fn get_api_key(&self, id: &Uuid) -> Result<Option<ApiKey>, sqlx::Error>;

    async fn update_api_key(
        &self,
        id: &Uuid,
        req: &UpdateApiKeyRequest,
    ) -> Result<Option<ApiKey>, sqlx::Error>;

    /// Look up a key by its SHA-256 hash. Only returns active, non-expired keys.
    async fn lookup_api_key_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error>;

    /// Atomically add `cost_usd` to `tokens_used`.
    /// Returns the new `tokens_used` value.
    /// Returns `sqlx::Error::RowNotFound` if the key doesn't exist.
    async fn atomic_charge_tokens(
        &self,
        id: &Uuid,
        cost_usd: f64,
    ) -> Result<f64, sqlx::Error>;

    async fn validate_api_key(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error>;

    async fn list_api_keys(&self) -> Result<Vec<ApiKey>, sqlx::Error>;

    /// List API keys owned by a specific user.
    async fn list_api_keys_for_owner(&self, _owner_user_id: &str) -> Result<Vec<ApiKey>, sqlx::Error> {
        Ok(vec![])
    }

    /// Set the owner_user_id of a key (called after creation for session-authed users).
    async fn set_api_key_owner(&self, _id: &Uuid, _owner_user_id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn revoke_api_key(&self, id: &Uuid) -> Result<bool, sqlx::Error>;

    /// Rotate an existing key's secret in place: replace `key_hash`/`key_prefix`
    /// (and `public_id`) on the same row id, keeping all other settings.
    /// Returns the updated key, or `None` if the id doesn't exist.
    /// Default impl returns None (for mock/test implementations).
    async fn rotate_api_key(
        &self,
        _id: &Uuid,
        _public_id: &str,
        _key_hash: &str,
        _key_prefix: &str,
    ) -> Result<Option<ApiKey>, sqlx::Error> {
        Ok(None)
    }

    // ─── Stats ──────────────────────────────────────────────────────────

    // superseded by get_stats_for_tenant for the API (kept for tests/back-compat)
    async fn get_stats(&self) -> Result<ProxyStats, sqlx::Error>;

    /// Tenant-scoped aggregate stats: same shape as `get_stats` but restricted to
    /// `org_id` (and `app_id` when `Some`). Default impl returns empty stats so
    /// mock/test implementations still compile.
    async fn get_stats_for_tenant(
        &self,
        _org_id: &str,
        _app_id: Option<&str>,
    ) -> Result<ProxyStats, sqlx::Error> {
        Ok(ProxyStats {
            total_requests: 0,
            successful_requests: 0,
            failed_requests: 0,
            total_cost_usd: 0.0,
            avg_latency_ms: 0.0,
            requests_per_minute: 0.0,
            models_used: Vec::new(),
            providers_used: Vec::new(),
            latency_percentiles: LatencyPercentiles {
                p50_ms: 0.0,
                p95_ms: 0.0,
                p99_ms: 0.0,
                sample_count: 0,
                window_minutes: 60,
            },
        })
    }

    /// Compute latency percentiles (p50/p95/p99) for completed requests in the
    /// last `since_minutes` minutes.  Capped at 10 000 samples.
    /// Default impl returns zeroed percentiles (for mock/test implementations).
    async fn latency_percentiles(&self, since_minutes: i64) -> Result<LatencyPercentiles, sqlx::Error> {
        Ok(LatencyPercentiles { p50_ms: 0.0, p95_ms: 0.0, p99_ms: 0.0, sample_count: 0, window_minutes: since_minutes })
    }

    // ─── Audit Log ──────────────────────────────────────────────────────

    /// Insert a single audit log entry.
    /// Default impl is a no-op (for mock/test implementations).
    async fn insert_audit_log(&self, _entry: &AuditLogEntry) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Paginated list of audit log entries with optional filters.
    /// Returns `(entries, total_count)`.
    /// Default impl returns an empty list (for mock/test implementations).
    async fn list_audit_log(
        &self,
        _filter: &AuditLogFilter,
        _page: u32,
        _per_page: u32,
    ) -> Result<(Vec<AuditLogEntry>, i64), sqlx::Error> {
        Ok((Vec::new(), 0))
    }

    // ─── Webhook Delivery Log ────────────────────────────────────────────

    /// Record one webhook delivery attempt.
    /// Default impl is a no-op (for mock/test implementations).
    async fn insert_webhook_delivery(&self, _delivery: &WebhookDelivery) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Paginated list of webhook deliveries for a specific key.
    /// Returns `(deliveries, total_count)`.
    /// Default impl returns an empty list (for mock/test implementations).
    async fn list_webhook_deliveries(
        &self,
        _key_id: &str,
        _page: u32,
        _per_page: u32,
    ) -> Result<(Vec<WebhookDelivery>, i64), sqlx::Error> {
        Ok((Vec::new(), 0))
    }

    // ─── Request Artifacts ──────────────────────────────────────────────

    /// Store a request artifact (fire-and-forget; spawned async).
    /// Default impl is a no-op so test mocks don't break.
    async fn insert_request_artifact(&self, _a: &RequestArtifact) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Look up a request artifact by its request_id.
    /// Default impl returns None (for mock/test implementations).
    async fn get_request_artifact(&self, _request_id: &str) -> Result<Option<RequestArtifact>, sqlx::Error> {
        Ok(None)
    }

    // ─── Liveness ───────────────────────────────────────────────────────

    /// Liveness ping: execute `SELECT 1`. Returns Ok(()) if DB is reachable.
    /// Default impl returns Ok(()) (for mock/test implementations).
    async fn ping(&self) -> Result<(), sqlx::Error> {
        Ok(())
    }

    // ─── Users ──────────────────────────────────────────────────────────

    async fn create_user(&self, _user: &User) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_user_by_email(&self, _email: &str) -> Result<Option<User>, sqlx::Error> {
        Ok(None)
    }

    async fn get_user_by_id(&self, _id: &str) -> Result<Option<User>, sqlx::Error> {
        Ok(None)
    }

    async fn get_user_by_oauth(&self, _provider: &str, _oauth_id: &str) -> Result<Option<User>, sqlx::Error> {
        Ok(None)
    }

    async fn update_user(
        &self,
        _id: &str,
        _role: Option<Role>,
        _is_active: Option<bool>,
        _password_hash: Option<&str>,
    ) -> Result<Option<User>, sqlx::Error> {
        Ok(None)
    }

    async fn touch_last_login(&self, _id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn count_users(&self) -> Result<i64, sqlx::Error> {
        Ok(0)
    }

    async fn list_users(&self) -> Result<Vec<User>, sqlx::Error> {
        Ok(vec![])
    }

    async fn link_oauth(&self, _user_id: &str, _provider: &str, _oauth_id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn transfer_owner(&self, _new_owner_id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    // ─── Sessions ───────────────────────────────────────────────────────

    async fn create_session(&self, _s: &Session) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_session(&self, _id: &str) -> Result<Option<Session>, sqlx::Error> {
        Ok(None)
    }

    async fn delete_session(&self, _id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn delete_user_sessions(&self, _user_id: &str, _except_id: Option<&str>) -> Result<u64, sqlx::Error> {
        Ok(0)
    }

    async fn list_user_sessions(&self, _user_id: &str) -> Result<Vec<Session>, sqlx::Error> {
        Ok(vec![])
    }

    async fn bump_session_expiry(
        &self,
        _id: &str,
        _new_expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    // ─── Invitations ────────────────────────────────────────────────────

    async fn create_invitation(&self, _inv: &Invitation) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_invitation(&self, _token: &str) -> Result<Option<Invitation>, sqlx::Error> {
        Ok(None)
    }

    async fn mark_invitation_used(&self, _token: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn delete_invitation(&self, _id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn list_invitations(&self) -> Result<Vec<Invitation>, sqlx::Error> {
        Ok(vec![])
    }

    // ─── Password Resets ────────────────────────────────────────────────

    async fn create_password_reset(&self, _r: &PasswordReset) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_password_reset(&self, _token: &str) -> Result<Option<PasswordReset>, sqlx::Error> {
        Ok(None)
    }

    async fn mark_password_reset_used(&self, _token: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    // ─── Login Attempts ─────────────────────────────────────────────────

    async fn record_login_attempt(&self, _email: &str, _success: bool) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn recent_failed_login_attempts(
        &self,
        _email: &str,
        _since: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
        Ok(vec![])
    }

    // ─── Organizations ──────────────────────────────────────────────────
    // Default impls return empty/Ok/None/false so mock/test implementations
    // (and backends not yet migrated) still compile.

    async fn create_organization(&self, _o: &Organization) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_organization(&self, _id: &str) -> Result<Option<Organization>, sqlx::Error> {
        Ok(None)
    }

    async fn get_org_by_slug(&self, _slug: &str) -> Result<Option<Organization>, sqlx::Error> {
        Ok(None)
    }

    /// Organizations the user belongs to, paired with their role in each.
    async fn list_orgs_for_user(
        &self,
        _user_id: &str,
    ) -> Result<Vec<(Organization, Role)>, sqlx::Error> {
        Ok(vec![])
    }

    async fn update_organization(
        &self,
        _id: &str,
        _name: Option<&str>,
    ) -> Result<Option<Organization>, sqlx::Error> {
        Ok(None)
    }

    async fn delete_organization(&self, _id: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    // ─── Members ────────────────────────────────────────────────────────

    async fn add_org_member(
        &self,
        _org_id: &str,
        _user_id: &str,
        _role: Role,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_membership(
        &self,
        _org_id: &str,
        _user_id: &str,
    ) -> Result<Option<Role>, sqlx::Error> {
        Ok(None)
    }

    async fn list_org_members(&self, _org_id: &str) -> Result<Vec<OrganizationMember>, sqlx::Error> {
        Ok(vec![])
    }

    async fn update_member_role(
        &self,
        _org_id: &str,
        _user_id: &str,
        _role: Role,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn remove_org_member(&self, _org_id: &str, _user_id: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Demote the current owner(s) to admin and promote `new_owner_user_id` to owner.
    async fn transfer_org_owner(
        &self,
        _org_id: &str,
        _new_owner_user_id: &str,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    // ─── Applications ───────────────────────────────────────────────────

    async fn create_application(&self, _a: &Application) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_application(&self, _id: &str) -> Result<Option<Application>, sqlx::Error> {
        Ok(None)
    }

    async fn list_apps_for_org(&self, _org_id: &str) -> Result<Vec<Application>, sqlx::Error> {
        Ok(vec![])
    }

    async fn update_application(
        &self,
        _id: &str,
        _name: Option<&str>,
    ) -> Result<Option<Application>, sqlx::Error> {
        Ok(None)
    }

    async fn delete_application(&self, _id: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    // ─── Provider Credentials (store opaque ciphertext only) ────────────

    async fn upsert_provider_credential(
        &self,
        _app_id: &str,
        _provider: &str,
        _ciphertext: &str,
        _nonce: &str,
        _display_hint: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }

    /// Returns `(ciphertext, nonce)` for the stored credential, if any.
    async fn get_provider_credential(
        &self,
        _app_id: &str,
        _provider: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        Ok(None)
    }

    async fn list_provider_credentials(
        &self,
        _app_id: &str,
    ) -> Result<Vec<ProviderCredentialInfo>, sqlx::Error> {
        Ok(vec![])
    }

    async fn delete_provider_credential(
        &self,
        _app_id: &str,
        _provider: &str,
    ) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    // ─── Per-app BYO storage config ─────────────────────────────────────

    async fn upsert_app_storage(&self, _input: &AppStorageUpsert) -> Result<(), sqlx::Error> {
        Ok(())
    }

    async fn get_app_storage(&self, _app_id: &str) -> Result<Option<AppStorageRow>, sqlx::Error> {
        Ok(None)
    }

    async fn delete_app_storage(&self, _app_id: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }

    // ─── API Keys (tenant-scoped create / list) ─────────────────────────

    async fn create_api_key_scoped(
        &self,
        _org_id: &str,
        _app_id: &str,
        _public_id: &str,
        _name: &str,
        _key_hash: &str,
        _key_prefix: &str,
        _token_quota: Option<f64>,
        _rpm_limit: Option<u32>,
        _scopes: &str,
        _webhook_url: Option<&str>,
    ) -> Result<ApiKey, sqlx::Error> {
        Err(sqlx::Error::RowNotFound)
    }

    async fn list_api_keys_for_app(&self, _app_id: &str) -> Result<Vec<ApiKey>, sqlx::Error> {
        Ok(vec![])
    }

    // ─── Tenant-scoped reads ────────────────────────────────────────────

    async fn list_generations_for_tenant(
        &self,
        _org_id: &str,
        _app_id: Option<&str>,
        _page: u32,
        _per_page: u32,
    ) -> Result<Vec<Generation>, sqlx::Error> {
        Ok(vec![])
    }

    async fn count_generations_for_tenant(
        &self,
        _org_id: &str,
        _app_id: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        Ok(0)
    }

    async fn get_request_logs_for_tenant(
        &self,
        _org_id: &str,
        _app_id: Option<&str>,
        _page: u32,
        _per_page: u32,
    ) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
        Ok((Vec::new(), 0))
    }

    async fn list_audit_log_for_tenant(
        &self,
        _org_id: &str,
        _page: u32,
        _per_page: u32,
    ) -> Result<(Vec<AuditLogEntry>, i64), sqlx::Error> {
        Ok((Vec::new(), 0))
    }
}
