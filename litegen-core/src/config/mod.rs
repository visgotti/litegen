use serde::Deserialize;
use std::collections::HashMap;

pub mod connections;
pub mod load;

pub use load::{config_file, load_config_from, load_config_with, parse_args, Args, ConfigFile, USAGE};

/// Runtime tenancy mode.
/// `SingleTenant` (default) preserves the original single-deployment behavior;
/// `Hosted` enables multi-tenant features (enforced by later tasks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    SingleTenant,
    Hosted,
}

impl Mode {
    /// Parse a mode from a string (used by tests and any non-serde string source).
    /// Config loading itself uses serde deserialization of `LITEGEN__MODE`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "single_tenant" | "single" => Some(Mode::SingleTenant),
            "hosted" | "multi_tenant" => Some(Mode::Hosted),
            _ => None,
        }
    }
}

/// Development-only feature flags. Off by default.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DevFlags {
    #[serde(default)] pub expose_invite_tokens: bool,
    #[serde(default)] pub expose_reset_tokens: bool,
}

/// Authentication configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuthConfig {
    /// Allow email/password signup + login. Default `true` (preserves
    /// single-tenant/dev behavior). Set `LITEGEN__AUTH__ALLOW_PASSWORD=false`
    /// in hosted OAuth-only deployments to disable the password endpoints and
    /// hide the password form in the dashboard.
    #[serde(default = "default_true")]
    pub allow_password: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self { allow_password: true }
    }
}

/// Top-level application configuration, loaded from the config file
/// (`litegen.yaml` in the working directory, or the file `--config` or
/// `LITEGEN_CONFIG_FILE_PATH` names) and environment variables
/// ([`load_config_with`], `docs/configuration.md`). `--config` is the only
/// command-line argument; no setting is read from the command line.
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
    /// Runtime tenancy mode (single_tenant | hosted).
    #[serde(default)]
    pub mode: Mode,
    /// `LITEGEN__SECRETS_KEY`: base64 32-byte key for encrypting BYO provider
    /// credentials at rest. Required in hosted mode (enforced by a later task).
    #[serde(default)]
    pub secrets_key: Option<String>,
    /// Development-only feature flags.
    #[serde(default)]
    pub dev: DevFlags,
    /// Authentication configuration (password-auth toggle, etc.).
    #[serde(default)]
    pub auth: AuthConfig,
    /// Generation call hooks (`docs/hooks.md`): Python, Node or Bun files run
    /// around the generation routes, in this order.
    #[serde(default)]
    pub hooks: Vec<crate::hooks::HookConfig>,
    /// Who may open stored outputs (`docs/storage-access.md`): the defaults
    /// under the gateway setting and each app's own.
    #[serde(default)]
    pub storage_access: StorageAccessConfig,
    /// litegen's own AWS identity, with which it assumes the IAM roles that
    /// apps' buckets trust (`credential_mode: assume_role`).
    #[serde(default)]
    pub aws: AwsConfig,
    /// How retryable vendor errors are tried again (`docs/gateway-settings.md`).
    #[serde(default)]
    pub retries: RetriesConfig,
    /// The MCP endpoints (`/mcp` and `/admin/mcp`).
    #[serde(default)]
    pub mcp: McpConfig,
    /// Redis, shared by replicas (`docs/scaling.md`). Set by `LITEGEN_REDIS_URL`
    /// or `LITEGEN_REDIS_HOST` and their companions, never by `litegen.yaml`;
    /// `None` is one replica, as before.
    #[serde(skip)]
    pub redis: Option<connections::RedisSettings>,
    /// Settings that lost to a higher-precedence one, named for the log at
    /// startup (`main` logs each).
    #[serde(skip)]
    pub startup_warnings: Vec<String>,
    /// The config file read ([`config_file`]), or `None` when there was none.
    #[serde(skip)]
    pub config_file: Option<std::path::PathBuf>,
}

/// LiteGen's MCP server. `/mcp` serves the AI API and `/admin/mcp` the
/// management API, on LiteLLM's paths.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(default)]
pub struct McpConfig {
    /// Serve MCP at all. Off turns both endpoints off.
    pub enabled: bool,
    /// Also serve `/admin/mcp` (like LiteLLM's `LITELLM_ENABLE_ADMIN_MCP`).
    pub admin_enabled: bool,
    /// Extra `Host` values MCP accepts. Loopback and the host of
    /// `server.public_base_url` are always accepted; `["*"]` turns the check off.
    pub allowed_hosts: Vec<String>,
    /// Extra browser `Origin`s MCP accepts. The CORS origins and the origin of
    /// `server.public_base_url` are always accepted.
    pub allowed_origins: Vec<String>,
    /// Sign-in for clients that cannot send an API key (ChatGPT, claude.ai).
    pub oauth: McpOAuthConfig,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            admin_enabled: true,
            allowed_hosts: Vec::new(),
            allowed_origins: Vec::new(),
            oauth: McpOAuthConfig::default(),
        }
    }
}

/// LiteGen as the OAuth 2.1 authorization server for its MCP endpoints. The
/// issuer is `server.public_base_url`, which must be the public HTTPS URL for
/// ChatGPT and claude.ai to sign in.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(default)]
pub struct McpOAuthConfig {
    /// Serve the discovery documents and the authorization server.
    pub enabled: bool,
    /// The dashboard's consent page. Default: the origin of
    /// `server.public_base_url` + `/oauth/consent`.
    pub consent_url: Option<String>,
    pub access_token_ttl_secs: u64,
    pub refresh_token_ttl_days: u64,
    /// Requests each address may make a minute to `/oauth/register` and
    /// `/oauth/authorize`, which anyone may call; 0 turns the limit off.
    pub rate_limit_per_minute: u32,
    /// Hosts whose redirect URIs get an authorization request's error back
    /// automatically, before anyone has signed in (RFC 9700 §4.11.2), for
    /// example `claude.ai`. Matched exactly, ignoring case and port. Loopback
    /// redirect URIs (native apps) always do. Any other app's error is shown
    /// on a LiteGen page with a link back, since anyone may register a client
    /// with any `https` redirect URI. Default: none.
    pub trusted_redirect_hosts: Vec<String>,
}

impl Default for McpOAuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            consent_url: None,
            access_token_ttl_secs: 3600,
            refresh_token_ttl_days: 30,
            rate_limit_per_minute: 30,
            trusted_redirect_hosts: Vec::new(),
        }
    }
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
            mode: Mode::default(),
            secrets_key: None,
            dev: DevFlags::default(),
            auth: AuthConfig::default(),
            hooks: Vec::new(),
            storage_access: StorageAccessConfig::default(),
            aws: AwsConfig::default(),
            retries: RetriesConfig::default(),
            mcp: McpConfig::default(),
            redis: None,
            startup_warnings: Vec::new(),
            config_file: None,
        }
    }
}

/// Result of the pre-bind security self-check ([`AppConfig::startup_security_check`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupSecurityCheck {
    /// Safe to start.
    Ok,
    /// Single-tenant + no `master_key` + a non-loopback bind: the
    /// `auth_middleware` no-credential dev-bypass (which grants full admin scope
    /// to anonymous requests) would be reachable from the network, exposing the
    /// entire admin API. `host` is the offending bind address.
    UnauthenticatedExposure { host: String },
}

