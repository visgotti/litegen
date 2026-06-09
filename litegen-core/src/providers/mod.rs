use crate::capabilities::ModelSchema;
use crate::proxy::materializer::MaterializedRequest;
use crate::types::*;
use async_trait::async_trait;
use std::collections::HashMap;

pub mod auth;
pub mod image;
pub mod video;

pub use auth::{AuthSpec, ProviderCredentials};

// ─── W3C Trace Context Propagation ──────────────────────────────────────────

/// Inject W3C `traceparent` / `tracestate` headers from the current span into
/// a `reqwest::RequestBuilder`.  If there is no active span the builder is
/// returned unchanged.
///
/// In production, picks up the tracing span's OTel context via the
/// `tracing-opentelemetry` bridge.  Falls back to the raw OTel thread-local
/// context (used in tests that attach a context directly).
pub fn inject_trace_headers(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    // Resolve the active OTel context: first check the tracing span (for
    // in-flight request spans created by TraceLayer), then fall back to the
    // raw OTel thread-local (for tests and standalone callers).
    let ctx = {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        use opentelemetry::trace::TraceContextExt;
        let via_tracing = tracing::Span::current().context();
        if via_tracing.span().span_context().is_valid() {
            via_tracing
        } else {
            opentelemetry::Context::current()
        }
    };

    // Propagate into a plain HashMap, then attach each header.
    let mut carrier: HashMap<String, String> = HashMap::new();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&ctx, &mut carrier);
    });

    let mut builder = req;
    for (k, v) in carrier {
        builder = builder.header(k, v);
    }
    builder
}

#[cfg(test)]
mod trace_tests {
    use super::inject_trace_headers;

    /// Helper: parse headers from a built request without sending.
    fn get_header(req: reqwest::Request, name: &str) -> Option<String> {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }

    /// When a W3C propagator is installed and an OTel span context is active,
    /// `inject_trace_headers` adds a `traceparent` header.
    #[test]
    fn injects_traceparent_when_span_active() {
        use opentelemetry::trace::{TraceFlags, TraceId, SpanId, SpanContext, TraceContextExt};

        // Install a W3C propagator.
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );

        // Build a fake, sampled span context with known trace/span IDs.
        let trace_id = TraceId::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        let span_id  = SpanId::from_bytes([1, 2, 3, 4, 5, 6, 7, 8]);
        let span_ctx = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            false,
            Default::default(),
        );

        // Wrap in an OTel Context and attach it as the current context.
        let cx = opentelemetry::Context::current().with_remote_span_context(span_ctx);
        let _guard = cx.attach();

        let client = reqwest::Client::new();
        let builder = client.get("http://example.com");
        let builder = inject_trace_headers(builder);
        let built = builder.build().expect("build request");

        // The traceparent header must be present when there is an active span.
        let tp = get_header(built, "traceparent");
        assert!(
            tp.is_some(),
            "expected traceparent header to be present, headers were empty"
        );
        let tp_val = tp.unwrap();
        // W3C traceparent format: 00-<trace-id>-<span-id>-<flags>
        assert!(
            tp_val.starts_with("00-"),
            "traceparent should start with '00-', got: {tp_val}"
        );
    }

    /// When no span is active the builder is returned unchanged (no header injected).
    #[test]
    fn no_header_when_no_active_span() {
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );

        let client = reqwest::Client::new();
        let builder = client.get("http://example.com");
        let builder = inject_trace_headers(builder);
        let built = builder.build().expect("build request");

        // Without an active span the carrier should be empty — no traceparent.
        let tp = get_header(built, "traceparent");
        assert!(
            tp.is_none(),
            "expected no traceparent without active span, got: {:?}",
            tp
        );
    }
}

// ─── Provider Trait ─────────────────────────────────────────────────────────

