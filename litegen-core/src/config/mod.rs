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
    /// Deprecated: also adds `_dev_token`, the same value as `token`, to the
    /// answer that makes an invitation, which carries the token without it
    /// since 2026-11-15.
    #[serde(default)] pub expose_invite_tokens: bool,
    /// Development only: the self-service reset request
    /// (`POST /v1/auth/password-reset/request`) answers with its token as
    /// `_dev_token`. `LITEGEN__DEV__EXPOSE_RESET_TOKENS` sets it too.
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
    /// The one address the first single-tenant signup may use
    /// (`LITEGEN__OWNER_EMAIL`), read once at start like every setting; see
    /// [`AppConfig::owner_email`]. Start-up refuses one that isn't an address
    /// while that signup is still to come (`auth::email::owner_email_problem`).
    #[serde(default)]
    pub owner_email: Option<String>,
    /// Generation call hooks (`docs/hooks.md`): Python, Node or Bun files run
    /// around the generation routes, in this order. Read entry by entry, so
    /// an entry's error names it.
    #[serde(default, deserialize_with = "crate::hooks::config::hook_entries")]
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
    /// Where keys' webhooks may point (`docs/management-api.md`).
    #[serde(default)]
    pub webhooks: WebhooksConfig,
    /// How long the request log is kept (`docs/RUNBOOK.md`).
    #[serde(default)]
    pub logs: LogsConfig,
    /// How litegen reads the 3D files it converts: the Draco decoder's switch
    /// and limits (`docs/gateway-settings.md`).
    #[serde(default)]
    pub mesh: MeshConfig,
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
            owner_email: None,
            auth: AuthConfig::default(),
            hooks: Vec::new(),
            storage_access: StorageAccessConfig::default(),
            aws: AwsConfig::default(),
            retries: RetriesConfig::default(),
            mcp: McpConfig::default(),
            webhooks: WebhooksConfig::default(),
            logs: LogsConfig::default(),
            mesh: MeshConfig::default(),
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
    /// `owner_email` as signup compares it: trimmed, and `None` when unset or
    /// empty (a compose file passes an unset variable as `""`).
    pub fn owner_email(&self) -> Option<&str> {
        self.owner_email.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }

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

    /// The dev token flags that are on: setting, variable, what it does.
    fn dev_token_flags_on(&self) -> Vec<(&'static str, &'static str, &'static str)> {
        let mut on = Vec::new();
        if self.dev.expose_reset_tokens {
            on.push((
                "dev.expose_reset_tokens",
                "LITEGEN__DEV__EXPOSE_RESET_TOKENS",
                "answers an unauthenticated reset request with the reset token, so anyone who knows an account's email can take it over",
            ));
        }
        if self.dev.expose_invite_tokens {
            on.push((
                "dev.expose_invite_tokens",
                "LITEGEN__DEV__EXPOSE_INVITE_TOKENS",
                "adds `_dev_token` to the answer that makes an invitation (deprecated: that answer carries the token without it)",
            ));
        }
        on
    }

    /// Why the public could reach this deployment, if it could: hosted mode,
    /// a bind other than loopback, or a public URL that isn't loopback (a
    /// proxy on the same host forwards public traffic to 127.0.0.1, so a
    /// loopback bind alone proves nothing).
    fn public_reach(&self) -> Option<String> {
        if self.mode == Mode::Hosted {
            return Some("it runs in hosted mode".to_string());
        }
        if !host_is_loopback(&self.server.host) {
            return Some(format!("it binds {}, not a loopback address", self.server.host));
        }
        let public_url = self.server.public_base_url.as_deref().map(str::trim).filter(|u| !u.is_empty())?;
        match reqwest::Url::parse(public_url).ok().and_then(|u| u.host_str().map(str::to_string)) {
            Some(host) if host_is_loopback(&host) => None,
            _ => Some(format!("server.public_base_url ({public_url}) is not a loopback address")),
        }
    }

    /// The dev token flags that are on where the public could reach them,
    /// one line each for a FATAL refusal at start (review A7): they are for
    /// local tests only.
    pub fn dev_token_flag_refusals(&self) -> Vec<String> {
        let Some(reach) = self.public_reach() else { return Vec::new() };
        self.dev_token_flags_on()
            .into_iter()
            .map(|(setting, variable, what)| {
                format!("{setting} ({variable}) {what}. It is for local tests only, and the public could reach this server: {reach}")
            })
            .collect()
    }

    /// A warning for each dev token flag that is on.
    pub fn dev_token_flag_warnings(&self) -> Vec<String> {
        self.dev_token_flags_on()
            .into_iter()
            .map(|(setting, variable, what)| format!("SECURITY: {setting} ({variable}) is on: it {what}. It is for local tests only"))
            .collect()
    }

    /// A warning when a single-tenant deployment with no account yet can be
    /// reached from outside with no owner email set (review A5): its first
    /// signup claims the Owner, and nothing proves the address.
    pub fn open_first_signup_warning(&self, no_account_yet: bool) -> Option<String> {
        if self.mode != Mode::SingleTenant || !no_account_yet || self.owner_email().is_some() {
            return None;
        }
        let reach = self.public_reach()?;
        Some(format!(
            "SECURITY: no account exists yet and LITEGEN__OWNER_EMAIL is not set, so whoever signs up first becomes the Owner, \
             and the public could reach this server: {reach}. Set LITEGEN__OWNER_EMAIL before exposing a fresh deployment, or sign up first."
        ))
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
    /// Whether a cache hit is charged the cached answer's price (default
    /// true). False makes hits free, as LiteLLM does; either way the hit's
    /// log row says `cache_hit`.
    #[serde(default = "default_true")]
    pub charge_hits: bool,
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

/// `webhooks` in `litegen.yaml` (review 2026-11-11 SEC-STWEBHOOK).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WebhooksConfig {
    /// Whose keys' webhooks may reach the deployment's own network (private,
    /// loopback and link-local addresses) in single-tenant mode. In hosted
    /// mode no tenant's webhook ever may.
    #[serde(default)]
    pub internal_targets: InternalTargets,
    /// Days the webhook log is kept: the hourly retention sweep deletes an
    /// outbox delivery this long after it ended, and an attempt row this long
    /// after it was made. 0 (the default) keeps them for good, as before the
    /// purge, so an upgrade deletes nothing until this is set; at most 36500.
    /// A pending delivery is never deleted.
    #[serde(default, deserialize_with = "log_retention_days")]
    pub log_retention_days: u32,
}