/// True when `host` binds only the loopback interface (not reachable off-box).
fn host_is_loopback(host: &str) -> bool {
    if host.trim().eq_ignore_ascii_case("localhost") {
        return true;
    }
    // Tolerate bracketed IPv6 literals like "[::1]".
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    matches!(h.parse::<std::net::IpAddr>(), Ok(ip) if ip.is_loopback())
}

impl AppConfig {
    /// Settings this config sets that litegen no longer uses, one line each;
    /// the loader adds them to `startup_warnings`.
    pub fn retired_setting_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if self.server.request_timeout_seconds.is_some() {
            warnings.push(
                "server.request_timeout_seconds is no longer used and will be removed: litegen sets no limit on a whole request. \
                 A vendor call is limited by server.vendor_timeout_secs, request headers by server.header_read_timeout_secs, \
                 a request body by server.request_body_timeout_secs, and shutdown by server.shutdown_grace_secs."
                    .to_string(),
            );
        }
        warnings
    }

    /// What's wrong with the providers' config, one line each, naming the
    /// provider: today, `extra_headers` that can't be sent. `main` refuses to
    /// start on any.
    pub fn provider_config_errors(&self) -> Vec<String> {
        let mut names: Vec<&String> = self.providers.keys().collect();
        names.sort();
        names
            .into_iter()
            .filter_map(|name| {
                crate::providers::check_extra_headers(&self.providers[name].extra_headers)
                    .err()
                    .map(|e| format!("providers.{name}.extra_headers: {e}"))
            })
            .collect()
    }

    /// Provider credentials that no longer authenticate anything, one line
    /// each; `main` logs them at startup. Kling takes an API key since
    /// 2026-11-03 ("API Key authentication must be used to call Kling AI
    /// API"); its access/secret key pair works only on the legacy API design
    /// litegen left.
    /// @see <https://kling.ai/document-api/api/get-started/authentication.md> (2026-11-03)
    pub fn provider_credential_warnings(&self) -> Vec<String> {
        let Some(kling) = self.providers.get("kling") else { return Vec::new() };
        let has = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
        let has_key = has(&kling.api_key) || has(&kling.api_keys);
        if (has(&kling.key_id) || has(&kling.key_secret)) && !has_key {
            return vec![
                "providers.kling: Kling takes an API key since 2026-11-03 (KLING_API_KEY, or providers.kling.api_key); \
                 the access/secret key pair (KLING_ACCESS_KEY, KLING_SECRET_KEY) no longer authenticates, so Kling is not configured"
                    .to_string(),
            ];
        }
        Vec::new()
    }

    /// Pre-bind security self-check.
    ///
    /// The `auth_middleware` no-credential dev-bypass is only active in
    /// `SingleTenant` mode with no `master_key`; in that combo an anonymous
    /// request is granted full admin scope. If the server is also bound to a
    /// non-loopback address, that admin API is exposed to the network. Flag
    /// exactly that combination so `main` can refuse to start (or require an
    /// explicit opt-in).
    pub fn startup_security_check(&self) -> StartupSecurityCheck {
        // A present-but-blank master_key (empty or whitespace-only, e.g.
        // `LITEGEN__MASTER_KEY=` or a placeholder) is NOT a real secret: the
        // auth middleware compares it with constant_time_eq without trimming,
        // so a `Bearer <whitespace>` request would match and gain full admin.
        // Treat such a key as "no master key" for the exposure check.
        let has_master_key = self
            .master_key
            .as_deref()
            .is_some_and(|k| !k.trim().is_empty());
        if self.mode == Mode::SingleTenant
            && !has_master_key
            && !host_is_loopback(&self.server.host)
        {
            StartupSecurityCheck::UnauthenticatedExposure {
                host: self.server.host.clone(),
            }
        } else {
            StartupSecurityCheck::Ok
        }
    }
}

/// The database litegen opens when no setting names one: `data/litegen.db` in
/// the working directory, beside the kept files (`data/assets`), created with
/// its directory on the first start (`mode=rwc`). In the Docker image that is
/// `/app/data/litegen.db`, inside the `/app/data` volume. A `litegen.db` the
/// old default (`sqlite://litegen.db`) left in the working directory is still
/// used until `data/litegen.db` exists ([`connections::default_database_in`]).
pub const DEFAULT_DATABASE_URL: &str = "sqlite://data/litegen.db?mode=rwc";

fn default_database_url() -> String {
    DEFAULT_DATABASE_URL.to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Retired: accepted and ignored, and named in a startup warning. litegen
    /// sets no limit on a whole request (see `vendor_timeout_secs`,
    /// `header_read_timeout_secs`, `request_body_timeout_secs` and
    /// `shutdown_grace_secs`). `Some` means the config set it.
    #[serde(default)]
    pub request_timeout_seconds: Option<u64>,
    /// Externally reachable origin for URLs litegen mints and hands to clients
    /// (currently 3D asset URLs, which MUST be absolute). Set this behind a
    /// proxy — e.g. `https://app.litegen.ai/api`. Defaults to
    /// `http://{host}:{port}`, which is correct for local dev and CI.
    #[serde(default)]
    pub public_base_url: Option<String>,
    /// Seconds one vendor call may take before litegen stops waiting
    /// (default 120, 5 to 3600). A call that ran out of time is answered 504
    /// and never sent again: the vendor may still finish, and bill, the job.
    #[serde(default, deserialize_with = "vendor_timeout_secs")]
    pub vendor_timeout_secs: Option<u64>,
    /// Seconds a connection may take to send a request's headers (default
    /// 130, 1 to 3600). The same deadline closes a connection that sends
    /// nothing, and a keep-alive connection idle that long, so a reverse
    /// proxy's upstream idle timeout must stay below it (Caddy's is 120 s).
    /// HTTP/1 only: an HTTP/2 connection without TLS (h2c) has neither
    /// deadline once its preface is in.
    #[serde(default, deserialize_with = "header_read_timeout_secs")]
    pub header_read_timeout_secs: Option<u64>,
    /// Seconds a stop (SIGTERM, ctrl-c) gives the open requests and the
    /// poller's current rows before they are cut (1 to 3600; unset: the
    /// effective vendor timeout + 30, which may pass 3600).
    #[serde(default, deserialize_with = "shutdown_grace_secs")]
    pub shutdown_grace_secs: Option<u64>,
    /// Seconds a generation, quote or segmentation body (and an MCP request's)
    /// may take to arrive, whole (default 300, 1 to 3600); a slower one is
    /// answered 408 `body_timeout`. At the default a 25 MiB body needs about
    /// 87 KB/s.
    #[serde(default, deserialize_with = "request_body_timeout_secs")]
    pub request_body_timeout_secs: Option<u64>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            request_timeout_seconds: None,
            public_base_url: None,
            vendor_timeout_secs: None,
            header_read_timeout_secs: None,
            shutdown_grace_secs: None,
            request_body_timeout_secs: None,
        }
    }
}

