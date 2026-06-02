use serde::Deserialize;
use std::collections::HashMap;

/// Top-level application configuration.
/// Loaded from `litegen.yaml`, environment variables, and CLI args.
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    /// Server settings.
    #[serde(default)]
    pub server: ServerConfig,
    /// Database connection string.
    #[serde(default = "default_database_url")]
    pub database_url: String,
    /// Master API key for admin operations (optional; if set, all requests must auth).
    #[serde(default)]
    pub master_key: Option<String>,
    /// Global markup percentage applied to provider costs (default: 0.0 = no markup).
    #[serde(default)]
    pub cost_markup_percent: f64,
    /// Provider configurations keyed by name.
    #[serde(default)]
    pub providers: HashMap<String, ProviderEnvConfig>,
    /// Model routing rules.
    #[serde(default)]
    pub model_routes: Vec<ModelRouteConfig>,
    /// Global cache settings.
    #[serde(default)]
    pub cache: CacheGlobalConfig,
    /// Logging & observability.
    #[serde(default)]
    pub observability: ObservabilityConfig,
    /// Image storage settings (local base64, S3, etc.).
    #[serde(default)]
    pub image_storage: ImageStorageConfig,
    /// Circuit breaker settings for provider failure detection.
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerConfig,
    /// CORS configuration.
    #[serde(default)]
    pub cors: CorsConfig,
    /// Backpressure / concurrency limit configuration.
    #[serde(default)]
    pub backpressure: BackpressureConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            database_url: default_database_url(),
            master_key: None,
            cost_markup_percent: 0.0,
            providers: HashMap::new(),
            model_routes: Vec::new(),
            cache: CacheGlobalConfig::default(),
            observability: ObservabilityConfig::default(),
            image_storage: ImageStorageConfig::default(),
            circuit_breaker: CircuitBreakerConfig::default(),
            cors: CorsConfig::default(),
            backpressure: BackpressureConfig::default(),
        }
    }
}

fn default_database_url() -> String {
    "sqlite://litegen.db".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Request timeout in seconds.
    #[serde(default = "default_request_timeout")]
    pub request_timeout_seconds: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            request_timeout_seconds: default_request_timeout(),
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    4000
}
fn default_request_timeout() -> u64 {
    300
}

/// Provider settings loaded from config or env vars.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderEnvConfig {
    /// API key (single). For multiple keys, use `api_keys`.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Multiple API keys with weights ("key:weight,key:weight").
    #[serde(default)]
    pub api_keys: Option<String>,
    /// Base URL override.
    #[serde(default)]
    pub api_base: Option<String>,
    /// Model mapping overrides.
    #[serde(default)]
    pub model_mapping: HashMap<String, String>,
    /// Extra headers.
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    /// Provider-specific options.
    #[serde(default)]
    pub options: Option<serde_json::Value>,
    /// Access key id (SigV4) / secret id (TC3) / access key (Kling).
    #[serde(default)]
    pub key_id: Option<String>,
    /// Secret access key (SigV4) / secret key (TC3, Kling).
    #[serde(default)]
    pub key_secret: Option<String>,
    /// Region for signing schemes (SigV4 / TC3). Non-secret.
    #[serde(default)]
    pub region: Option<String>,
    /// Auxiliary credential fields (reserved; e.g. group_id).
    #[serde(default)]
    pub credentials_extra: HashMap<String, String>,
    /// Enabled flag.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelRouteConfig {
    /// Model pattern (e.g. "dall-e-3", "openai/*", "*").
    pub model: String,
    /// Deployments in priority order.
    #[serde(default)]
    pub deployments: Vec<DeploymentConfig>,
    /// Routing strategy.
    #[serde(default)]
    pub strategy: Option<String>,
    /// Cache config for this route.
    #[serde(default)]
    pub cache: Option<CacheRouteConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeploymentConfig {
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

fn default_weight() -> u32 {
    1
}
fn default_retries() -> u32 {
    2
}
fn default_timeout() -> u64 {
    120
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheRouteConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_cache_ttl")]
    pub ttl_seconds: u64,
}

fn default_cache_ttl() -> u64 {
    3600
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheGlobalConfig {
    /// Enable the generation cache globally.
    #[serde(default)]
    pub enabled: bool,
    /// Default TTL in seconds.
    #[serde(default = "default_cache_ttl")]
    pub default_ttl_seconds: u64,
    /// Max total cached items.
    #[serde(default = "default_max_items")]
    pub max_items: u64,
}

impl Default for CacheGlobalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_ttl_seconds: 3600,
            max_items: 10_000,
        }
    }
}

fn default_max_items() -> u64 {
    10_000
}

