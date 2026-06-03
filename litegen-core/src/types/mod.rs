use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

// ─── Generation Request / Response ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BaseGenerationRequest {
    pub prompt: String,
    pub model: String,
    #[serde(default = "default_n")]
    pub n: u32,
    #[serde(default)]
    pub negative_prompt: Option<String>,
    #[serde(default)]
    pub seed: Option<i64>,
    #[serde(default)]
    pub reference_images: Vec<ReferenceImage>,
    #[serde(default = "default_true")]
    pub strict: bool,
    #[serde(default)]
    pub extra: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageGenerationRequest {
    #[serde(flatten)]
    pub base: BaseGenerationRequest,
    #[serde(default)] pub size: Option<String>,
    #[serde(default)] pub aspect_ratio: Option<String>,
    #[serde(default)] pub quality: Option<String>,
    #[serde(default)] pub style: Option<String>,
    #[serde(default)] pub steps: Option<u32>,
    #[serde(default)] pub guidance_scale: Option<f64>,
    #[serde(default)] pub strength: Option<f64>,
    #[serde(default = "default_response_format")]
    pub response_format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VideoGenerationRequest {
    #[serde(flatten)]
    pub base: BaseGenerationRequest,
    #[serde(default = "default_duration")]
    pub duration_seconds: f64,
    #[serde(default)] pub aspect_ratio: Option<String>,
    #[serde(default)] pub resolution: Option<String>,
    #[serde(default)] pub fps: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReferenceImage {
    #[serde(rename = "type")]
    pub kind: RefImageKind,
    pub value: String,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefImageKind {
    Base64,
    Url,
    Blob,
}

fn default_n() -> u32 { 1 }
fn default_response_format() -> String { "url".to_string() }
fn default_duration() -> f64 { 5.0 }
fn default_true() -> bool { true }

/// A single generated image in the response.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageResult {
    /// URL of the generated image (if response_format=url).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Base64-encoded image data (if response_format=b64_json).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b64_json: Option<String>,
    /// Revised prompt (if provider modified the prompt, e.g. DALL-E 3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
    /// Content type of the image (e.g. "image/png").
    #[serde(default = "default_content_type")]
    pub content_type: String,
    /// Index in the batch.
    pub index: u32,
}

fn default_content_type() -> String {
    "image/png".to_string()
}

/// Response for an image generation request.
/// Follows OpenAI's images response format for compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageGenerationResponse {
    /// Unix timestamp of when the request was created.
    pub created: i64,
    /// Array of generated images.
    pub data: Vec<ImageResult>,
    /// The model that was used.
    pub model: String,
    /// The provider that handled the request.
    pub provider: String,
    /// Cost information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
    /// Unique request ID for tracking.
    pub id: String,
}

// ─── Video Generation ─────────────────────────────────────────────────────

/// Video generation can be async — this is the initial response.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VideoGenerationResponse {
    /// Unique generation ID for polling.
    pub id: String,
    /// Current status.
    pub status: GenerationStatus,
    /// The model used.
    pub model: String,
    /// The provider.
    pub provider: String,
    /// Video URL when completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_url: Option<String>,
    /// Progress percentage (0-100).
    pub progress: u8,
    /// Error message if failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Cost information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
    /// Unix timestamp.
    pub created: i64,
}

// ─── Shared Types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GenerationStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for GenerationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Processing => write!(f, "processing"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Cost / usage information for a generation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UsageInfo {
    /// Provider cost in USD.
    pub cost_usd: f64,
    /// Internal token cost.
    pub tokens: u64,
    /// Cost source: "dynamic" (from API) or "estimated" (from pricing table).
    pub cost_source: CostSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    Dynamic,
    Estimated,
}

// ─── Provider / Model Types ─────────────────────────────────────────────────

/// A model available through LiteGen.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelInfo {
    /// Unique model ID (e.g. "openai/dall-e-3").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Provider name.
    pub provider: String,
    /// Media type this model produces.
    pub media_type: MediaType,
    /// Whether the model is currently available.
    pub is_available: bool,
    /// Model capabilities.
    pub capabilities: ModelCapabilities,
    /// Pricing information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing: Option<ModelPricing>,
    /// Tags for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Image,
    Video,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelCapabilities {
    pub supports_text_to_image: bool,
    pub supports_image_to_image: bool,
    #[serde(default)]
    pub supports_inpainting: bool,
    #[serde(default)]
    pub supported_sizes: Vec<String>,
    #[serde(default = "default_max_images")]
    pub max_images: u32,
    // Video-specific
    #[serde(default)]
    pub supports_text_to_video: bool,
    #[serde(default)]
    pub supports_image_to_video: bool,
    #[serde(default)]
    pub supports_first_frame: bool,
    #[serde(default)]
    pub supports_last_frame: bool,
    #[serde(default)]
    pub max_duration_seconds: Option<f64>,
}

