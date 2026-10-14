//! Wire and storage types for the management API (budgets/limits, end users,
//! spend reports, runtime models, provider configs, gateway settings).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// ─── Limits / budgets ────────────────────────────────────────────────────────

/// Which principal a limits row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Key,
    Org,
    App,
    Member,
    EndUser,
}

impl EntityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Org => "org",
            Self::App => "app",
            Self::Member => "member",
            Self::EndUser => "end_user",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "key" => Some(Self::Key),
            "org" => Some(Self::Org),
            "app" => Some(Self::App),
            "member" => Some(Self::Member),
            "end_user" => Some(Self::EndUser),
            _ => None,
        }
    }
}

/// Stored id of a member limits row: one user inside one org.
pub fn member_entity_id(org_id: &str, user_id: &str) -> String {
    format!("{org_id}:{user_id}")
}

/// Stored id of an end-user limits row: end-user ids are only unique per org.
pub fn end_user_entity_id(org_id: &str, end_user_id: &str) -> String {
    format!("{org_id}:{end_user_id}")
}

/// Budget and access limits for one key, org, app, org member or end user.
///
/// `spend_usd` is the spend inside the current budget window. With a
/// `budget_duration` the window resets (spend back to 0) at `budget_reset_at`;
/// without one the budget is a lifetime cap.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct EntityLimits {
    pub entity_type: EntityType,
    pub entity_id: String,
    /// Owning org (None only for keys minted with no tenant).
    pub org_id: Option<String>,
    /// Hard cap in USD for the current window. Requests that would exceed it
    /// are refused with 402 `budget_exceeded`. None = unlimited.
    pub max_budget_usd: Option<f64>,
    /// Alert threshold in USD. Crossing it records a `budget.soft_limit_reached`
    /// audit entry and fires the gateway's alert webhook once per window.
    pub soft_budget_usd: Option<f64>,
    /// Window length: `30s`, `30m`, `12h`, `30d`, `1w` or `1mo`. None = never resets.
    pub budget_duration: Option<String>,
    /// Spend inside the current window, including in-flight reservations.
    pub spend_usd: f64,
    /// When the current window ends and `spend_usd` resets.
    pub budget_reset_at: Option<DateTime<Utc>>,
    /// When the soft budget was crossed in the current window, if it was.
    pub soft_budget_alerted_at: Option<DateTime<Utc>>,
    /// Model ids this principal may generate with. None = no restriction.
    pub allowed_models: Option<Vec<String>>,
    /// Concurrent in-flight generation cap. None = unlimited.
    pub max_parallel_requests: Option<u32>,
    /// Requests-per-minute cap. None = unlimited.
    pub rpm_limit: Option<u32>,
    /// Free-form JSON object for the caller's own bookkeeping.
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for `PUT …/limits`. A full replacement of the configurable
/// fields: an omitted field is cleared. Accumulated spend is kept unless the
/// budget window changes, which starts a fresh window from now.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct LimitsInput {
    #[serde(default)]
    pub max_budget_usd: Option<f64>,
    #[serde(default)]
    pub soft_budget_usd: Option<f64>,
    #[serde(default)]
    pub budget_duration: Option<String>,
    #[serde(default)]
    pub allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub max_parallel_requests: Option<u32>,
    #[serde(default)]
    pub rpm_limit: Option<u32>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Response for `GET /v1/limits`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LimitsListResponse {
    pub data: Vec<EntityLimits>,
}

// ─── End users ───────────────────────────────────────────────────────────────

/// An end user: a caller of the tenant's own product, identified per request by
/// the `x-litegen-end-user` header. Rows are created on first sight or through
/// the API.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct EndUser {
    pub id: String,
    pub org_id: String,
    pub alias: Option<String>,
    /// A blocked end user's requests are refused with 403 `end_user_blocked`.
    pub blocked: bool,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

/// End user plus its limits, as returned by the end-user endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EndUserView {
    #[serde(flatten)]
    pub end_user: EndUser,
    pub limits: Option<EntityLimits>,
}