#[derive(Debug, Clone, Deserialize)]
pub struct ObservabilityConfig {
    /// Log level (trace, debug, info, warn, error).
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Enable JSON structured logging.
    #[serde(default)]
    pub json_logs: bool,
    /// Enable Prometheus metrics endpoint.
    #[serde(default = "default_true")]
    pub metrics_enabled: bool,
    /// Metrics endpoint path.
    #[serde(default = "default_metrics_path")]
    pub metrics_path: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            json_logs: false,
            metrics_enabled: true,
            metrics_path: default_metrics_path(),
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}
fn default_metrics_path() -> String {
    "/metrics".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the breaker.
    #[serde(default = "default_cb_threshold")]
    pub threshold: u32,
    /// Seconds to keep the breaker open before allowing a trial request.
    #[serde(default = "default_cb_open_for")]
    pub open_for_seconds: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            threshold: 5,
            open_for_seconds: 30,
        }
    }
}

fn default_cb_threshold() -> u32 { 5 }
fn default_cb_open_for() -> u64 { 30 }

/// Backpressure / global concurrency limit configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct BackpressureConfig {
    /// Maximum number of in-flight `generate_image` + `generate_video` requests.
    /// When this limit is reached new requests receive `503 Service Unavailable`.
    #[serde(default = "default_max_in_flight")]
    pub max_in_flight: usize,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self { max_in_flight: default_max_in_flight() }
    }
}

fn default_max_in_flight() -> usize { 64 }

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CorsConfig {
    /// CSV of allowed origins. Empty/unset = deny all. "*" = allow any.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// Set Access-Control-Allow-Credentials header.
    #[serde(default)]
    pub allow_credentials: bool,
}

/// Image storage configuration.
/// Controls where generated images are persisted.
#[derive(Debug, Clone, Deserialize)]
pub struct ImageStorageConfig {
    /// Storage backend: "local" (return base64 inline) or "s3" (upload to S3-compatible storage).
    #[serde(default = "default_storage_backend")]
    pub backend: String,
    /// S3 configuration (required when backend = "s3").
    #[serde(default)]
    pub s3: Option<S3StorageConfig>,
    /// Custom path prefix inside the bucket (default: "litegen/images").
    #[serde(default)]
    pub path_prefix: Option<String>,
}

impl Default for ImageStorageConfig {
    fn default() -> Self {
        Self {
            backend: default_storage_backend(),
            s3: None,
            path_prefix: None,
        }
    }
}

fn default_storage_backend() -> String {
    "local".to_string()
}

/// S3-compatible storage configuration.
/// Works with AWS S3, MinIO, Cloudflare R2, DigitalOcean Spaces, etc.
#[derive(Debug, Clone, Deserialize)]
pub struct S3StorageConfig {
    /// S3 bucket name.
    pub bucket_name: String,
    /// AWS region (e.g. "us-east-1") or region identifier for compatible services.
    #[serde(default = "default_s3_region")]
    pub region: String,
    /// AWS access key ID (or set AWS_ACCESS_KEY_ID / LITEGEN_S3_ACCESS_KEY_ID env var).
    #[serde(default)]
    pub access_key_id: Option<String>,
    /// AWS secret access key (or set AWS_SECRET_ACCESS_KEY / LITEGEN_S3_SECRET_ACCESS_KEY env var).
    #[serde(default)]
    pub secret_access_key: Option<String>,
    /// Custom S3 endpoint URL for S3-compatible services (MinIO, R2, Spaces, etc.).
    #[serde(default)]
    pub endpoint_url: Option<String>,
    /// Custom public URL base for accessing stored images.
    /// Use this when your bucket is behind a CDN or custom domain.
    /// Example: "https://cdn.example.com" → images served at https://cdn.example.com/litegen/images/...
    #[serde(default)]
    pub custom_public_url: Option<String>,
}

fn default_s3_region() -> String {
    "us-east-1".to_string()
}

/// Load configuration from litegen.yaml + env vars + LITEGEN_ prefix.
pub fn load_config() -> Result<AppConfig, config::ConfigError> {
    let mut builder = config::Config::builder();

    // 1. Default values
    builder = builder.add_source(config::Config::try_from(&AppConfig::default())?);

    // 2. Config file (optional)
    builder = builder.add_source(
        config::File::with_name("litegen")
            .required(false),
    );

    // 3. Environment variables (LITEGEN_ prefix, __ for nesting)
    builder = builder.add_source(
        config::Environment::with_prefix("LITEGEN")
            .separator("__")
            .try_parsing(true),
    );

    // Also load common env vars directly for provider convenience.
    // e.g. OPENAI_API_KEY → providers.openai.api_key
    load_provider_env_overrides(&mut builder);

    builder.build()?.try_deserialize()
}