fn default_max_images() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelPricing {
    /// Base cost per image/video in USD.
    pub base_cost_usd: f64,
    /// Variable pricing by dimension (JSON map).
    #[serde(default)]
    pub variable_pricing: Option<serde_json::Value>,
}

// ─── Cost Estimation ────────────────────────────────────────────────────────

/// Cost estimate returned before generation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CostEstimate {
    /// Base cost from the provider in USD.
    pub base_cost_usd: f64,
    /// Markup applied (configurable, default 0%).
    pub markup_usd: f64,
    /// Total cost including markup.
    pub total_cost_usd: f64,
    /// Equivalent token cost.
    pub tokens_required: u64,
    /// Where the cost data came from.
    pub cost_source: CostSource,
    /// Breakdown details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakdown: Option<serde_json::Value>,
}

// ─── Routing / Config Types ─────────────────────────────────────────────────

/// Configuration for a provider deployment.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderConfig {
    /// Provider name (e.g. "openai", "stability", "replicate").
    pub provider: String,
    /// API key(s) with optional weights.
    pub api_keys: Vec<ApiKeyEntry>,
    /// Base URL override.
    #[serde(default)]
    pub api_base: Option<String>,
    /// Model ID mapping: internal ID → provider's model ID.
    #[serde(default)]
    pub model_mapping: std::collections::HashMap<String, String>,
    /// Extra headers.
    #[serde(default)]
    pub extra_headers: std::collections::HashMap<String, String>,
    /// Provider-specific options.
    #[serde(default)]
    pub options: Option<serde_json::Value>,
    /// Whether this provider config is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// An API key with weight for round-robin distribution.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyEntry {
    /// The API key value. Stored encrypted at rest.
    pub key: String,
    /// Weight for round-robin (higher = more traffic). Default: 1.
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// Optional label for the key.
    #[serde(default)]
    pub label: Option<String>,
}

fn default_weight() -> u32 {
    1
}

/// Routing configuration for a model, with fallbacks and weights.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelRoute {
    /// The model ID pattern (e.g. "dall-e-3", "openai/*").
    pub model: String,
    /// Ordered list of provider deployments to try.
    pub deployments: Vec<Deployment>,
    /// Routing strategy.
    #[serde(default)]
    pub strategy: RoutingStrategy,
    /// Cache settings for this model.
    #[serde(default)]
    pub cache: Option<CacheConfig>,
}

/// A single deployment in a routing chain.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Deployment {
    /// Provider config name/id.
    pub provider: String,
    /// Weight for weighted routing.
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// Max retries before falling to next deployment.
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    /// Timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    /// Rate limit (requests per minute). 0 = unlimited.
    #[serde(default)]
    pub rpm_limit: u32,
    /// Whether to skip this deployment on health check failure.
    #[serde(default = "default_true")]
    pub respect_health: bool,
}

fn default_retries() -> u32 {
    2
}
fn default_timeout() -> u64 {
    120
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    /// Try deployments in order, fall back on failure.
    #[default]
    Fallback,
    /// Distribute by weight.
    WeightedRoundRobin,
    /// Pick the cheapest provider.
    LowestCost,
    /// Pick the fastest provider (by latency history).
    LowestLatency,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CacheConfig {
    /// Enable caching for this model.
    #[serde(default)]
    pub enabled: bool,
    /// Cache TTL in seconds.
    #[serde(default = "default_cache_ttl")]
    pub ttl_seconds: u64,
    /// Max cached items.
    #[serde(default = "default_cache_max")]
    pub max_items: u64,
}

fn default_cache_ttl() -> u64 {
    3600
}
fn default_cache_max() -> u64 {
    1000
}

// ─── Health & Stats ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderHealth {
    pub provider: String,
    pub healthy: bool,
    pub message: Option<String>,
    pub latency_ms: Option<u64>,
    pub last_checked: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RequestLog {
    pub id: String,
    pub model: String,
    pub provider: String,
    pub status: GenerationStatus,
    pub media_type: MediaType,
    pub cost_usd: f64,
    pub latency_ms: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Latency percentiles for the last N minutes.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LatencyPercentiles {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub sample_count: u64,
    pub window_minutes: i64,
}

/// Aggregate stats for the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProxyStats {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
    pub requests_per_minute: f64,
    pub models_used: Vec<ModelUsageStat>,
    pub providers_used: Vec<ProviderUsageStat>,
    pub latency_percentiles: LatencyPercentiles,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelUsageStat {
    pub model: String,
    pub requests: u64,
    pub cost_usd: f64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderUsageStat {
    pub provider: String,
    pub requests: u64,
    pub failures: u64,
    pub cost_usd: f64,
    pub avg_latency_ms: f64,
}

// ─── Log Filters ─────────────────────────────────────────────────────────────

/// Optional filters for `get_request_logs_filtered`.
#[derive(Debug, Clone, Default)]
pub struct LogFilters {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub status: Option<String>,
    /// ISO 8601 lower bound (inclusive).
    pub from: Option<String>,
    /// ISO 8601 upper bound (inclusive).
    pub to: Option<String>,
}

// ─── Pagination ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PaginatedResponse<T: Serialize> {
    pub data: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
    pub total_pages: u32,
}

// ─── Error Response ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorDetail {
    pub message: String,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_error: Option<serde_json::Value>,
}

// ─── Generations (DB-backed polling) ───────────────────────────────────────

/// A DB-backed generation row (video or image, currently used for video).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Generation {
    /// Locally-minted ID (e.g. "litegen-vid-<uuid>").
    pub id: String,
    /// API key that submitted this generation; None when master key was used.
    pub key_id: Option<uuid::Uuid>,
    pub model: String,
    pub provider: String,
    pub media_type: String,
    pub status: GenerationStatus,
    pub progress: i32,
    /// Provider-assigned job ID used for polling.
    pub provider_job_id: Option<String>,
    /// Final result URL when completed.
    pub result_url: Option<String>,
    pub error_message: Option<String>,
    pub cost_usd: f64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Arbitrary JSON metadata.
    pub metadata: Option<serde_json::Value>,
}