/// Request body for `POST /v1/end-users`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateEndUserRequest {
    pub id: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Optional limits to set in the same call.
    #[serde(default)]
    pub limits: Option<LimitsInput>,
}

/// Request body for `PATCH /v1/end-users/{id}`. Omitted fields are unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct UpdateEndUserRequest {
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub blocked: Option<bool>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Response for `GET /v1/end-users`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EndUserListResponse {
    pub data: Vec<EndUserView>,
}

// ─── Spend ───────────────────────────────────────────────────────────────────

/// Dimension a spend report is grouped by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpendGroupBy {
    Day,
    Model,
    Provider,
    MediaType,
    Status,
    Org,
    App,
    Key,
    User,
    EndUser,
    Tag,
}

impl SpendGroupBy {
    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
    }
}

/// Filters shared by the spend report and spend log queries.
#[derive(Debug, Clone, Default)]
pub struct SpendFilter {
    /// None only for a platform-admin query across every org.
    pub org_id: Option<String>,
    pub app_id: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub key_id: Option<String>,
    pub user_id: Option<String>,
    pub end_user: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub tag: Option<String>,
}

/// One group of a spend report.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct SpendReportRow {
    /// The group value (a day `YYYY-MM-DD`, model id, key id, …). Null when the
    /// requests carried no value for the dimension (e.g. no end user).
    pub group: Option<String>,
    pub requests: u64,
    pub failed_requests: u64,
    pub cost_usd: f64,
}

/// Response for `GET /v1/spend/report`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SpendReportResponse {
    pub group_by: SpendGroupBy,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub total_requests: u64,
    pub total_cost_usd: f64,
    pub data: Vec<SpendReportRow>,
}

/// One request in the spend log, with its attribution.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct SpendLogEntry {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub model: String,
    pub provider: String,
    pub media_type: String,
    pub status: String,
    pub cost_usd: f64,
    pub latency_ms: i64,
    pub org_id: Option<String>,
    pub app_id: Option<String>,
    pub key_id: Option<String>,
    pub user_id: Option<String>,
    pub end_user: Option<String>,
    pub tags: Vec<String>,
}

/// Response for `GET /v1/spend/logs`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SpendLogsResponse {
    pub data: Vec<SpendLogEntry>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

/// Who a request is attributed to, written onto its request-log row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Attribution {
    pub key_id: Option<String>,
    pub user_id: Option<String>,
    pub end_user: Option<String>,
    pub tags: Vec<String>,
}

// ─── Runtime models ──────────────────────────────────────────────────────────

/// Where a served model definition comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// `models/*.yaml`, shipped with the gateway. Read-only through the API.
    Catalog,
    /// Created through `POST /v1/admin/models`.
    Runtime,
}

/// Admin view of one model: its full schema plus management state.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AdminModelView {
    pub id: String,
    pub source: ModelSource,
    /// Disabled models are hidden from `/v1/models` and refused at generation.
    pub disabled: bool,
    pub schema: crate::capabilities::schema::ModelSchema,
}

/// Response for `GET /v1/admin/models`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AdminModelListResponse {
    pub data: Vec<AdminModelView>,
}

/// Request body for `PATCH /v1/admin/models/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PatchModelStateRequest {
    pub disabled: bool,
}

/// A runtime model as stored.
#[derive(Debug, Clone)]
pub struct StoredRuntimeModel {
    pub id: String,
    pub schema_json: String,
    pub updated_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ─── Provider configs ────────────────────────────────────────────────────────

/// Secret part of a runtime provider config. Encrypted at rest; never returned.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct ProviderSecrets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Weighted key pool, `"key:weight,key:weight"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_keys: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_secret: Option<String>,
}

impl ProviderSecrets {
    pub fn is_empty(&self) -> bool {
        self.api_key.is_none() && self.api_keys.is_none() && self.key_id.is_none() && self.key_secret.is_none()
    }