fn log_retention_days<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let v = i64::deserialize(d)?;
    if !crate::access::retention_in_range(v) {
        return Err(serde::de::Error::custom(format!(
            "webhooks.log_retention_days must be between 0 and {} days, not {v}",
            crate::access::MAX_RETENTION_DAYS,
        )));
    }
    Ok(v as u32)
}

/// `logs` in `litegen.yaml` (`LITEGEN__LOGS__RETENTION_DAYS`,
/// `LITEGEN__LOGS__ARTIFACT_RETENTION_DAYS`): how long the request log is
/// kept. The hourly retention sweep deletes what is older, on the database's
/// clock (`proxy::retention::purge_request_log`). Both 0 unless set, which
/// keeps everything, as before.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "LogsInput")]
pub struct LogsConfig {
    /// Days a request's log row is kept, with its spend tags and its
    /// artifact: 0 keeps them for good; at most 36500. `/v1/logs`,
    /// `/v1/stats` and `/v1/spend/*` read only the rows kept.
    pub retention_days: u32,
    /// Days a request's artifact (its prompt, params and output, up to 2 MiB:
    /// `GET /v1/logs/{id}/artifact`) is kept, when that should be shorter
    /// than its log row: 0 keeps it as long as the row; at most 36500, and no
    /// more than `retention_days` when that is set.
    pub artifact_retention_days: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogsInput {
    #[serde(default)]
    retention_days: Option<i64>,
    #[serde(default)]
    artifact_retention_days: Option<i64>,
}

impl TryFrom<LogsInput> for LogsConfig {
    type Error = String;
    fn try_from(l: LogsInput) -> Result<Self, String> {
        let days = |key: &str, v: Option<i64>| {
            let v = v.unwrap_or(0);
            if crate::access::retention_in_range(v) {
                Ok(v as u32)
            } else {
                Err(format!("{key} must be between 0 and {} days, not {v}", crate::access::MAX_RETENTION_DAYS))
            }
        };
        let c = LogsConfig {
            retention_days: days("logs.retention_days", l.retention_days)?,
            artifact_retention_days: days("logs.artifact_retention_days", l.artifact_retention_days)?,
        };
        if c.retention_days > 0 && c.artifact_retention_days > c.retention_days {
            return Err(format!(
                "logs.artifact_retention_days ({}) must be no more than logs.retention_days ({}): an artifact is kept no longer than its log row",
                c.artifact_retention_days, c.retention_days,
            ));
        }
        Ok(c)
    }
}

/// `webhooks.internal_targets`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InternalTargets {
    /// Only the deployment's own keys: made with the master key (no owner),
    /// or owned by an active Owner or Admin of the deployment's org.
    #[default]
    Admins,
    /// Any key's, as before 2026-11 (opt-in).
    Everyone,
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

/// `mesh` in `litegen.yaml`: how litegen reads the 3D files it converts, on
/// the convert endpoint (`POST /v1/utils/models3d/convert`), in a 3D
/// generation's delivery and in a 3D operation's source mesh alike.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MeshConfig {
    /// Draco-compressed glTF primitives.
    pub draco: DracoConfig,
}