/// How long a direct vendor call may take when the config does not say.
pub const DEFAULT_VENDOR_TIMEOUT_SECS: u64 = 120;

/// How long a connection may take to send a request's headers, or sit idle
/// between keep-alive requests, when the config does not say: above the
/// upstream idle timeouts of Caddy (120 s), nginx and AWS ALB (60 s), so the
/// proxy closes an idle connection before litegen does.
pub const DEFAULT_HEADER_READ_TIMEOUT_SECS: u64 = 130;

/// How long a request body may take to arrive when the config does not say:
/// Node's `requestTimeout`.
pub const DEFAULT_REQUEST_BODY_TIMEOUT_SECS: u64 = 300;

impl ServerConfig {
    /// The per-call vendor timeout.
    pub fn vendor_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.vendor_timeout_secs.unwrap_or(DEFAULT_VENDOR_TIMEOUT_SECS).max(1))
    }

    /// How long a connection may take to send a request's headers.
    pub fn header_read_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.header_read_timeout_secs.unwrap_or(DEFAULT_HEADER_READ_TIMEOUT_SECS))
    }

    /// How long a request body may take to arrive.
    pub fn request_body_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.request_body_timeout_secs.unwrap_or(DEFAULT_REQUEST_BODY_TIMEOUT_SECS))
    }

    /// How long a stop waits for open requests and the poller's current rows,
    /// given the effective vendor timeout: a synchronous generation started
    /// just before the stop still has its vendor's time, unless set.
    pub fn shutdown_grace(&self, vendor_timeout: std::time::Duration) -> std::time::Duration {
        match self.shutdown_grace_secs {
            Some(secs) => std::time::Duration::from_secs(secs),
            None => vendor_timeout + std::time::Duration::from_secs(30),
        }
    }

    /// The absolute origin to mint client-facing URLs from.
    ///
    /// `host` is a BIND address, not an origin, and the two are only
    /// interchangeable by accident. The default `0.0.0.0` means "listen on
    /// every interface"; minted into a 3D asset URL it is absolute (so every
    /// validity check passes) yet unreachable for a remote client and blocked
    /// outright by Chromium, so a default install would report generations
    /// `completed` with a dead mesh URL. Fall back to loopback for the wildcard
    /// addresses — correct for local dev and CI, which is the only situation
    /// where nobody has set `public_base_url`. Anything else deployed behind a
    /// proxy MUST set it (`LITEGEN__SERVER__PUBLIC_BASE_URL`).
    ///
    /// An empty value counts as unset: compose files pass an unset variable as
    /// `""` (`${VAR:-}`), which config-rs keeps as `Some("")`, and minting from
    /// it would make every asset URL relative.
    pub fn public_base_url(&self) -> String {
        self.public_base_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("http://{}:{}", reachable_host(&self.host), self.port))
            .trim_end_matches('/')
            .to_string()
    }
}

/// Map a wildcard bind address onto an address a client can actually connect
/// to. Everything else is passed through untouched.
fn reachable_host(host: &str) -> &str {
    match host {
        "0.0.0.0" | "::" | "[::]" | "" => "127.0.0.1",
        other => other,
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    4000
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
    /// Access key id (SigV4) / secret id (TC3). Kling's old access key still
    /// loads here only so that `provider_credential_warnings` can name it.
    #[serde(default)]
    pub key_id: Option<String>,
    /// Secret access key (SigV4) / secret key (TC3).
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
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
    /// Default TTL in seconds, 1 to 604800 (a week).
    #[serde(default = "default_cache_ttl", deserialize_with = "cache_ttl_seconds")]
    pub default_ttl_seconds: u64,
    /// Max total cached items, 1 to 1000000.
    #[serde(default = "default_max_items", deserialize_with = "cache_max_items")]
    pub max_items: u64,
    /// About how much memory the cached answers may take on each replica, in
    /// bytes, 1048576 (1 MiB) to 68719476736 (64 GiB); default 268435456
    /// (256 MiB). An answer weighs about its JSON size, and a `b64_json`
    /// image is megabytes. Whichever of this and `max_items` is reached
    /// first evicts.
    #[serde(default = "default_cache_max_bytes", deserialize_with = "cache_max_bytes")]
    pub max_bytes: u64,
}

/// A whole number within `lo..=hi`, or an error naming `field`: config that
/// would make a setting meaningless never loads (see `docs/gateway-settings.md`).
fn in_range<'de, D: serde::Deserializer<'de>>(d: D, field: &str, lo: u64, hi: u64) -> Result<u64, D::Error> {
    let v = u64::deserialize(d)?;
    if !(lo..=hi).contains(&v) {
        return Err(serde::de::Error::custom(format!("{field} must be between {lo} and {hi}, not {v}")));
    }
    Ok(v)
}

fn cache_ttl_seconds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    in_range(d, "cache.default_ttl_seconds", 1, 604_800)
}

fn cache_max_items<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    in_range(d, "cache.max_items", 1, 1_000_000)
}

fn cache_max_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    in_range(d, "cache.max_bytes", 1_048_576, 68_719_476_736)
}

fn vendor_timeout_secs<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    in_range(d, "server.vendor_timeout_secs", 5, 3600).map(Some)
}

fn header_read_timeout_secs<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    in_range(d, "server.header_read_timeout_secs", 1, 3600).map(Some)
}

fn shutdown_grace_secs<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    in_range(d, "server.shutdown_grace_secs", 1, 3600).map(Some)
}

fn request_body_timeout_secs<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    in_range(d, "server.request_body_timeout_secs", 1, 3600).map(Some)
}

fn breaker_threshold<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    in_range(d, "circuit_breaker.threshold", 1, 1000).map(|v| v as u32)
}

fn breaker_open_for_seconds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    in_range(d, "circuit_breaker.open_for_seconds", 1, 3600)
}

/// `retries` in `litegen.yaml`: how a vendor call turned away with a
/// retryable error is tried again (`docs/gateway-settings.md`). Whether a
/// call may be sent again at all is never a setting: never after a timeout,
/// never once the vendor accepted the job.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RetriesInput")]
pub struct RetriesConfig {
    /// Tries after the first, 0 to 5.
    pub max_retries: u32,
    /// The longest first wait, in ms (0 to 60000); each retry may wait up to twice the last.
    pub backoff_ms: u64,
    /// The longest any wait may be, in ms (up to 60000, at least `backoff_ms`).
    pub max_backoff_ms: u64,
}