/// Map common env vars (OPENAI_API_KEY, etc.) into the config tree.
fn load_provider_env_overrides(builder: &mut config::ConfigBuilder<config::builder::DefaultState>) {
    let env_mappings = [
        ("OPENAI_API_KEY", "providers.openai.api_key"),
        ("OPENAI_API_KEYS", "providers.openai.api_keys"),
        ("OPENAI_API_BASE", "providers.openai.api_base"),
        ("STABILITY_API_KEY", "providers.stability.api_key"),
        ("STABILITY_API_KEYS", "providers.stability.api_keys"),
        ("REPLICATE_API_TOKEN", "providers.replicate.api_key"),
        ("REPLICATE_API_TOKENS", "providers.replicate.api_keys"),
        ("GOOGLE_API_KEY", "providers.google.api_key"),
        ("GOOGLE_API_KEYS", "providers.google.api_keys"),
        ("FAL_KEY", "providers.fal.api_key"),
        ("FAL_KEYS", "providers.fal.api_keys"),
        ("RUNWAY_API_KEY", "providers.runway.api_key"),
        ("LUMA_API_KEY", "providers.luma.api_key"),
        // ── Expansion vendors (simple API keys) ──
        ("BFL_API_KEY", "providers.bfl.api_key"),
        ("IDEOGRAM_API_KEY", "providers.ideogram.api_key"),
        ("RECRAFT_API_TOKEN", "providers.recraft.api_key"),
        ("MINIMAX_API_KEY", "providers.minimax.api_key"),
        ("MINIMAX_API_BASE", "providers.minimax.api_base"),
        ("BYTEDANCE_API_KEY", "providers.bytedance.api_key"),
        ("BYTEDANCE_API_BASE", "providers.bytedance.api_base"),
        ("ARK_API_KEY", "providers.bytedance.api_key"),
        ("VIDU_API_KEY", "providers.vidu.api_key"),
        ("PIXVERSE_API_KEY", "providers.pixverse.api_key"),
        ("LEONARDO_API_KEY", "providers.leonardo.api_key"),
        // ── Signing-scheme vendors (key_id/key_secret/region) ──
        ("KLING_ACCESS_KEY", "providers.kling.key_id"),
        ("KLING_SECRET_KEY", "providers.kling.key_secret"),
        ("BEDROCK_ACCESS_KEY_ID", "providers.bedrock.key_id"),
        ("BEDROCK_SECRET_ACCESS_KEY", "providers.bedrock.key_secret"),
        ("BEDROCK_REGION", "providers.bedrock.region"),
        ("TENCENT_SECRET_ID", "providers.hunyuan.key_id"),
        ("TENCENT_SECRET_KEY", "providers.hunyuan.key_secret"),
        ("TENCENT_REGION", "providers.hunyuan.region"),
        // NOTE: openai/fal/replicate each serve both image AND video under the
        // single vendor name (registered as image+video in the provider
        // registry), so their video impls reuse the vendor key above — no
        // separate "*-video" provider entries are needed.
        // S3 storage credentials
        ("AWS_ACCESS_KEY_ID", "image_storage.s3.access_key_id"),
        ("AWS_SECRET_ACCESS_KEY", "image_storage.s3.secret_access_key"),
        ("LITEGEN_S3_ACCESS_KEY_ID", "image_storage.s3.access_key_id"),
        ("LITEGEN_S3_SECRET_ACCESS_KEY", "image_storage.s3.secret_access_key"),
        ("LITEGEN_S3_BUCKET", "image_storage.s3.bucket_name"),
        ("LITEGEN_S3_REGION", "image_storage.s3.region"),
        ("LITEGEN_S3_ENDPOINT_URL", "image_storage.s3.endpoint_url"),
    ];

    for (env_var, _config_path) in &env_mappings {
        if let Ok(val) = std::env::var(env_var) {
            // Store in a way the config system can pick up.
            // The environment source with LITEGEN_ prefix handles structured mapping.
            // For direct env vars, we set them via the LITEGEN prefix:
            let litegen_key = format!("LITEGEN__{}", _config_path.replace('.', "__").to_uppercase());
            std::env::set_var(&litegen_key, &val);
        }
    }

    // Rebuild environment source to pick up the new vars
    // (This is handled by the caller re-adding the Environment source)
    let _ = builder;
}

impl serde::Serialize for AppConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("server", &ServerConfigSerialize(&self.server))?;
        map.serialize_entry("database_url", &self.database_url)?;
        map.serialize_entry("cost_markup_percent", &self.cost_markup_percent)?;
        map.end()
    }
}

struct ServerConfigSerialize<'a>(&'a ServerConfig);

impl<'a> serde::Serialize for ServerConfigSerialize<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("host", &self.0.host)?;
        map.serialize_entry("port", &self.0.port)?;
        map.serialize_entry("request_timeout_seconds", &self.0.request_timeout_seconds)?;
        map.end()
    }
}