/// `mesh.draco` (`LITEGEN__MESH__DRACO__…`): whether a Draco-compressed glTF
/// primitive (`KHR_draco_mesh_compression`) is decoded, and the most one
/// primitive may hold. The stream's header states its counts before any data,
/// so the counts and their ratio are checked before anything decodes. A
/// primitive past one is refused, naming the limit: 422 `mesh_unsupported` on
/// the convert endpoint, 422 `source_format_unsupported` for an operation's
/// source mesh (before anything is billed), a failed delivery for a
/// generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "DracoInput")]
pub struct DracoConfig {
    /// Decode Draco primitives (default true). Off, a file with one is
    /// refused, naming this setting.
    pub enabled: bool,
    /// Points one primitive may declare or decode: 1 to 4,000,000 (the
    /// scene's vertex cap, the default).
    pub max_points: u32,
    /// Faces one primitive may declare or decode: 1 to 4,000,000 (a third of
    /// the scene's index cap, the default).
    pub max_faces: u32,
    /// Decoded attribute data one primitive may hold, in MB of 1,000,000
    /// bytes: 1 to 192 (default 24).
    pub max_decoded_mb: u32,
    /// Faces a point one primitive's header may declare: 1 to 64 (default
    /// 5; a closed mesh has about 2, a double-sided one that shares its
    /// vertices about 4, and each face a point more may cost a decode about
    /// 0.5 GB more). Hosted deployments that accept uploads from untrusted
    /// users can lower it to 3 (about 2 GB worst case, extrapolated) at the
    /// cost of double-sided meshes.
    pub max_faces_per_point: u32,
}

/// What a Draco refusal names while `mesh.draco.enabled` is false.
pub const DRACO_SWITCH: &str = "the server setting mesh.draco.enabled (LITEGEN__MESH__DRACO__ENABLED)";