impl Default for RetriesConfig {
    fn default() -> Self {
        Self { max_retries: 2, backoff_ms: 500, max_backoff_ms: 10_000 }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetriesInput {
    #[serde(default)]
    max_retries: Option<u32>,
    #[serde(default)]
    backoff_ms: Option<u64>,
    #[serde(default)]
    max_backoff_ms: Option<u64>,
}

impl TryFrom<RetriesInput> for RetriesConfig {
    type Error = String;
    fn try_from(r: RetriesInput) -> Result<Self, String> {
        let d = RetriesConfig::default();
        let c = RetriesConfig {
            max_retries: r.max_retries.unwrap_or(d.max_retries),
            backoff_ms: r.backoff_ms.unwrap_or(d.backoff_ms),
            max_backoff_ms: r.max_backoff_ms.unwrap_or(d.max_backoff_ms),
        };
        if c.max_retries > 5 {
            return Err(format!("retries.max_retries must be between 0 and 5, not {}", c.max_retries));
        }
        if c.backoff_ms > 60_000 {
            return Err(format!("retries.backoff_ms must be between 0 and 60000, not {}", c.backoff_ms));
        }
        if c.max_backoff_ms > 60_000 || c.max_backoff_ms < c.backoff_ms {
            return Err(format!(
                "retries.max_backoff_ms must be between retries.backoff_ms ({}) and 60000, not {}",
                c.backoff_ms, c.max_backoff_ms
            ));
        }
        Ok(c)
    }
}

impl Default for CacheGlobalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_ttl_seconds: 3600,
            max_items: 10_000,
            max_bytes: default_cache_max_bytes(),
        }
    }
}

fn default_max_items() -> u64 {
    10_000
}

/// 256 MiB: the cache's memory per replica when nothing sets it (P7, code
/// review 2026-11-11). It holds about 90 answers that carry a 2.8 MB
/// `b64_json` image, or many thousands of URL answers.
fn default_cache_max_bytes() -> u64 {
    268_435_456
}

#[derive(Debug, Clone, Deserialize)]
pub struct ObservabilityConfig {
    /// Log level (trace, debug, info, warn, error).
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Enable JSON structured logging.
    #[serde(default)]
    pub json_logs: bool,
    /// Serve Prometheus metrics (default true). `false` mounts no metrics
    /// route and records nothing.
    #[serde(default = "default_true")]
    pub metrics_enabled: bool,
    /// Where the metrics are served (default `/metrics`): a static path that
    /// litegen does not already serve (see [`RESERVED_METRICS_SEGMENTS`]).
    #[serde(default = "default_metrics_path", deserialize_with = "metrics_path")]
    pub metrics_path: String,
    /// Whether the metrics need a credential that may read the deployment
    /// (default false): the master key, or in single-tenant mode a key with
    /// `read` in the deployment's org. LiteLLM's
    /// `require_auth_for_metrics_endpoint`.
    #[serde(default)]
    pub metrics_require_auth: bool,
}

/// The first path segments litegen serves itself, which
/// `observability.metrics_path` may not start with.
/// `every_route_litegen_serves_is_kept_from_metrics_path` keeps it whole.
pub const RESERVED_METRICS_SEGMENTS: [&str; 9] = ["v1", "health", "openapi.json", "mock", "auth", "mcp", "admin", "oauth", ".well-known"];

/// `observability.metrics_path`: an absolute path of plain segments
/// (`A-Za-z0-9._~-`, no `.` or `..`, no empty one, no trailing `/`), not `/`,
/// whose first segment is not one litegen serves. Anything else would
/// overlap a route (axum panics at startup), capture other paths, or be
/// rewritten by clients and proxies before it arrives.
fn metrics_path<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let path = String::deserialize(d)?;
    let servable = path.strip_prefix('/').is_some_and(|rest| {
        let segments: Vec<&str> = rest.split('/').collect();
        segments.iter().all(|s| {
            !s.is_empty() && *s != "." && *s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._~-".contains(&b))
        }) && !RESERVED_METRICS_SEGMENTS.contains(&segments[0])
    });
    if !servable {
        return Err(serde::de::Error::custom(format!(
            "observability.metrics_path must be a static path litegen does not already serve, such as /metrics or /internal/metrics, not {path:?}"
        )));
    }
    Ok(path)
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            json_logs: false,
            metrics_enabled: true,
            metrics_path: default_metrics_path(),
            metrics_require_auth: false,
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
    /// Number of consecutive failures before opening the breaker (1 to 1000).
    #[serde(default = "default_cb_threshold", deserialize_with = "breaker_threshold")]
    pub threshold: u32,
    /// Seconds to keep the breaker open before calls go through again; the
    /// first failure after that opens it for another window (1 to 3600).
    #[serde(default = "default_cb_open_for", deserialize_with = "breaker_open_for_seconds")]
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

/// `storage_access` in `litegen.yaml`: how the URLs litegen hands out for
/// stored files work, unless the gateway setting or an app's own says
/// otherwise (`docs/storage-access.md`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageAccessConfig {
    /// `public` (default), `signed` or `private`.
    #[serde(default)]
    pub mode: crate::access::AccessMode,
    /// Which keys may open a file: `key` (default, the one that made it),
    /// `app` or `org`.
    #[serde(default)]
    pub owner_scope: crate::access::OwnerScope,
    /// How long a signed link works: 60 to 604800 seconds (a week).
    #[serde(default = "default_signed_url_ttl_seconds", deserialize_with = "signed_url_ttl_seconds")]
    pub signed_url_ttl_seconds: u32,
    /// Whether litegen keeps a copy of each output (default `true`). `false`
    /// keeps nothing: answers carry the vendor's own link, or the image itself.
    #[serde(default = "default_keep_outputs")]
    pub keep_outputs: bool,
    /// Days after which kept files are deleted, 0 to 36500; 0 (the default)
    /// keeps them until they are deleted.
    #[serde(default, deserialize_with = "retention_days")]
    pub retention_days: u32,
    /// Who signs a signed link: `store` (default) or `litegen`.
    #[serde(default)]
    pub signed_links: crate::access::SignedLinks,
    /// Tests only: fetch a vendor's file (an output litegen didn't keep)
    /// without the public-address check, so wiremock on 127.0.0.1 answers. The
    /// connect-time guard still refuses a *name* that resolves inside; an IP
    /// literal never reaches it. No config file or variable can set this; in a
    /// running litegen, vendor files come from public hosts.
    #[doc(hidden)]
    #[serde(skip)]
    pub unchecked_vendor_hosts: bool,
    /// Tests only: skip the public-address check on a hosted app's own bucket,
    /// so wiremock on 127.0.0.1 answers. Its requests still go through the
    /// guarded client, which refuses a *name* that resolves inside (an IP
    /// literal never reaches it). No config file or variable can set this; in
    /// a running hosted litegen, an app's bucket must be on a public address.
    #[doc(hidden)]
    #[serde(skip)]
    pub unchecked_app_storage_hosts: bool,
}

impl Default for StorageAccessConfig {
    fn default() -> Self {
        Self {
            mode: crate::access::AccessMode::default(),
            owner_scope: crate::access::OwnerScope::default(),
            signed_url_ttl_seconds: default_signed_url_ttl_seconds(),
            keep_outputs: default_keep_outputs(),
            retention_days: 0,
            signed_links: crate::access::SignedLinks::default(),
            unchecked_vendor_hosts: false,
            unchecked_app_storage_hosts: false,
        }
    }
}

fn default_signed_url_ttl_seconds() -> u32 {
    3600
}

fn default_keep_outputs() -> bool {
    true
}