    /// Fill each unset field from `other`.
    pub fn or(self, other: &ProviderSecrets) -> ProviderSecrets {
        ProviderSecrets {
            api_key: self.api_key.or_else(|| other.api_key.clone()),
            api_keys: self.api_keys.or_else(|| other.api_keys.clone()),
            key_id: self.key_id.or_else(|| other.key_id.clone()),
            key_secret: self.key_secret.or_else(|| other.key_secret.clone()),
        }
    }
}

/// Non-secret part of a runtime provider config. Unset fields fall back to the
/// provider's env/YAML config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct ProviderSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_mapping: Option<std::collections::HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_headers: Option<std::collections::HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// False takes the provider out of service until re-enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Request body for `PUT /v1/admin/providers/{name}`. Secret fields left out
/// keep their stored (or env) value, so an api_base change needs no key.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct PutProviderConfigRequest {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_keys: Option<String>,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub key_secret: Option<String>,
    #[serde(default, flatten)]
    pub settings: ProviderSettings,
}

/// A runtime provider config as stored.
#[derive(Debug, Clone)]
pub struct StoredProviderConfig {
    pub provider: String,
    pub ciphertext: Option<String>,
    pub nonce: Option<String>,
    pub display_hint: Option<String>,
    pub settings_json: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Admin view of one provider: what is configured, with every secret masked.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderConfigView {
    pub name: String,
    /// Media this adapter can serve: image, video, model3d.
    pub modalities: Vec<String>,
    /// Whether a live instance is registered and serving requests.
    pub active: bool,
    /// Where the effective config comes from: `env`, `runtime` or `none`.
    pub source: String,
    /// Last 4 characters of the effective key, e.g. `…a1b2`. Never the secret.
    pub key_hint: Option<String>,
    pub has_secret: bool,
    pub api_base: Option<String>,
    pub region: Option<String>,
    pub model_mapping: std::collections::HashMap<String, String>,
    /// Header names only; values can carry secrets.
    pub extra_header_names: Vec<String>,
    pub options: Option<serde_json::Value>,
    pub enabled: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Response for `GET /v1/admin/providers`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderConfigListResponse {
    pub data: Vec<ProviderConfigView>,
}

// ─── Routing + gateway settings ──────────────────────────────────────────────

/// One provider a route can dispatch to.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct RouteDeployment {
    pub provider: String,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub rpm_limit: u32,
}

fn default_weight() -> u32 { 1 }
fn default_retries() -> u32 { 2 }
fn default_timeout() -> u64 { 120 }

/// A model route: which providers serve models matching `model`, and how.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct RouteSpec {
    /// Exact model id, `prefix/*` or `*`.
    pub model: String,
    pub deployments: Vec<RouteDeployment>,
    /// `fallback` (default), `weighted_round_robin`, `lowest_cost` or `lowest_latency`.
    #[serde(default)]
    pub strategy: Option<String>,
    #[serde(default)]
    pub cache_enabled: Option<bool>,
    #[serde(default)]
    pub cache_ttl_seconds: Option<u64>,
}

/// Body and response for `GET`/`PUT /v1/admin/routes`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RoutesDocument {
    /// `config` while the routes come from litegen.yaml, `runtime` once replaced through the API.
    #[serde(default)]
    pub source: Option<String>,
    pub routes: Vec<RouteSpec>,
}

/// Gateway-wide settings editable at runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct GatewaySettings {
    /// Percent added on top of provider cost when billing. Overrides the config value.
    pub cost_markup_percent: Option<f64>,
    /// URL that receives budget alerts (soft budget crossed, hard budget hit).
    pub alert_webhook_url: Option<String>,
    /// Default hard budget (USD) for an end user with no limits of its own.
    pub default_end_user_max_budget_usd: Option<f64>,
    /// Window for that default end-user budget.
    pub default_end_user_budget_duration: Option<String>,
}

/// Response for `GET /v1/admin/settings`: the stored overrides plus the
/// values in effect.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GatewaySettingsView {
    pub settings: GatewaySettings,
    pub effective_cost_markup_percent: f64,
}