/// The most `mesh.draco` takes. Points and faces stop at the scene's own caps,
/// past which the scene refuses first; decoded data at 192 MB, the document's
/// own budget of 48,000,000 decoded values read as 4-byte floats; faces a point
/// at 64, 32 times a closed mesh's.
const DRACO_MAX_POINTS: i64 = crate::mesh::glb::MAX_MERGED_VERTICES as i64;
const DRACO_MAX_FACES: i64 = (crate::mesh::glb::MAX_MERGED_INDICES / 3) as i64;
const DRACO_MAX_DECODED_MB: i64 = 192;
const DRACO_MAX_FACES_PER_POINT: i64 = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DracoInput {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    max_points: Option<i64>,
    #[serde(default)]
    max_faces: Option<i64>,
    #[serde(default)]
    max_decoded_mb: Option<i64>,
    #[serde(default)]
    max_faces_per_point: Option<i64>,
}

impl TryFrom<DracoInput> for DracoConfig {
    type Error = String;
    fn try_from(d: DracoInput) -> Result<Self, String> {
        let defaults = DracoConfig::default();
        let limit = |key: &str, v: Option<i64>, default: u32, max: i64| match v {
            None => Ok(default),
            Some(v) if (1..=max).contains(&v) => Ok(v as u32),
            Some(v) => Err(format!("mesh.draco.{key} must be between 1 and {max}, not {v}")),
        };
        Ok(DracoConfig {
            enabled: d.enabled.unwrap_or(defaults.enabled),
            max_points: limit("max_points", d.max_points, defaults.max_points, DRACO_MAX_POINTS)?,
            max_faces: limit("max_faces", d.max_faces, defaults.max_faces, DRACO_MAX_FACES)?,
            max_decoded_mb: limit("max_decoded_mb", d.max_decoded_mb, defaults.max_decoded_mb, DRACO_MAX_DECODED_MB)?,
            max_faces_per_point: limit("max_faces_per_point", d.max_faces_per_point, defaults.max_faces_per_point, DRACO_MAX_FACES_PER_POINT)?,
        })
    }
}

impl Default for DracoConfig {
    /// The mesh library's own defaults, which every read had before these
    /// were settings, and 5 faces a point.
    fn default() -> Self {
        let d = crate::mesh::DracoOptions::default();
        Self {
            enabled: d.enabled,
            max_points: d.max_points as u32,
            max_faces: d.max_faces as u32,
            max_decoded_mb: (d.max_decoded_bytes / 1_000_000) as u32,
            max_faces_per_point: d.max_faces_per_point as u32,
        }
    }
}

impl DracoConfig {
    /// The options the mesh library reads a Draco primitive with.
    pub fn options(&self) -> crate::mesh::DracoOptions {
        crate::mesh::DracoOptions {
            enabled: self.enabled,
            disabled_by: DRACO_SWITCH,
            max_points: self.max_points as usize,
            max_faces: self.max_faces as usize,
            max_decoded_bytes: self.max_decoded_mb as usize * 1_000_000,
            max_faces_per_point: self.max_faces_per_point as usize,
        }
    }
}