fn retention_days<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let v = i64::deserialize(d)?;
    if !crate::access::retention_in_range(v) {
        return Err(serde::de::Error::custom(format!(
            "storage_access.retention_days must be between 0 and {} days, not {v}",
            crate::access::MAX_RETENTION_DAYS,
        )));
    }
    Ok(v as u32)
}

fn signed_url_ttl_seconds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let v = i64::deserialize(d)?;
    if !crate::access::ttl_in_range(v) {
        return Err(serde::de::Error::custom(format!(
            "storage_access.signed_url_ttl_seconds must be between {} and {} seconds, not {v}",
            crate::access::MIN_SIGNED_URL_TTL_SECONDS,
            crate::access::MAX_SIGNED_URL_TTL_SECONDS,
        )));
    }
    Ok(v as u32)
}

/// Whether `storage_access` is set, in the config file litegen read (`file`,
/// from [`config_file`]) or in a `LITEGEN__STORAGE_ACCESS__*` variable of
/// `env`. litegen refuses to start on its defaults when a config that sets it
/// fails to load: the default is `public`, so falling back would open files
/// the operator meant to close.
pub fn storage_access_in(file: Option<&ConfigFile>, mut env: impl Iterator<Item = (String, String)>) -> bool {
    if env.any(|(k, _)| k.to_ascii_uppercase().starts_with("LITEGEN__STORAGE_ACCESS")) {
        return true;
    }
    file.is_some_and(|f| f.get::<config::Value>("storage_access").is_ok_and(|v| v.is_some()))
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
    /// Where the local backend keeps stored assets (default `data/assets`,
    /// relative to the working directory). Stored files outlive a restart, so
    /// a chain of operations that reads its source by generation id survives
    /// one. Unused by the S3 backend, whose retention is the bucket's.
    #[serde(default)]
    pub local_dir: Option<String>,
    /// Days the local backend keeps a stored asset. Never below 7: a smaller
    /// value is raised to 7.
    #[serde(default)]
    pub retention_days: Option<u32>,
}

impl Default for ImageStorageConfig {
    fn default() -> Self {
        Self {
            backend: default_storage_backend(),
            s3: None,
            path_prefix: None,
            local_dir: None,
            retention_days: None,
        }
    }
}

/// The floor on `retention_days`. Clients chain operations by generation id
/// and re-download results days later; PixelGenie's contract is a week.
pub const MIN_ASSET_RETENTION_DAYS: u32 = 7;

impl ImageStorageConfig {
    /// The local backend's directory.
    pub fn local_dir(&self) -> std::path::PathBuf {
        match self.local_dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            Some(d) => std::path::PathBuf::from(d),
            None => default_local_dir(),
        }
    }

    /// Days to keep a locally stored asset, raised to the 7-day floor.
    pub fn retention_days(&self) -> u32 {
        self.retention_days.unwrap_or(MIN_ASSET_RETENTION_DAYS).max(MIN_ASSET_RETENTION_DAYS)
    }
}

#[cfg(not(test))]
fn default_local_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("data/assets")
}

/// Unit tests build routers from default configs all over the crate; keep
/// what they store out of the source tree.
#[cfg(test)]
fn default_local_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("litegen-unit-test-assets")
}

fn default_storage_backend() -> String {
    "local".to_string()
}

/// S3-compatible storage configuration.
/// Works with AWS S3, MinIO, Cloudflare R2, DigitalOcean Spaces, etc.
#[derive(Debug, Clone, Deserialize)]
pub struct S3StorageConfig {
    /// S3 bucket name (or `LITEGEN_S3_BUCKET`). Required when
    /// `image_storage.backend` is `s3`: litegen refuses to start without one.
    #[serde(default)]
    pub bucket_name: String,
    /// AWS region (e.g. "us-east-1") or region identifier for compatible services.
    #[serde(default = "default_s3_region")]
    pub region: String,
    /// AWS access key ID (or `LITEGEN_S3_ACCESS_KEY_ID`). Unset or blank, the
    /// S3 client takes its credentials from the AWS chain: `AWS_ACCESS_KEY_ID`,
    /// `AWS_SECRET_ACCESS_KEY` and `AWS_SESSION_TOKEN`, then a profile, then
    /// web identity when `AWS_ROLE_ARN` and `AWS_WEB_IDENTITY_TOKEN_FILE` are
    /// set, then a container or instance role. Set, it needs
    /// `secret_access_key` too: S3 storage with a key and no secret refuses to
    /// start.
    #[serde(default)]
    pub access_key_id: Option<String>,
    /// AWS secret access key (or `LITEGEN_S3_SECRET_ACCESS_KEY`); unset or
    /// blank, the AWS chain's, as for `access_key_id`.
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
    /// The session token of temporary credentials (an assumed role's); a
    /// blank one counts as unset.
    #[serde(default)]
    pub session_token: Option<String>,
    /// When temporary credentials stop working: a presigned link that would
    /// outlive them isn't minted. Never set from config.
    #[serde(skip)]
    pub credentials_expire_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn default_s3_region() -> String {
    "us-east-1".to_string()
}

/// `aws` in `litegen.yaml`: litegen's own AWS identity, which assumes the
/// roles apps' buckets trust (docs/storage-access.md). Unset keys use the
/// standard chain: environment, profile, instance or container metadata,
/// web identity.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AwsConfig {
    #[serde(default)]
    pub access_key_id: Option<String>,
    #[serde(default)]
    pub secret_access_key: Option<String>,
    #[serde(default)]
    pub session_token: Option<String>,
    /// The STS region (default us-east-1).
    #[serde(default)]
    pub region: Option<String>,
    /// STS's URL (default `https://sts.{region}.amazonaws.com`).
    #[serde(default)]
    pub sts_endpoint: Option<String>,
    /// The principal a customer's role must trust, shown with each app's
    /// external ID. Unset, litegen asks STS (`GetCallerIdentity`) when its
    /// keys or STS endpoint are configured.
    #[serde(default)]
    pub principal_arn: Option<String>,
}

/// Load configuration as `main` does with no arguments: in the process's
/// working directory, from its environment ([`load_config_from`]).
pub fn load_config() -> Result<AppConfig, config::ConfigError> {
    let cwd = std::env::current_dir().map_err(|e| config::ConfigError::Message(format!("cannot read the working directory: {e}")))?;
    let env: HashMap<String, String> = std::env::vars().collect();
    load_config_from(&cwd, None, &env)
}