/// Result from a successful generation.
#[derive(Debug, Clone)]
pub struct GenerationOutput {
    /// Raw image/video bytes.
    pub data: Vec<u8>,
    /// MIME content type (e.g. "image/png", "video/mp4").
    pub content_type: String,
    /// Provider-specific metadata (revised_prompt, model version, etc.).
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Configuration passed to a provider instance.
#[derive(Debug, Clone, Default)]
pub struct ProviderInstanceConfig {
    /// Primary API key.
    pub api_key: String,
    /// Additional API keys with weights.
    pub api_keys: Vec<crate::types::ApiKeyEntry>,
    /// Base URL override.
    pub api_base: Option<String>,
    /// Model mapping: internal ID → provider's model ID.
    pub model_mapping: HashMap<String, String>,
    /// Extra headers.
    pub extra_headers: HashMap<String, String>,
    /// Provider-specific options.
    pub options: Option<serde_json::Value>,
    /// Resolved auth credentials (api_key/keys mirrored here, plus key_id/
    /// key_secret/region for signing schemes). Built by `build_instance_config`.
    pub credentials: ProviderCredentials,
}

impl ProviderInstanceConfig {
    /// Clone this config but replace its credentials (and the derived `api_key`/
    /// `api_keys`) with `creds`. Used to build a per-request provider instance
    /// carrying a per-app BYO credential, preserving non-credential fields
    /// (api_base, model_mapping, extra_headers, options).
    pub fn with_credentials(&self, creds: ProviderCredentials) -> Self {
        let mut c = self.clone();
        c.api_key = creds.api_key.clone().unwrap_or_default();
        c.api_keys = creds.api_keys.clone();
        c.credentials = creds;
        c
    }
}

/// Health check result for a provider.
#[derive(Debug, Clone)]
pub struct HealthCheckResult {
    pub healthy: bool,
    pub message: String,
    pub latency_ms: Option<u64>,
}

/// Error from a provider.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("Provider not configured: {0}")]
    NotConfigured(String),

    #[error("Request failed: {message}")]
    RequestFailed {
        message: String,
        status_code: Option<u16>,
        provider_error: Option<serde_json::Value>,
        retryable: bool,
    },

    #[error("Rate limited by {provider}: retry after {retry_after_ms}ms")]
    RateLimited {
        provider: String,
        retry_after_ms: u64,
    },

    #[error("Request timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("Model not supported: {model} by provider {provider}")]
    ModelNotSupported { model: String, provider: String },

    #[error("Pricing unavailable for {provider}/{model}: {reason}")]
    PricingUnavailable {
        provider: String,
        model: String,
        reason: String,
    },

    #[error("Invalid request: {0}")]
    InvalidRequest(String),
}

impl ProviderError {
    /// Whether this error should trigger a fallback to the next deployment.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RequestFailed { retryable, .. } => *retryable,
            Self::RateLimited { .. } => true,
            Self::Timeout { .. } => true,
            Self::NotConfigured(_) => false,
            Self::ModelNotSupported { .. } => false,
            Self::PricingUnavailable { .. } => false,
            Self::InvalidRequest(_) => false,
        }
    }
}

/// Extra image-specific parameters extracted from a validated request.
#[derive(serde::Serialize)]
pub struct ImageExtras {
    pub size: Option<String>,
    pub aspect_ratio: Option<String>,
    pub quality: Option<String>,
    pub style: Option<String>,
    pub steps: Option<u32>,
    pub guidance_scale: Option<f64>,
    pub strength: Option<f64>,
    pub response_format: String,
    pub extra: Option<serde_json::Value>,
}

/// Extra video-specific parameters extracted from a validated request.
#[derive(serde::Serialize)]
pub struct VideoExtras {
    pub duration_seconds: f64,
    pub aspect_ratio: Option<String>,
    pub resolution: Option<String>,
    pub fps: Option<u32>,
    pub extra: Option<serde_json::Value>,
}

/// The core trait that every provider (OpenAI, Stability, Replicate, etc.) implements.
#[async_trait]
pub trait ImageProvider: Send + Sync {
    /// Unique provider name (e.g. "openai", "stability", "replicate").
    fn name(&self) -> &str;

    /// Configure the provider with credentials and settings.
    fn configure(&mut self, config: ProviderInstanceConfig);

    /// Whether the provider has been configured with valid credentials.
    fn is_configured(&self) -> bool;

    /// Generate an image from a normalized request.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError>;

    /// Estimate the cost of a generation request.
    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProviderError>;

    /// Lightweight health check (validate credentials, check connectivity).
    async fn health_check(&self) -> HealthCheckResult;

    /// Map an internal model ID to the provider's native model ID.
    fn map_model_id(&self, model: &str, mapping: &HashMap<String, String>) -> String {
        mapping.get(model).cloned().unwrap_or_else(|| model.to_string())
    }
}