impl Default for CacheGlobalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_ttl_seconds: 3600,
            max_items: 10_000,
            max_bytes: default_cache_max_bytes(),
            charge_hits: true,
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

    /// SEC-STWEBHOOK: `webhooks.internal_targets` is `admins` unless set,
    /// takes `everyone`, and refuses anything else.
    #[test]
    fn webhooks_internal_targets_default_to_admins() {
        assert_eq!(AppConfig::default().webhooks.internal_targets, InternalTargets::Admins);
        let read = |v: serde_json::Value| serde_json::from_value::<WebhooksConfig>(v).map(|w| w.internal_targets);
        assert_eq!(read(serde_json::json!({})).unwrap(), InternalTargets::Admins);
        assert_eq!(read(serde_json::json!({ "internal_targets": "everyone" })).unwrap(), InternalTargets::Everyone);
        assert!(read(serde_json::json!({ "internal_targets": "nobody" })).is_err());
    }

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

    /// Review A7: the dev token flags hand out account takeovers (a reset
    /// token for any email, unauthenticated), so they run only for local
    /// tests: refused in hosted mode, on a bind other than loopback, and
    /// behind a public URL that isn't loopback (a proxy on the same host
    /// forwards public traffic to 127.0.0.1). Whenever one is on, it is named.
    #[test]
    fn the_dev_token_flags_refuse_anywhere_the_public_could_reach() {
        let local = || {
            let mut cfg = AppConfig::default();
            cfg.server.host = "127.0.0.1".into();
            cfg.dev.expose_reset_tokens = true;
            cfg
        };
        assert!(local().dev_token_flag_refusals().is_empty(), "single-tenant, loopback, no public URL");
        let warnings = local().dev_token_flag_warnings();
        assert!(warnings.len() == 1 && warnings[0].contains("dev.expose_reset_tokens"), "{warnings:?}");
        assert!(AppConfig::default().dev_token_flag_warnings().is_empty(), "off: nothing to say");

        let hosted = AppConfig { mode: Mode::Hosted, ..local() };
        let public_bind = AppConfig { server: ServerConfig { host: "0.0.0.0".into(), ..local().server }, ..local() };
        let behind_proxy = AppConfig { server: ServerConfig { public_base_url: Some("https://app.example.com/api".into()), ..local().server }, ..local() };
        for (why, cfg) in [("hosted", hosted), ("a public bind", public_bind), ("a public URL", behind_proxy)] {
            let refusals = cfg.dev_token_flag_refusals();
            assert!(refusals.len() == 1 && refusals[0].contains("dev.expose_reset_tokens") && refusals[0].contains("LITEGEN__DEV__EXPOSE_RESET_TOKENS"), "{why}: {refusals:?}");
        }
        let loopback_url = AppConfig { server: ServerConfig { public_base_url: Some("http://127.0.0.1:5274/api".into()), ..local().server }, ..local() };
        assert!(loopback_url.dev_token_flag_refusals().is_empty(), "a loopback public URL (the e2e's vite proxy)");
        let invite = AppConfig { mode: Mode::Hosted, dev: DevFlags { expose_invite_tokens: true, expose_reset_tokens: false }, ..local() };
        assert!(invite.dev_token_flag_refusals().iter().any(|r| r.contains("dev.expose_invite_tokens")), "the invite flag too");
    }

    /// Review A5: a single-tenant deployment with no account yet lets the
    /// first signup claim its Owner, so one reachable from outside with no
    /// owner email set is named at start.
    #[test]
    fn an_open_first_signup_reachable_from_outside_is_named() {
        let exposed = || {
            let mut cfg = AppConfig::default();
            cfg.server.host = "0.0.0.0".into();
            cfg
        };
        let warning = exposed().open_first_signup_warning(true).expect("named");
        assert!(warning.contains("LITEGEN__OWNER_EMAIL"), "{warning}");
        let behind_proxy = AppConfig { server: ServerConfig { host: "127.0.0.1".into(), public_base_url: Some("https://app.example.com/api".into()), ..exposed().server }, ..exposed() };
        assert!(behind_proxy.open_first_signup_warning(true).is_some(), "behind a public URL");
        assert_eq!(exposed().open_first_signup_warning(false), None, "an account exists");
        assert_eq!(AppConfig { owner_email: Some("owner@example.com".into()), ..exposed() }.open_first_signup_warning(true), None, "the owner email is set");
        assert_eq!(AppConfig { mode: Mode::Hosted, ..exposed() }.open_first_signup_warning(true), None, "hosted signup is open by design");
        let local = AppConfig { server: ServerConfig { host: "127.0.0.1".into(), ..exposed().server }, ..exposed() };
        assert_eq!(local.open_first_signup_warning(true), None, "loopback only");
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

    /// `webhooks.log_retention_days`: the days a finished webhook delivery
    /// and each attempt are kept. Unset it is 0, which keeps them for good,
    /// as every version did before the purge: an upgrade deletes nothing
    /// until the operator sets a number, at most 36500.
    #[test]
    fn webhooks_keep_their_log_for_good_unless_set() {
        assert_eq!(AppConfig::default().webhooks.log_retention_days, 0);
        assert_eq!(load_yaml("cost_markup_percent: 0\n").unwrap().webhooks.log_retention_days, 0);
        assert_eq!(load_yaml("webhooks:\n  internal_targets: admins\n").unwrap().webhooks.log_retention_days, 0);
        assert_eq!(load_yaml("webhooks:\n  log_retention_days: 30\n").unwrap().webhooks.log_retention_days, 30);
        assert_eq!(load_yaml("webhooks:\n  log_retention_days: 0\n").unwrap().webhooks.log_retention_days, 0);
        assert_eq!(load_yaml("webhooks:\n  log_retention_days: 36500\n").unwrap().webhooks.log_retention_days, 36_500);
        let e = load_yaml("webhooks:\n  log_retention_days: 36501\n").unwrap_err().to_string();
        assert!(e.contains("log_retention_days") && e.contains("36500"), "{e}");
    }

    /// `logs`: how long the request log is kept. Both 0 unless set, which
    /// keeps everything, as before; either may be set alone, each from 0 to
    /// 36500 days, in the file or as `LITEGEN__LOGS__…`.
    #[test]
    fn the_request_log_is_kept_for_good_unless_logs_set_a_retention() {
        let days = |c: &AppConfig| (c.logs.retention_days, c.logs.artifact_retention_days);
        assert_eq!(days(&AppConfig::default()), (0, 0));
        assert_eq!(days(&load_yaml("cost_markup_percent: 0\n").unwrap()), (0, 0));
        for (yaml, want) in [
            ("logs:\n  retention_days: 90\n  artifact_retention_days: 7\n", (90, 7)),
            ("logs:\n  retention_days: 30\n", (30, 0)),
            ("logs:\n  artifact_retention_days: 14\n", (0, 14)),
            ("logs:\n  retention_days: 0\n  artifact_retention_days: 0\n", (0, 0)),
            ("logs:\n  retention_days: 36500\n  artifact_retention_days: 36500\n", (36_500, 36_500)),
            ("logs:\n  artifact_retention_days: 36500\n", (0, 36_500)),
            ("logs:\n  retention_days: 7\n  artifact_retention_days: 7\n", (7, 7)),
        ] {
            assert_eq!(days(&load_yaml(yaml).unwrap_or_else(|e| panic!("{yaml}: {e}"))), want, "{yaml}");
        }
        let dir = tempfile::tempdir().unwrap();
        let env: HashMap<String, String> = [("LITEGEN__LOGS__RETENTION_DAYS", "30"), ("LITEGEN__LOGS__ARTIFACT_RETENTION_DAYS", "7")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let c = load_config_from(dir.path(), None, &env).unwrap_or_else(|e| panic!("LITEGEN__LOGS__*: {e}"));
        assert_eq!(days(&c), (30, 7));
    }

    /// `logs` is refused at load, naming the key: a value outside 0 to 36500,
    /// an artifact kept longer than its log row (both keys named), and a key
    /// it doesn't have. In the file and from the environment alike.
    #[test]
    fn logs_refuse_a_retention_out_of_range_or_an_artifact_outliving_its_log_row() {
        for (yaml, key) in [
            ("logs:\n  retention_days: 36501\n", "logs.retention_days"),
            ("logs:\n  retention_days: -1\n", "logs.retention_days"),
            ("logs:\n  artifact_retention_days: 36501\n", "logs.artifact_retention_days"),
            ("logs:\n  artifact_retention_days: -1\n", "logs.artifact_retention_days"),
        ] {
            let e = load_yaml(yaml).unwrap_err().to_string();
            assert!(e.contains(key) && e.contains("36500"), "{yaml}: {e}");
        }
        let e = load_yaml("logs:\n  retention_days: 7\n  artifact_retention_days: 30\n").unwrap_err().to_string();
        assert!(e.contains("logs.artifact_retention_days") && e.contains("logs.retention_days"), "{e}");
        let e = load_yaml("logs:\n  retention: 30\n").unwrap_err().to_string();
        assert!(e.contains("retention"), "{e}");

        let dir = tempfile::tempdir().unwrap();
        let env = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<String, String>>();
        let e = load_config_from(dir.path(), None, &env(&[("LITEGEN__LOGS__RETENTION_DAYS", "7"), ("LITEGEN__LOGS__ARTIFACT_RETENTION_DAYS", "30")]))
            .unwrap_err()
            .to_string();
        assert!(e.contains("logs.artifact_retention_days") && e.contains("logs.retention_days"), "{e}");
        let e = load_config_from(dir.path(), None, &env(&[("LITEGEN__LOGS__RETENTION_DAYS", "36501")])).unwrap_err().to_string();
        assert!(e.contains("logs.retention_days") && e.contains("36500"), "{e}");
    }

    /// `mesh.draco`: Draco on, at the limits every read had before they were
    /// settings (4,000,000 points and faces, 24 MB of decoded attributes),
    /// and 5 faces a point. Its options are the mesh library's own
    /// defaults, with this setting named for a refusal while it is off.
    #[test]
    fn mesh_draco_is_on_at_todays_limits_unless_set() {
        for c in [AppConfig::default(), load_yaml("cost_markup_percent: 0\n").unwrap(), load_yaml("mesh:\n  draco: {}\n").unwrap()] {
            assert_eq!(c.mesh.draco, DracoConfig { enabled: true, max_points: 4_000_000, max_faces: 4_000_000, max_decoded_mb: 24, max_faces_per_point: 5 });
            assert_eq!(c.mesh.draco.options(), crate::mesh::DracoOptions { disabled_by: DRACO_SWITCH, ..Default::default() });
        }
        assert!(DRACO_SWITCH.contains("mesh.draco.enabled") && DRACO_SWITCH.contains("LITEGEN__MESH__DRACO__ENABLED"));
    }

    /// Each `mesh.draco` setting loads from the file and from its
    /// `LITEGEN__MESH__DRACO__…` variable (a variable beats the file), both
    /// ends of each range load, and the options carry them to the mesh
    /// library, `max_decoded_mb` in MB of 1,000,000 bytes.
    #[test]
    fn mesh_draco_settings_load_from_the_file_and_the_environment() {
        let c = load_yaml("mesh:\n  draco:\n    enabled: false\n    max_points: 100000\n    max_faces: 200000\n    max_decoded_mb: 8\n    max_faces_per_point: 4\n").unwrap();
        assert_eq!(c.mesh.draco, DracoConfig { enabled: false, max_points: 100_000, max_faces: 200_000, max_decoded_mb: 8, max_faces_per_point: 4 });
        assert_eq!(
            c.mesh.draco.options(),
            crate::mesh::DracoOptions { enabled: false, disabled_by: DRACO_SWITCH, max_points: 100_000, max_faces: 200_000, max_decoded_bytes: 8_000_000, max_faces_per_point: 4 }
        );
        for (yaml, want) in [
            ("mesh:\n  draco:\n    max_points: 1\n    max_faces: 1\n    max_decoded_mb: 1\n    max_faces_per_point: 1\n", (1, 1, 1, 1)),
            ("mesh:\n  draco:\n    max_points: 4000000\n    max_faces: 4000000\n    max_decoded_mb: 192\n    max_faces_per_point: 64\n", (4_000_000, 4_000_000, 192, 64)),
        ] {
            let d = load_yaml(yaml).unwrap_or_else(|e| panic!("{yaml}: {e}")).mesh.draco;
            assert_eq!((d.max_points, d.max_faces, d.max_decoded_mb, d.max_faces_per_point), want, "{yaml}");
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("litegen.yaml"), "mesh:\n  draco:\n    max_points: 5\n").unwrap();
        let env: HashMap<String, String> = [
            ("LITEGEN__MESH__DRACO__ENABLED", "off"),
            ("LITEGEN__MESH__DRACO__MAX_POINTS", "1000"),
            ("LITEGEN__MESH__DRACO__MAX_FACES", "3000"),
            ("LITEGEN__MESH__DRACO__MAX_DECODED_MB", "12"),
            ("LITEGEN__MESH__DRACO__MAX_FACES_PER_POINT", "6"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let c = load_config_from(dir.path(), None, &env).unwrap_or_else(|e| panic!("LITEGEN__MESH__DRACO__*: {e}"));
        assert_eq!(c.mesh.draco, DracoConfig { enabled: false, max_points: 1000, max_faces: 3000, max_decoded_mb: 12, max_faces_per_point: 6 });
        let file_only = load_config_from(dir.path(), None, &HashMap::new()).unwrap_or_else(|e| panic!("the file alone: {e}"));
        assert_eq!(file_only.mesh.draco, DracoConfig { max_points: 5, ..DracoConfig::default() });
    }

    /// `mesh.draco` is refused at load, naming the setting and its range: a
    /// limit of 0, a negative one, one past its ceiling (the scene's caps for
    /// points and faces, 192 MB, 64 faces a point), and a key it doesn't
    /// have. From the file and the environment alike.
    #[test]
    fn mesh_draco_refuses_values_out_of_range_and_unknown_keys() {
        for (yaml, says) in [
            ("mesh:\n  draco:\n    max_points: 0\n", "mesh.draco.max_points must be between 1 and 4000000, not 0"),
            ("mesh:\n  draco:\n    max_points: -1\n", "mesh.draco.max_points must be between 1 and 4000000, not -1"),
            ("mesh:\n  draco:\n    max_points: 4000001\n", "mesh.draco.max_points must be between 1 and 4000000, not 4000001"),
            ("mesh:\n  draco:\n    max_faces: 0\n", "mesh.draco.max_faces must be between 1 and 4000000, not 0"),
            ("mesh:\n  draco:\n    max_faces: 4000001\n", "mesh.draco.max_faces must be between 1 and 4000000, not 4000001"),
            ("mesh:\n  draco:\n    max_decoded_mb: 0\n", "mesh.draco.max_decoded_mb must be between 1 and 192, not 0"),
            ("mesh:\n  draco:\n    max_decoded_mb: 193\n", "mesh.draco.max_decoded_mb must be between 1 and 192, not 193"),
            ("mesh:\n  draco:\n    max_faces_per_point: 0\n", "mesh.draco.max_faces_per_point must be between 1 and 64, not 0"),
            ("mesh:\n  draco:\n    max_faces_per_point: 65\n", "mesh.draco.max_faces_per_point must be between 1 and 64, not 65"),
        ] {
            let e = load_yaml(yaml).map(|c| c.mesh.draco).unwrap_err().to_string();
            assert!(e.contains(says), "{yaml}: {e}");
        }
        for (yaml, key) in [("mesh:\n  draco:\n    max_vertices: 10\n", "max_vertices"), ("mesh:\n  meshopt: {}\n", "meshopt")] {
            let e = load_yaml(yaml).map(|c| c.mesh).unwrap_err().to_string();
            assert!(e.contains(key), "{yaml}: {e}");
        }
        let dir = tempfile::tempdir().unwrap();
        let env = |k: &str, v: &str| [(k.to_string(), v.to_string())].into_iter().collect::<HashMap<String, String>>();
        let e = load_config_from(dir.path(), None, &env("LITEGEN__MESH__DRACO__MAX_FACES_PER_POINT", "0")).map(|c| c.mesh.draco).unwrap_err().to_string();
        assert!(e.contains("mesh.draco.max_faces_per_point must be between 1 and 64, not 0"), "{e}");
        let e = load_config_from(dir.path(), None, &env("LITEGEN__MESH__DRACO__ENABLED", "maybe")).map(|c| c.mesh.draco).unwrap_err().to_string();
        assert!(e.contains("enabled") || e.contains("maybe"), "{e}");
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