/// The vendors' own variables (`OPENAI_API_KEY`, `FAL_KEY`, …), each with the
/// setting it fills. They rank above the config file and below `LITEGEN__`
/// variables (`docs/configuration.md`). A blank one counts as unset, so
/// compose's `${OPENAI_API_KEY:-}` never blanks a key set elsewhere. Of two
/// names for one setting, the later in the table wins (`ARK_API_KEY` over
/// `BYTEDANCE_API_KEY`), and a startup warning names the other. A new
/// vendor's variables go in this table.
fn load_provider_env_overrides(env: &HashMap<String, String>) -> Vec<load::Setting> {
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
        ("PIXELLAB_API_KEY", "providers.pixellab.api_key"),
        ("MINIMAX_API_KEY", "providers.minimax.api_key"),
        ("MINIMAX_API_BASE", "providers.minimax.api_base"),
        ("BYTEDANCE_API_KEY", "providers.bytedance.api_key"),
        ("BYTEDANCE_API_BASE", "providers.bytedance.api_base"),
        ("ARK_API_KEY", "providers.bytedance.api_key"),
        ("VIDU_API_KEY", "providers.vidu.api_key"),
        ("PIXVERSE_API_KEY", "providers.pixverse.api_key"),
        ("LEONARDO_API_KEY", "providers.leonardo.api_key"),
        // Tripo's own SDKs read TRIPO_API_KEY; a China-mainland key sets the
        // base to https://openapi.tripo3d.com/v3.
        ("TRIPO_API_KEY", "providers.tripo.api_key"),
        ("TRIPO_API_BASE", "providers.tripo.api_base"),
        ("MESHY_API_KEY", "providers.meshy.api_key"),
        // Tencent TokenHub: one Bearer API key for every model ("鉴权方式为请求头
        // Authorization: Bearer YOUR_API_KEY"); the TC3 pair below is the old
        // Tencent Cloud API's (`hunyuan`).
        ("TOKENHUB_API_KEY", "providers.tokenhub.api_key"),
        ("TOKENHUB_API_KEYS", "providers.tokenhub.api_keys"),
        ("TOKENHUB_API_BASE", "providers.tokenhub.api_base"),
        // Kling: an API key since 2026-11-03 ("API Key authentication must be
        // used to call Kling AI API"). The old pair still loads, only so that
        // `provider_credential_warnings` can name it at startup.
        ("KLING_API_KEY", "providers.kling.api_key"),
        ("KLING_ACCESS_KEY", "providers.kling.key_id"),
        ("KLING_SECRET_KEY", "providers.kling.key_secret"),
        // ── Signing-scheme vendors (key_id/key_secret/region) ──
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
        // Not AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY: ambient credentials
        // that hosts set for every AWS tool. The S3 backend reads them itself,
        // with AWS_SESSION_TOKEN, through the AWS credential chain when no key
        // is configured. litegen's own LITEGEN_S3_* and LITEGEN_ASSET_* rank
        // higher: `load::LITEGEN_ALIASES`.
    ];
    load::named_settings(env, &env_mappings)
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
        map.serialize_entry("public_base_url", &self.0.public_base_url)?;
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_config_errors_name_the_provider_and_the_header() {
        let mut cfg = AppConfig::default();
        let provider = |headers: serde_json::Value| -> ProviderEnvConfig {
            serde_json::from_value(serde_json::json!({ "api_key": "k", "extra_headers": headers })).unwrap()
        };
        cfg.providers.insert("openai".into(), provider(serde_json::json!({ "OpenAI-Organization": "org-1" })));
        assert!(cfg.provider_config_errors().is_empty());
        cfg.providers.insert("fal".into(), provider(serde_json::json!({ "Host": "evil.example" })));
        let errors = cfg.provider_config_errors();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("fal") && errors[0].contains("Host"), "{errors:?}");
    }

    /// Kling takes an API key since 2026-11-03 ("API Key authentication must
    /// be used to call Kling AI API"): a config still holding only the old
    /// access/secret key pair is named at startup instead of failing quietly
    /// at the first call.
    /// @see <https://kling.ai/document-api/api/get-started/authentication.md> (2026-11-03)
    #[test]
    fn a_kling_key_pair_without_an_api_key_is_named() {
        let mut cfg = AppConfig::default();
        let kling = |v: serde_json::Value| -> ProviderEnvConfig { serde_json::from_value(v).unwrap() };
        cfg.providers.insert("kling".into(), kling(serde_json::json!({ "key_id": "ak", "key_secret": "sk" })));
        let warnings = cfg.provider_credential_warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("KLING_API_KEY"), "{warnings:?}");
        cfg.providers.insert("kling".into(), kling(serde_json::json!({ "api_key": "k", "key_id": "ak", "key_secret": "sk" })));
        assert!(cfg.provider_credential_warnings().is_empty());
    }

    #[test]
    fn an_empty_public_base_url_counts_as_unset() {
        // Compose passes an unset `${VAR:-}` as "", which config-rs keeps; it
        // must not turn every minted asset URL relative.
        let mut s = ServerConfig { port: 4000, host: "0.0.0.0".into(), public_base_url: Some("".into()), ..Default::default() };
        assert_eq!(s.public_base_url(), "http://127.0.0.1:4000");
        s.public_base_url = Some("  ".into());
        assert_eq!(s.public_base_url(), "http://127.0.0.1:4000");
        s.public_base_url = Some("https://app.litegen.ai/api/".into());
        assert_eq!(s.public_base_url(), "https://app.litegen.ai/api");
    }

    /// P11 and P9: the server's deadlines. The header deadline sits above the
    /// upstream idle timeouts of common proxies (Caddy 120 s, nginx and ALB
    /// 60 s), so a proxy always closes an idle connection first; the stop's
    /// grace follows the vendor timeout unless set, and may pass 3600 then.
    #[test]
    fn the_server_deadlines_have_their_defaults_and_ranges() {
        use std::time::Duration;
        let s = ServerConfig::default();
        assert_eq!(s.header_read_timeout(), Duration::from_secs(130));
        assert_eq!(s.shutdown_grace(Duration::from_secs(120)), Duration::from_secs(150), "unset: the vendor timeout + 30 s");
        assert_eq!(s.shutdown_grace(Duration::from_secs(3600)), Duration::from_secs(3630));
        let set: ServerConfig = serde_json::from_value(serde_json::json!({ "header_read_timeout_secs": 20, "shutdown_grace_secs": 25 })).unwrap();
        assert_eq!(set.header_read_timeout(), Duration::from_secs(20));
        assert_eq!(set.shutdown_grace(Duration::from_secs(120)), Duration::from_secs(25), "set: whatever the vendor timeout");
        for field in ["header_read_timeout_secs", "shutdown_grace_secs"] {
            for bad in [0, 3601] {
                let refused = serde_json::from_value::<ServerConfig>(serde_json::json!({ field: bad })).map(|_| ()).unwrap_err().to_string();
                assert!(refused.contains(&format!("server.{field} must be between 1 and 3600, not {bad}")), "{field} = {bad}: {refused}");
            }
        }
    }

    /// P9: a request body gets 300 s to arrive (Node's `requestTimeout`) unless
    /// set; a 25 MiB body then needs about 87 KB/s.
    #[test]
    fn the_request_body_deadline_has_its_default_and_range() {
        use std::time::Duration;
        assert_eq!(ServerConfig::default().request_body_timeout(), Duration::from_secs(300));
        let set: ServerConfig = serde_json::from_value(serde_json::json!({ "request_body_timeout_secs": 45 })).unwrap();
        assert_eq!(set.request_body_timeout(), Duration::from_secs(45));
        for bad in [0, 3601] {
            let refused = serde_json::from_value::<ServerConfig>(serde_json::json!({ "request_body_timeout_secs": bad })).map(|_| ()).unwrap_err().to_string();
            assert!(refused.contains(&format!("server.request_body_timeout_secs must be between 1 and 3600, not {bad}")), "{bad}: {refused}");
        }
    }

    /// S7: `observability.metrics_path` is honoured, so it must be a static
    /// path litegen does not already serve: one that overlaps a route would
    /// stop startup (axum panics on the overlap), a `{capture}` would match
    /// other paths, and a `.` or `..` segment is one clients and proxies
    /// rewrite before it arrives.
    #[test]
    fn a_metrics_path_litegen_cannot_serve_is_refused() {
        let bad_paths = [
            "", "metrics", "/", "/v1/metrics", "/health", "/mcp", "/admin/metrics", "/oauth", "/auth/m", "/mock/m",
            "/openapi.json", "/.well-known/x", "/x/{id}", "/a b", "/metrics/", "//metrics", "/../metrics", "/a/./b",
            "/a/../b", "/metrics?x=1", "/métrics",
        ];
        for bad in bad_paths {
            match serde_json::from_value::<ObservabilityConfig>(serde_json::json!({ "metrics_path": bad })) {
                Ok(_) => panic!("metrics_path {bad:?} loaded"),
                Err(e) => assert!(e.to_string().contains("observability.metrics_path"), "{bad:?}: {e}"),
            }
        }
        for good in ["/metrics", "/internal/metrics", "/prometheus", "/x-metrics_1.0~"] {
            let ok: ObservabilityConfig = serde_json::from_value(serde_json::json!({ "metrics_path": good })).unwrap_or_else(|e| panic!("{good:?}: {e}"));
            assert_eq!(ok.metrics_path, good);
        }
    }

    /// Guard for the reserved first segments: a `metrics_path` equal to any
    /// path litegen serves (each capture filled in) is refused, so a new
    /// top-level route the list misses fails here.
    #[test]
    fn every_route_litegen_serves_is_kept_from_metrics_path() {
        use utoipa::OpenApi;
        let mut paths: Vec<String> = crate::api::openapi::ApiDoc::openapi().paths.paths.keys().cloned().collect();
        paths.extend(
            ["/health/live", "/health/ready", "/mcp", "/admin/mcp", "/auth/redirect", "/mock/video/x", "/oauth/token", "/.well-known/oauth-authorization-server", "/openapi.json"]
                .map(String::from),
        );
        for path in paths {
            let concrete: String = path
                .split('/')
                .map(|s| if s.starts_with('{') { "x" } else { s })
                .collect::<Vec<_>>()
                .join("/");
            let refused = serde_json::from_value::<ObservabilityConfig>(serde_json::json!({ "metrics_path": concrete })).is_err();
            assert!(refused, "metrics_path {concrete:?} (litegen serves {path}) loaded");
        }
    }

    #[test]
    fn mode_defaults_to_single_tenant_and_parses_env() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.mode, Mode::SingleTenant);
        assert_eq!(Mode::parse("hosted"), Some(Mode::Hosted));
        assert_eq!(Mode::parse("single_tenant"), Some(Mode::SingleTenant));
        assert_eq!(Mode::parse("bogus"), None);
    }

    #[test]
    fn auth_allow_password_defaults_to_true() {
        let cfg = AppConfig::default();
        assert!(cfg.auth.allow_password, "password auth must default to enabled");
        assert!(AuthConfig::default().allow_password);
    }

    #[test]
    fn default_config_is_flagged_as_unauthenticated_network_exposure() {
        // Default = SingleTenant + no master_key + 0.0.0.0 bind. In that combo
        // the auth_middleware no-credential dev-bypass grants full admin scope
        // to anonymous requests, and 0.0.0.0 exposes it to the network.
        let cfg = AppConfig::default();
        assert!(
            matches!(
                cfg.startup_security_check(),
                StartupSecurityCheck::UnauthenticatedExposure { .. }
            ),
            "default config must be flagged as an unauthenticated network exposure"
        );
    }

    #[test]
    fn setting_master_key_makes_startup_safe() {
        let cfg = AppConfig { master_key: Some("a-long-random-master-secret".to_string()), ..Default::default() };
        assert!(matches!(cfg.startup_security_check(), StartupSecurityCheck::Ok));
    }

    #[test]
    fn loopback_bind_without_master_key_is_safe() {
        // Bound to loopback only -> not reachable from the network, so the
        // no-auth bypass is acceptable (local dev).
        for host in ["127.0.0.1", "localhost", "::1", "[::1]"] {
            let mut cfg = AppConfig::default();
            cfg.server.host = host.to_string();
            assert!(
                matches!(cfg.startup_security_check(), StartupSecurityCheck::Ok),
                "loopback host {host:?} must be safe to start without a master key"
            );
        }
    }

    #[test]
    fn hosted_mode_without_master_key_is_safe_to_start() {
        // In Hosted mode the no-auth dev-bypass is disabled (anonymous requests
        // get 401), so a missing master key is not a network exposure of admin.
        let cfg = AppConfig { mode: Mode::Hosted, ..Default::default() };
        assert!(matches!(cfg.startup_security_check(), StartupSecurityCheck::Ok));
    }

    #[test]
    fn blank_or_whitespace_master_key_is_treated_as_no_auth() {
        // A present-but-blank master key (LITEGEN__MASTER_KEY= or a whitespace
        // placeholder) is NOT a real secret: the middleware accepts the blank
        // token as full admin (constant_time_eq does not trim). The guard must
        // treat it as "no master key" and flag the exposure, not return Ok.
        for blank in ["", "   ", "\t", "\n "] {
            // SingleTenant + 0.0.0.0
            let cfg = AppConfig { master_key: Some(blank.to_string()), ..Default::default() };
            assert!(
                matches!(
                    cfg.startup_security_check(),
                    StartupSecurityCheck::UnauthenticatedExposure { .. }
                ),
                "blank master_key {blank:?} must be flagged as exposure, not treated as auth"
            );
        }
    }

    fn load_yaml(yaml: &str) -> Result<AppConfig, config::ConfigError> {
        config::Config::builder()
            .add_source(config::File::from_str(yaml, config::FileFormat::Yaml))
            .build()?
            .try_deserialize()
    }

    #[test]
    fn storage_access_defaults_to_public_key_and_an_hour() {
        use crate::access::{AccessMode, OwnerScope};
        let cfg = load_yaml("cost_markup_percent: 0\n").unwrap();
        assert_eq!(cfg.storage_access.mode, AccessMode::Public);
        assert_eq!(cfg.storage_access.owner_scope, OwnerScope::Key);
        assert_eq!(cfg.storage_access.signed_url_ttl_seconds, 3600);
        let cfg = load_yaml("storage_access:\n  mode: signed\n").unwrap();
        assert_eq!(cfg.storage_access.mode, AccessMode::Signed);
        assert_eq!(cfg.storage_access.owner_scope, OwnerScope::Key);
        assert_eq!(cfg.storage_access.signed_url_ttl_seconds, 3600);
        let cfg = load_yaml("storage_access:\n  mode: private\n  owner_scope: org\n  signed_url_ttl_seconds: 600\n").unwrap();
        assert_eq!(
            (cfg.storage_access.mode, cfg.storage_access.owner_scope, cfg.storage_access.signed_url_ttl_seconds),
            (AccessMode::Private, OwnerScope::Org, 600)
        );
    }

    #[test]
    fn storage_access_refuses_a_ttl_outside_a_minute_to_a_week() {
        for bad in [0, 59, 604_801] {
            let e = load_yaml(&format!("storage_access:\n  signed_url_ttl_seconds: {bad}\n")).unwrap_err().to_string();
            assert!(e.contains("signed_url_ttl_seconds") && e.contains("60") && e.contains("604800"), "{bad}: {e}");
        }
        for good in [60, 604_800] {
            let cfg = load_yaml(&format!("storage_access:\n  signed_url_ttl_seconds: {good}\n")).unwrap();
            assert_eq!(cfg.storage_access.signed_url_ttl_seconds, good);
        }
    }

    #[test]
    fn storage_access_refuses_unknown_fields_and_values() {
        let e = load_yaml("storage_access:\n  mode: privat\n").unwrap_err().to_string();
        assert!(e.contains("privat"), "{e}");
        let e = load_yaml("storage_access:\n  owner_scope: team\n").unwrap_err().to_string();
        assert!(e.contains("team"), "{e}");
        let e = load_yaml("storage_access:\n  ttl: 60\n").unwrap_err().to_string();
        assert!(e.contains("ttl"), "{e}");
    }

    #[test]
    fn storage_access_keeps_everything_forever_by_default_and_loads_the_new_fields() {
        use crate::access::SignedLinks;
        let cfg = load_yaml("cost_markup_percent: 0\n").unwrap();
        assert!(cfg.storage_access.keep_outputs);
        assert_eq!(cfg.storage_access.retention_days, 0);
        assert_eq!(cfg.storage_access.signed_links, SignedLinks::Store);
        let cfg = load_yaml("storage_access:\n  keep_outputs: false\n  retention_days: 36500\n  signed_links: litegen\n").unwrap();
        assert!(!cfg.storage_access.keep_outputs);
        assert_eq!(cfg.storage_access.retention_days, 36_500);
        assert_eq!(cfg.storage_access.signed_links, SignedLinks::Litegen);
    }

    #[test]
    fn storage_access_refuses_bad_new_fields() {
        let e = load_yaml("storage_access:\n  keep_outputs: maybe\n").unwrap_err().to_string();
        assert!(e.contains("keep_outputs") || e.contains("maybe"), "{e}");
        let e = load_yaml("storage_access:\n  retention_days: 36501\n").unwrap_err().to_string();
        assert!(e.contains("retention_days") && e.contains("36500"), "{e}");
        let e = load_yaml("storage_access:\n  signed_links: cdn\n").unwrap_err().to_string();
        assert!(e.contains("cdn"), "{e}");
    }

    #[test]
    fn retries_default_to_two_with_half_a_second_backoff_and_refuse_bad_values() {
        let cfg = load_yaml("cost_markup_percent: 0\n").unwrap();
        assert_eq!((cfg.retries.max_retries, cfg.retries.backoff_ms, cfg.retries.max_backoff_ms), (2, 500, 10_000));
        let cfg = load_yaml("retries:\n  max_retries: 0\n  backoff_ms: 0\n").unwrap();
        assert_eq!((cfg.retries.max_retries, cfg.retries.backoff_ms), (0, 0));
        // The cache, the vendor timeout and the breaker are held to their ranges too.
        for (yaml, field) in [
            ("retries:\n  max_retries: 6\n", "max_retries"),
            ("retries:\n  backoff_ms: 60001\n", "backoff_ms"),
            ("retries:\n  backoff_ms: 2000\n  max_backoff_ms: 1000\n", "max_backoff_ms"),
            ("retries:\n  attempts: 3\n", "attempts"),
            ("cache:\n  default_ttl_seconds: 0\n", "default_ttl_seconds"),
            ("cache:\n  max_items: 0\n", "max_items"),
            ("cache:\n  max_items: 1000001\n", "max_items"),
            ("cache:\n  max_bytes: 1024\n", "max_bytes"),
            ("cache:\n  max_bytes: 68719476737\n", "max_bytes"),
            ("server:\n  vendor_timeout_secs: 4\n", "vendor_timeout_secs"),
            ("circuit_breaker:\n  threshold: 0\n", "threshold"),
            ("circuit_breaker:\n  open_for_seconds: 3601\n", "open_for_seconds"),
        ] {
            let e = load_yaml(yaml).unwrap_err().to_string();
            assert!(e.contains(field), "{yaml}: {e}");
        }
        let cfg = load_yaml("cache:\n  enabled: true\n  default_ttl_seconds: 60\n  max_items: 5\nserver:\n  vendor_timeout_secs: 5\ncircuit_breaker:\n  threshold: 1\n  open_for_seconds: 3600\n").unwrap();
        assert_eq!((cfg.cache.default_ttl_seconds, cfg.cache.max_items, cfg.server.vendor_timeout_secs), (60, 5, Some(5)));
        // The cache's byte budget (P7): 256 MiB unless set, both ends of its range load.
        assert_eq!(cfg.cache.max_bytes, 268_435_456);
        assert_eq!(load_yaml("cache:\n  max_bytes: 1048576\n").unwrap().cache.max_bytes, 1_048_576);
        assert_eq!(load_yaml("cache:\n  max_bytes: 68719476736\n").unwrap().cache.max_bytes, 68_719_476_736);
    }

    #[test]
    fn storage_access_is_noticed_in_the_file_or_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        let file = |dir: &std::path::Path| config_file(dir, None, &HashMap::new()).unwrap();
        assert!(!storage_access_in(file(dir.path()).as_ref(), std::iter::empty()));
        std::fs::write(dir.path().join("litegen.yaml"), "cost_markup_percent: lots\n").unwrap();
        assert!(!storage_access_in(file(dir.path()).as_ref(), std::iter::empty()));
        std::fs::write(dir.path().join("litegen.yaml"), "storage_access:\n  mode: privat\n").unwrap();
        assert!(storage_access_in(file(dir.path()).as_ref(), std::iter::empty()));
        let empty = tempfile::tempdir().unwrap();
        let env = [("LITEGEN__STORAGE_ACCESS__MODE".to_string(), "privat".to_string())];
        assert!(storage_access_in(file(empty.path()).as_ref(), env.into_iter()));
        let env = [("LITEGEN__SERVER__PORT".to_string(), "4000".to_string())];
        assert!(!storage_access_in(file(empty.path()).as_ref(), env.into_iter()));
    }

    #[test]
    fn nonblank_master_key_with_surrounding_space_is_still_a_real_key() {
        // A key with real content (even if padded) is a genuine secret -> safe.
        let cfg = AppConfig { master_key: Some("  s3cret-key  ".to_string()), ..Default::default() };
        assert!(matches!(cfg.startup_security_check(), StartupSecurityCheck::Ok));
    }
}