// ─── Tenancy (Organizations / Applications / Members / Credentials) ─────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Organization {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub plan: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Application {
    pub id: String,
    pub org_id: String,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OrganizationMember {
    pub org_id: String,
    pub user_id: String,
    pub email: String, // joined from users for list views
    pub role: Role,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Public view of a stored BYO provider credential — NEVER the plaintext secret.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderCredentialInfo {
    pub provider: String,
    pub display_hint: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ─── API Key Auth ───────────────────────────────────────────────────────────

/// API key for authenticating with the LiteGen proxy.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKey {
    pub id: Uuid,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
    /// USD budget cap; None = unlimited.
    pub token_quota: Option<f64>,
    /// Running USD spent under this key.
    pub tokens_used: f64,
    /// Requests-per-minute cap; None = unlimited.
    pub rpm_limit: Option<u32>,
    /// CSV of scopes: "generate,read,admin".
    pub scopes: String,
    /// Webhook URL for async notifications (future use).
    pub webhook_url: Option<String>,
    /// The user who owns this key (None for master-key-created keys).
    pub owner_user_id: Option<String>,
    pub org_id: Option<String>,
    pub app_id: Option<String>,
    /// Public key id shown to customers, e.g. "pk_live_…". None for legacy lg- keys.
    pub public_id: Option<String>,
}

/// Full key detail for GET /v1/keys/{id} and PATCH /v1/keys/{id} responses.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyDetail {
    pub id: Uuid,
    pub name: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
    pub token_quota: Option<f64>,
    pub tokens_used: f64,
    pub rpm_limit: Option<u32>,
    pub scopes: String,
    pub webhook_url: Option<String>,
}

/// Request body for PATCH /v1/keys/{id}.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateApiKeyRequest {
    pub name: Option<String>,
    pub token_quota: Option<f64>,
    pub rpm_limit: Option<u32>,
    pub scopes: Option<String>,
    pub webhook_url: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: Option<bool>,
}

// ─── Typed API Response Wrappers ────────────────────────────────────────────

/// Response for `GET /health/live`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LivenessResponse {
    pub status: String,
}

/// Response for `GET /health/ready`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReadinessResponse {
    /// "ready" or "not_ready"
    pub status: String,
    pub checks: ReadinessChecks,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReadinessChecks {
    pub db: bool,
    pub providers: Vec<String>,
}

/// Cache state included in the health response.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CacheStatus {
    pub enabled: bool,
    pub entries: u64,
}

/// Response for `GET /health`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub providers: Vec<ProviderHealth>,
    pub cache: CacheStatus,
}

/// Response for `GET /v1/models`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelListResponse {
    pub object: String,
    pub data: Vec<ModelInfo>,
}

/// Public view of an API key (no hash).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyInfo {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
    pub token_quota: Option<f64>,
    pub tokens_used: f64,
    pub rpm_limit: Option<u32>,
    pub scopes: String,
    pub webhook_url: Option<String>,
    pub public_id: Option<String>,
    pub app_id: Option<String>,
}

/// Response for `GET /v1/keys`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyListResponse {
    pub data: Vec<ApiKeyInfo>,
}

/// Response for `POST /v1/keys`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyCreatedResponse {
    pub id: Uuid,
    pub key: String,
    pub prefix: String,
    pub public_id: String,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub token_quota: Option<f64>,
    pub rpm_limit: Option<u32>,
    pub scopes: String,
}

/// Response for `DELETE /v1/keys/{id}` and similar revocation endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevokeKeyResponse {
    pub revoked: bool,
}