/// The core trait for video generation providers.
#[async_trait]
pub trait VideoProvider: Send + Sync {
    fn name(&self) -> &str;
    fn configure(&mut self, config: ProviderInstanceConfig);
    fn is_configured(&self) -> bool;

    /// Start a video generation (may be async/polling-based).
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError>;

    /// Poll the status of an in-flight generation.
    async fn poll_status(&self, handle: &VideoGenerationHandle) -> Result<VideoGenerationPollResult, ProviderError>;

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        request: &VideoGenerationRequest,
    ) -> Result<CostEstimate, ProviderError>;

    async fn health_check(&self) -> HealthCheckResult;
}

/// Handle returned when a video generation is started (for polling).
#[derive(Debug, Clone)]
pub struct VideoGenerationHandle {
    pub provider_job_id: String,
    pub provider: String,
    pub model: String,
}

/// Status from polling a video generation.
#[derive(Debug, Clone)]
pub struct VideoGenerationPollResult {
    pub status: crate::types::GenerationStatus,
    pub progress: u8,
    pub video_url: Option<String>,
    pub video_data: Option<Vec<u8>>,
    pub content_type: Option<String>,
    pub error: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

// ─── Shared Helpers ─────────────────────────────────────────────────────────

/// Apply a configurable markup percentage to a base cost.
pub fn apply_markup(base_cost_usd: f64, markup_percent: f64) -> (f64, f64) {
    let markup = base_cost_usd * (markup_percent / 100.0);
    (markup, base_cost_usd + markup)
}

/// Convert USD to internal tokens. Default: 1 token = $0.001.
pub fn usd_to_tokens(usd: f64, rate: f64) -> u64 {
    (usd / rate).ceil() as u64
}

/// Build a CostEstimate from a base USD cost.
pub fn build_cost_estimate(
    base_cost_usd: f64,
    markup_percent: f64,
    cost_source: CostSource,
    breakdown: Option<serde_json::Value>,
) -> CostEstimate {
    let (markup_usd, total_cost_usd) = apply_markup(base_cost_usd, markup_percent);
    CostEstimate {
        base_cost_usd,
        markup_usd,
        total_cost_usd,
        tokens_required: usd_to_tokens(total_cost_usd, 0.001),
        cost_source,
        breakdown,
    }
}

// ─── Weighted round-robin pools ─────────────────────────────────────────────

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Process-global rotation cursors keyed by a fingerprint of a pool's entries.
///
/// The registry rebuilds a fresh provider instance — and therefore a fresh pool
/// — on every per-app BYO request. A cursor owned by the instance would reset to
/// 0 each time, so a weighted pool would always return its first key. Sharing
/// the cursor process-wide (by credential fingerprint) lets rotation persist
/// across those rebuilds. The credential *values* are still rebuilt per request,
/// so editing a credential takes effect immediately; only the rotation index is
/// shared. Stale entries for deleted credentials are harmless (just integers).
static POOL_CURSORS: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<u64, Arc<AtomicUsize>>>> =
    std::sync::OnceLock::new();

fn shared_cursor(fingerprint: u64) -> Arc<AtomicUsize> {
    let map = POOL_CURSORS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()));
    if let Some(c) = map.read().unwrap().get(&fingerprint) {
        return Arc::clone(c);
    }
    Arc::clone(
        map.write()
            .unwrap()
            .entry(fingerprint)
            .or_insert_with(|| Arc::new(AtomicUsize::new(0))),
    )
}

fn hash_with<F: FnOnce(&mut std::collections::hash_map::DefaultHasher)>(tag: &str, f: F) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tag.hash(&mut h);
    f(&mut h);
    h.finish()
}

/// Weighted round-robin API key pool for distributing requests across
/// multiple keys to avoid per-key rate limits.
pub struct ApiKeyPool {
    entries: Vec<crate::types::ApiKeyEntry>,
    schedule: Vec<usize>,
    cursor: Arc<AtomicUsize>,
}

impl ApiKeyPool {
    /// Pool with a private cursor that starts at 0 — deterministic rotation,
    /// used for direct/unit construction.
    pub fn new(entries: Vec<crate::types::ApiKeyEntry>) -> Self {
        Self::build(entries, Arc::new(AtomicUsize::new(0)))
    }