/// Response for `DELETE /v1/cache`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CacheClearedResponse {
    pub cleared: bool,
}

// ─── Audit Log ───────────────────────────────────────────────────────────────

/// One audit log entry recording an admin action.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AuditLogEntry {
    pub id: String,
    /// The API key ID of the actor. None when the master key was used.
    pub actor_key_id: Option<String>,
    /// Human-readable actor label: `"master-key"` or the key's name.
    pub actor_label: String,
    /// Action identifier, e.g. `"key.create"`, `"generation.cancel"`.
    pub action: String,
    /// Target entity type: `"api_key"` or `"generation"`.
    pub target_type: String,
    /// Target entity ID.
    pub target_id: String,
    /// JSON snapshot of the entity state *before* the action (None on create).
    pub before_json: Option<String>,
    /// JSON snapshot of the entity state *after* the action (None on delete/revoke).
    pub after_json: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Filters for `list_audit_log`.
#[derive(Debug, Clone, Default)]
pub struct AuditLogFilter {
    pub actor_key_id: Option<String>,
    pub action: Option<String>,
    /// ISO 8601 lower bound (inclusive).
    pub from: Option<String>,
    /// ISO 8601 upper bound (inclusive).
    pub to: Option<String>,
}

// ─── Webhook Delivery Log ────────────────────────────────────────────────────

// ─── Request Artifacts ──────────────────────────────────────────────────────

/// Stores the input/output snapshot of a generation request for drill-down.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RequestArtifact {
    pub request_id: String,
    pub media_type: String,
    pub prompt: Option<String>,
    pub negative_prompt: Option<String>,
    pub params_json: Option<serde_json::Value>,
    pub refs_meta_json: Option<serde_json::Value>,
    /// "b64" | "url" | "error"
    pub output_kind: String,
    pub output_value: Option<String>,
    pub output_mime: Option<String>,
    pub output_truncated: bool,
    pub error_message: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// One row in the `webhook_deliveries` table.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WebhookDelivery {
    pub id: String,
    pub key_id: String,
    pub generation_id: String,
    pub url: String,
    pub attempt_number: i32,
    pub status_code: Option<i32>,
    /// Whether this attempt was successful (HTTP 2xx).
    pub success: bool,
    pub response_body: Option<String>,
    pub error_message: Option<String>,
    pub payload_json: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ─── User / Session / Invitation / PasswordReset ────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct User {
    pub id: String,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    pub role: Role,
    pub oauth_github_id: Option<String>,
    pub oauth_google_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_login_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub csrf_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Invitation {
    pub id: String,
    pub email: String,
    pub role: Role,
    pub token: String,
    pub invited_by: Option<String>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PasswordReset {
    pub token: String,
    pub user_id: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_and_app_serde_roundtrip() {
        let org = Organization { id: "o1".into(), name: "Acme".into(), slug: "acme".into(),
            plan: "free".into(), status: "active".into(),
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now() };
        let j = serde_json::to_string(&org).unwrap();
        let back: Organization = serde_json::from_str(&j).unwrap();
        assert_eq!(back.slug, "acme");
    }
}

#[cfg(test)]
mod ref_image_tests {
    use super::*;

    #[test]
    fn deserialize_url_ref() {
        let j = r#"{"type":"url","value":"https://example.com/x.png"}"#;
        let r: ReferenceImage = serde_json::from_str(j).unwrap();
        assert!(matches!(r.kind, RefImageKind::Url));
        assert_eq!(r.value, "https://example.com/x.png");
        assert!(r.role.is_none());
    }

    #[test]
    fn deserialize_base64_ref_with_role() {
        let j = r#"{"type":"base64","value":"abc==","role":"mask"}"#;
        let r: ReferenceImage = serde_json::from_str(j).unwrap();
        assert!(matches!(r.kind, RefImageKind::Base64));
        assert_eq!(r.role.as_deref(), Some("mask"));
    }

    #[test]
    fn deserialize_blob_ref() {
        let j = r#"{"type":"blob","value":"field_init","role":"init"}"#;
        let r: ReferenceImage = serde_json::from_str(j).unwrap();
        assert!(matches!(r.kind, RefImageKind::Blob));
        assert_eq!(r.value, "field_init");
    }

    #[test]
    fn flatten_image_request() {
        let j = r#"{
          "prompt":"hi","model":"x/y","reference_images":[
            {"type":"url","value":"u"}
          ],
          "size":"1024x1024","strict":false
        }"#;
        let r: ImageGenerationRequest = serde_json::from_str(j).unwrap();
        assert_eq!(r.base.prompt, "hi");
        assert_eq!(r.base.reference_images.len(), 1);
        assert!(!r.base.strict);
        assert_eq!(r.size.as_deref(), Some("1024x1024"));
    }
}