    /// Pool whose rotation cursor is shared process-wide by credential
    /// fingerprint, so rotation survives the per-request instance rebuilds the
    /// registry performs for BYO credentials. Use this on the request path.
    pub fn shared(entries: Vec<crate::types::ApiKeyEntry>) -> Self {
        use std::hash::Hash;
        let fp = hash_with("api_keys", |h| {
            for e in &entries {
                e.key.hash(h);
                e.weight.hash(h);
            }
        });
        let cursor = shared_cursor(fp);
        Self::build(entries, cursor)
    }

    fn build(entries: Vec<crate::types::ApiKeyEntry>, cursor: Arc<AtomicUsize>) -> Self {
        assert!(!entries.is_empty(), "ApiKeyPool requires at least one key");
        let schedule = build_weighted_schedule(entries.iter().map(|e| e.weight));
        Self { entries, schedule, cursor }
    }

    /// Get the next API key using weighted round-robin.
    /// Thread-safe via atomic increment.
    pub fn next(&self) -> &str {
        let pos = self.cursor.fetch_add(1, Ordering::Relaxed);
        let idx = self.schedule[pos % self.schedule.len()];
        &self.entries[idx].key
    }

    pub fn size(&self) -> usize {
        self.entries.len()
    }
}

/// Build a weighted round-robin schedule: index `i` appears `weights[i].max(1)`
/// times (so a weight of 0 still gets one slot — a key is never silently
/// dropped). Shared by [`ApiKeyPool`] and [`CredentialPool`].
fn build_weighted_schedule(weights: impl Iterator<Item = u32>) -> Vec<usize> {
    let mut schedule = Vec::new();
    for (i, w) in weights.enumerate() {
        for _ in 0..w.max(1) {
            schedule.push(i);
        }
    }
    schedule
}

/// Weighted round-robin pool over signing credential *sets* (the SigV4/TC3/JWT
/// analogue of [`ApiKeyPool`]). Distributes requests across multiple
/// `key_id`/`key_secret`(/`region`) accounts to spread per-account limits.
pub struct CredentialPool {
    entries: Vec<crate::types::CredentialEntry>,
    schedule: Vec<usize>,
    cursor: Arc<AtomicUsize>,
}

impl CredentialPool {
    /// Pool with a private cursor that starts at 0 (deterministic).
    pub fn new(entries: Vec<crate::types::CredentialEntry>) -> Self {
        Self::build(entries, Arc::new(AtomicUsize::new(0)))
    }

    /// Pool whose rotation cursor is shared process-wide by credential
    /// fingerprint — see [`ApiKeyPool::shared`].
    pub fn shared(entries: Vec<crate::types::CredentialEntry>) -> Self {
        use std::hash::Hash;
        let fp = hash_with("credential_sets", |h| {
            for e in &entries {
                e.key_id.hash(h);
                e.key_secret.hash(h);
                e.region.hash(h);
                e.weight.hash(h);
            }
        });
        let cursor = shared_cursor(fp);
        Self::build(entries, cursor)
    }

    fn build(entries: Vec<crate::types::CredentialEntry>, cursor: Arc<AtomicUsize>) -> Self {
        assert!(!entries.is_empty(), "CredentialPool requires at least one credential");
        let schedule = build_weighted_schedule(entries.iter().map(|e| e.weight));
        Self { entries, schedule, cursor }
    }

    /// Get the next credential set using weighted round-robin. Thread-safe.
    pub fn next(&self) -> &crate::types::CredentialEntry {
        let pos = self.cursor.fetch_add(1, Ordering::Relaxed);
        let idx = self.schedule[pos % self.schedule.len()];
        &self.entries[idx]
    }

    pub fn size(&self) -> usize {
        self.entries.len()
    }
}

/// Parse an env-style key string: "key1:weight1,key2:weight2,key3".
pub fn parse_api_keys(raw: &str) -> Vec<crate::types::ApiKeyEntry> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    raw.split(',')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .map(|part| {
            if let Some(colon_pos) = part.rfind(':') {
                let maybe_weight = &part[colon_pos + 1..];
                if let Ok(w) = maybe_weight.parse::<u32>() {
                    if w > 0 {
                        return crate::types::ApiKeyEntry {
                            key: part[..colon_pos].to_string(),
                            weight: w,
                            label: None,
                        };
                    }
                }
            }
            crate::types::ApiKeyEntry {
                key: part.to_string(),
                weight: 1,
                label: None,
            }
        })
        .collect()
}
