use crate::capabilities::ModelSchema;
use crate::proxy::materializer::MaterializedRequest;
use crate::types::*;
use async_trait::async_trait;
use std::collections::HashMap;

pub mod auth;
pub mod image;
pub mod model3d;
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
mod error_class_tests {
    use super::ProviderError;

    fn failed(status: Option<u16>, retryable: bool) -> ProviderError {
        ProviderError::RequestFailed { message: "x".into(), status_code: status, provider_error: None, retryable }
    }

    fn refusals() -> Vec<ProviderError> {
        vec![
            ProviderError::InvalidRequest("x".into()),
            ProviderError::ModelNotSupported { model: "m".into(), provider: "p".into() },
            ProviderError::PricingUnavailable { provider: "p".into(), model: "m".into(), reason: "r".into() },
            ProviderError::NotConfigured("p".into()),
        ]
    }

    /// E9: an adapter's verdict before any call is told apart from anything
    /// the vendor answered.
    #[test]
    fn a_refusal_before_any_call_is_told_apart_from_a_vendors_answer() {
        for e in refusals() {
            assert!(e.before_any_call(), "{e}");
        }
        let rate_limited = ProviderError::RateLimited { provider: "p".into(), retry_after_ms: 1, message: "m".into(), provider_error: None };
        for e in [failed(Some(400), false), failed(Some(500), true), failed(None, true), ProviderError::Timeout { timeout_ms: 1 }, rate_limited] {
            assert!(!e.before_any_call(), "{e}");
        }
    }

    /// E9: what counts toward a deployment's circuit breaker: the deployment
    /// being unwell, never the caller's own request being refused, and a
    /// 401 or 403 only when the platform's own credential got it (a 429 and
    /// a 402 too, since 2026-11-12: the next test).
    #[test]
    fn only_an_unwell_deployment_counts_toward_its_breaker() {
        for e in [failed(Some(500), true), failed(Some(503), true), failed(Some(408), true), failed(None, true), ProviderError::Timeout { timeout_ms: 1 }] {
            assert!(e.says_the_deployment_is_unhealthy(true) && e.says_the_deployment_is_unhealthy(false), "{e}");
        }
        for s in [401, 403] {
            assert!(failed(Some(s), false).says_the_deployment_is_unhealthy(true), "{s} to the platform's key");
            assert!(!failed(Some(s), false).says_the_deployment_is_unhealthy(false), "{s} to a tenant's own key");
        }
        let mut refused = vec![failed(Some(400), false), failed(Some(404), false), failed(Some(422), false), failed(None, false)];
        refused.extend(refusals());
        for e in refused {
            assert!(!e.says_the_deployment_is_unhealthy(true), "{e}");
        }
    }

    /// Ruled 2026-11-12 (handlers review should-fix 1): a 429 (or
    /// `RateLimited`) and a 402 say the deployment is unwell only to the
    /// platform's own credential, as a 401 or 403 does since E9. A tenant out
    /// of rate limit or credit on its own key says nothing about the provider
    /// for every other tenant. A timeout or a 5xx still counts for both.
    #[test]
    fn a_tenants_rate_limit_or_unpaid_key_does_not_count_toward_the_breaker() {
        let rate_limited = ProviderError::RateLimited { provider: "p".into(), retry_after_ms: 1, message: "m".into(), provider_error: None };
        for e in [failed(Some(429), true), rate_limited, failed(Some(402), false), failed(Some(401), false), failed(Some(403), false)] {
            assert!(e.says_the_deployment_is_unhealthy(true), "{e} to the platform's key");
            assert!(!e.says_the_deployment_is_unhealthy(false), "{e} to a tenant's own key");
        }
        for e in [failed(Some(500), true), failed(Some(503), true), failed(Some(408), true), failed(None, true), ProviderError::Timeout { timeout_ms: 1 }] {
            assert!(e.says_the_deployment_is_unhealthy(true) && e.says_the_deployment_is_unhealthy(false), "{e}");
        }
    }
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
    /// Provider-specific metadata (revised_prompt, model version, etc.). A
    /// multiview image carries its `view` and `yaw_deg` here.
    pub metadata: HashMap<String, serde_json::Value>,
    /// The call's other images, in order, when one call makes several (a
    /// multiview model's views). Each is returned as its own result.
    pub more: Vec<GenerationOutput>,
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

impl ProviderInstanceConfig {
    /// The id the vendor gets for catalog model `model_id`, when the operator
    /// mapped it (`model_mapping`): keyed by the catalog id
    /// (`bfl/flux-pro-1.1`), else by the id without its provider prefix
    /// (`flux-pro-1.1`). The catalog row still decides params, price and
    /// request shape; only the string the vendor reads changes.
    pub fn mapped_model(&self, model_id: &str) -> Option<&str> {
        self.model_mapping
            .get(model_id)
            .or_else(|| model_id.split_once('/').and_then(|(_, bare)| self.model_mapping.get(bare)))
            .map(String::as_str)
            .filter(|m| !m.trim().is_empty())
    }
}

/// The model id the vendor gets for catalog model `model_id`: the operator's
/// mapping (`model_mapping`, see [`ProviderInstanceConfig::mapped_model`]),
/// else the adapter's own `native` id. Behavior (endpoint, params) stays the
/// row's; only the string on the wire changes.
pub(crate) fn wire_model<'a>(config: Option<&'a ProviderInstanceConfig>, model_id: &str, native: &'a str) -> &'a str {
    config.and_then(|c| c.mapped_model(model_id)).unwrap_or(native)
}

/// The fields of a request's `extra` an adapter may send: every key of the
/// object except those `owned` names (fields the adapter sets itself from
/// canonical params, so `extra` cannot replace them: rule 11) and those
/// litegen keeps for itself ([`crate::capabilities::gateway_owned_extra`]:
/// where the vendor sends or stores a result, and whether it publishes it).
/// Every adapter that merges `extra` goes through here (E4a), so neither a
/// row, a runtime model nor an adapter's own list can hand a vendor callback
/// to a caller. The validator has already dropped every key the row does not
/// allowlist.
pub fn extra_fields<F: Fn(&str) -> bool>(
    extra: Option<&serde_json::Value>,
    owned: F,
) -> impl Iterator<Item = (&String, &serde_json::Value)> {
    extra
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .filter(move |(k, _)| !owned(k) && crate::capabilities::gateway_owned_extra(k).is_none())
}

#[cfg(test)]
mod extra_fields_tests {
    use serde_json::json;

    /// E4a: the adapter's own fields and litegen's are never sent; any
    /// other key is, with its value as given.
    #[test]
    fn extra_sends_neither_the_adapters_fields_nor_litegens() {
        let extra = json!({ "model": "dearer", "callback_url": "https://x", "isPublic": true, "storageUri": "gs://b", "watermark": false });
        let sent: Vec<(&String, &serde_json::Value)> = super::extra_fields(Some(&extra), |k| k == "model").collect();
        assert_eq!(sent, [(&"watermark".to_string(), &json!(false))]);
        assert_eq!(super::extra_fields(Some(&json!("not an object")), |_| false).count(), 0);
        assert_eq!(super::extra_fields(None, |_| false).count(), 0);
    }
}

/// Vendors (or single rows) whose API takes no model id: litegen picks the
/// endpoint (or action) per row, so nothing on the wire names a model that
/// `model_mapping` could replace.
pub const NO_MODEL_ID: &[(&str, &str)] = &[
    ("ideogram", "the endpoint (/v1/ideogram-v3/generate, /v1/ideogram-v4/generate) and rendering_speed pick the model"),
    ("pixellab", "one endpoint per model (/create-image-pixflux)"),
    ("hunyuan", "Tencent Cloud's X-TC-Action names the job type (SubmitHunyuanImageJob, SubmitHunyuanToVideoJob…)"),
    ("recraft/remove-background", "POST /images/removeBackground takes only the image"),
];

/// Headers a default on every request would break: the framing and
/// hop-by-hop headers reqwest and the vendor's server work out per request.
const RESERVED_EXTRA_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
    "te",
    "trailer",
    "proxy-authorization",
    "proxy-connection",
];

/// Why one of the operator's `extra_headers` can't be sent, if it can't.
fn extra_header_problem(name: &str, value: &str) -> Option<String> {
    if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
        return Some(format!("extra header '{name}' is not a valid HTTP header name"));
    }
    if RESERVED_EXTRA_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
        return Some(format!("extra header '{name}' can't be set: it is worked out per request"));
    }
    if reqwest::header::HeaderValue::from_str(value).is_err() {
        return Some(format!("extra header '{name}' has a value that is not valid HTTP"));
    }
    None
}

/// The operator's `extra_headers`, refused when any can't be sent: not
/// valid HTTP, or a framing or hop-by-hop header. Startup and the admin
/// API both use it, so a bad header is reported, never sent.
pub fn check_extra_headers(headers: &HashMap<String, String>) -> Result<(), String> {
    let mut problems: Vec<String> = headers.iter().filter_map(|(k, v)| extra_header_problem(k, v)).collect();
    problems.sort();
    if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
}

/// The client for a vendor's API: a total `timeout`, and the operator's
/// `extra_headers` on every request it makes. A header a request sets itself
/// (auth, `content-type`) wins over one of the same name, so these add to a
/// request and never replace its auth. Result downloads use a separate
/// client and never carry them. A header that can't be sent is skipped with
/// a warning; startup and the admin API refuse it before it gets here.
///
/// It is the process's one client for this timeout and header set
/// ([`crate::util::http_clients`], P8): every adapter configured alike (a
/// global instance, one built for a single call on an org's own key, the
/// poller's for one row) sends through one connection pool, and a skipped
/// header is warned about once, when the client is built.
pub fn api_client(timeout: std::time::Duration, extra_headers: &HashMap<String, String>) -> reqwest::Client {
    let key = crate::util::http_clients::ClientKey::Api { timeout, headers: headers_digest(extra_headers) };
    crate::util::http_clients::shared(key, || {
        let mut defaults = reqwest::header::HeaderMap::new();
        for (name, value) in extra_headers {
            if let Some(problem) = extra_header_problem(name, value) {
                tracing::warn!(header = %name, "{problem}; not sent");
                continue;
            }
            if let (Ok(n), Ok(v)) = (
                reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                reqwest::header::HeaderValue::from_str(value),
            ) {
                defaults.insert(n, v);
            }
        }
        reqwest::Client::builder().timeout(timeout).default_headers(defaults).build().expect("failed to build HTTP client")
    })
}

/// A digest of `headers`' names and values, in name order, each one
/// length-prefixed so no two different sets digest alike: the key of
/// [`api_client`]'s shared client, holding no copy of a header secret.
fn headers_digest(headers: &HashMap<String, String>) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut pairs: Vec<(&String, &String)> = headers.iter().collect();
    pairs.sort();
    let mut hasher = Sha256::new();
    for (name, value) in pairs {
        for part in [name.as_bytes(), value.as_bytes()] {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part);
        }
    }
    hasher.finalize().into()
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

    /// A vendor's 429 that said when to come back (`Retry-After`): litegen
    /// waits at least that long before trying again, or doesn't retry when it
    /// is longer than the longest backoff (`docs/gateway-settings.md`).
    #[error("{message}")]
    RateLimited {
        provider: String,
        retry_after_ms: u64,
        /// The vendor's own words, as a `RequestFailed`'s.
        message: String,
        provider_error: Option<serde_json::Value>,
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

    /// Whether this is the adapter's verdict before anything was sent to the
    /// vendor (E9): a request it can't send (`InvalidRequest`), a model it
    /// doesn't serve, a price it can't work out, or no credential it can use.
    /// The caller's (or the operator's) to fix; nothing was billed.
    pub fn before_any_call(&self) -> bool {
        matches!(
            self,
            Self::InvalidRequest(_) | Self::ModelNotSupported { .. } | Self::PricingUnavailable { .. } | Self::NotConfigured(_)
        )
    }

    /// Whether this failure says the deployment itself is unwell, so it
    /// counts toward its circuit breaker (E9): no answer, a timeout, a 5xx or
    /// a 408, and a 401, 402, 403 or 429 (`RateLimited` too) to the
    /// platform's own credential (`platform_credential`). Never a refusal of
    /// one request (another 4xx, a verdict such as moderation, the adapter's
    /// own refusal before any call), and never a key, credit or rate-limit
    /// refusal of a tenant's own key (ruled 2026-11-12 for 402 and 429): one
    /// tenant's requests must not cut a provider off for every tenant.
    pub fn says_the_deployment_is_unhealthy(&self, platform_credential: bool) -> bool {
        match self {
            Self::Timeout { .. } => true,
            Self::RateLimited { .. } => platform_credential,
            Self::RequestFailed { status_code: Some(s), .. } => {
                *s >= 500 || *s == 408 || (platform_credential && matches!(s, 401 | 402 | 403 | 429))
            }
            Self::RequestFailed { status_code: None, retryable, .. } => *retryable,
            Self::NotConfigured(_) | Self::ModelNotSupported { .. } | Self::PricingUnavailable { .. } | Self::InvalidRequest(_) => false,
        }
    }
}

/// Where an adapter that submits a job and then waits for it records the
/// vendor's job id, the moment the vendor accepts it.
///
/// The router reads it after a failure. A job the vendor accepted is never
/// sent again, since the vendor may bill it, and a timeout reports its id.
/// Adapters whose call is a single request-response leave it empty.
#[derive(Debug, Clone, Default)]
pub struct VendorJobSink(Arc<std::sync::Mutex<Option<RecordedJob>>>);

/// A vendor job's id, and the state a late look resumes it from.
type RecordedJob = (String, Option<serde_json::Value>);

impl VendorJobSink {
    /// The vendor accepted job `id`.
    pub fn record(&self, id: impl Into<String>) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some((id.into(), None));
    }

    /// The vendor accepted job `id`, which [`ImageProvider::poll_job`] can
    /// ask about again with `resume` (BFL's regional polling URL).
    pub fn record_resumable(&self, id: impl Into<String>, resume: serde_json::Value) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some((id.into(), Some(resume)));
    }

    /// The job id recorded since the last [`clear`](Self::clear), if any.
    pub fn get(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|(id, _)| id.clone())
    }

    /// What [`record_resumable`](Self::record_resumable) kept, if anything.
    pub fn resume(&self) -> Option<serde_json::Value> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|(_, r)| r.clone())
    }

    /// Forget the recorded id, before a fresh attempt.
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// One look at an image job the vendor accepted (late delivery,
/// docs/superpowers/specs/2026-11-03-delivery-and-billing-design.md).
#[derive(Debug)]
pub enum ImageJobPoll {
    /// Still queued or running.
    Running,
    /// Finished: the image, built as `generate` would have returned it.
    Done(GenerationOutput),
    /// The vendor's verdict: it failed, with its reason.
    Failed(String),
}

tokio::task_local! {
    /// When litegen stops waiting for the vendor call running in this task.
    static CALL_DEADLINE: std::time::Instant;
    /// How long the caller waits for an image job before litegen answers that
    /// it is still running (`Prefer: wait`, `respond-async`).
    static CALL_WAIT: std::time::Duration;
}

/// Run `call` for a caller that waits at most `wait` for an image job: the
/// router's deadline is the sooner of this and the vendor timeout, and a job
/// still running then is delivered later (late delivery).
pub async fn with_wait<F: std::future::Future>(wait: std::time::Duration, call: F) -> F::Output {
    CALL_WAIT.scope(wait, call).await
}

/// The wait [`with_wait`] set for this task, if any.
pub fn call_wait() -> Option<std::time::Duration> {
    CALL_WAIT.try_with(|w| *w).ok()
}

/// Run `call` knowing litegen stops waiting for it at `deadline`: an adapter
/// that polls for its result keeps asking until then ([`PollBudget`]), so a
/// longer vendor timeout (`docs/gateway-settings.md`) reaches a slow job.
pub async fn with_deadline<F: std::future::Future>(deadline: std::time::Instant, call: F) -> F::Output {
    CALL_DEADLINE.scope(deadline, call).await
}

/// How long an adapter that polls for its result goes on: until litegen
/// stops waiting for the call ([`with_deadline`]), or for `default` when it
/// is called without one (directly, in tests).
pub struct PollBudget {
    until: std::time::Instant,
    interval: std::time::Duration,
}

impl PollBudget {
    pub fn new(default: std::time::Duration, interval: std::time::Duration) -> Self {
        let until = CALL_DEADLINE.try_with(|d| *d).unwrap_or_else(|_| std::time::Instant::now() + default);
        Self { until, interval }
    }

    /// Wait one interval, never past the deadline. False once out of time,
    /// at once when the deadline has already passed.
    pub async fn next(&mut self) -> bool {
        let now = std::time::Instant::now();
        if now >= self.until {
            return false;
        }
        tokio::time::sleep(self.interval.min(self.until - now)).await;
        true
    }

    /// The whole budget, for an error that reports it.
    pub fn total_ms(&self, started: std::time::Instant) -> u64 {
        self.until.saturating_duration_since(started).as_millis() as u64
    }
}

/// Extra image-specific parameters extracted from a validated request.
#[derive(serde::Serialize)]
pub struct ImageExtras {
    /// Pixels to add a side (3D doc §4), checked against the model's spec.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expand: Option<crate::types::Expand>,
    /// Layout boxes (3D doc §4), checked against the model's spec.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<Vec<crate::types::LayoutItem>>,
    /// The area an edit replaces, named in text (3D doc §4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_prompt: Option<String>,
    /// Paste the init back outside the edit area (3D doc §4); part of the
    /// cache key, so a preserved answer is never served for an unpreserved
    /// request or the other way round.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserve_unmasked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feather_px: Option<u32>,
    /// Colours as `#rrggbb` (PixelGenie L16), checked against the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub palette: Option<Vec<String>>,
    /// 4 or 8 facings, one image each (PixelGenie L16).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directions: Option<u32>,
    pub size: Option<String>,
    pub aspect_ratio: Option<String>,
    /// The output tier on litegen's scale (`1K`, `2K`, …), checked against the
    /// model's `resolution` values; each adapter maps it to its vendor's name.
    pub resolution: Option<String>,
    pub quality: Option<String>,
    pub style: Option<String>,
    pub steps: Option<u32>,
    pub guidance_scale: Option<f64>,
    pub strength: Option<f64>,
    pub response_format: String,
    pub extra: Option<serde_json::Value>,
    /// The views a `multiview` model makes, in order — the request's, or the
    /// model's default set (3D doc §3E). Empty for other models.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<String>,
    /// The container to ask the vendor for (`png` | `webp` | `jpeg`), when
    /// the request named one (PixelGenie L9).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    /// `transparent` | `opaque`, when the request named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// A partial edit (3D doc §4) after litegen checked and converted it: the
    /// mask itself travels as the `mask` reference, in the vendor's
    /// convention.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<ImageEdit>,
    /// Filled by adapters that poll: see [`VendorJobSink`].
    #[serde(skip)]
    pub vendor_job: VendorJobSink,
}

/// What a vendor needs of a partial edit beyond its mask.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ImageEdit {
    /// The box, for a model that takes a region as its own box: the region,
    /// or the vendor's minimum window around it.
    pub window: Option<crate::types::Region>,
    /// The growth for a vendor that grows masks itself — sent explicitly,
    /// even 0, because its default is not 0.
    pub vendor_grow_px: Option<u32>,
    /// What `preserve_unmasked` pastes back (3D doc §4); never serialised.
    #[serde(skip)]
    pub preserve: Option<crate::proxy::preserve::Preserve>,
}

/// A video operation's source clip (3D doc §5).
#[derive(Debug, Clone)]
pub struct VideoSource {
    pub bytes: bytes::Bytes,
    pub content_type: String,
    /// Read from the bytes.
    pub duration_s: f64,
    pub fps: Option<f64>,
    /// The vendor's own job id, when the source is one of its generations on
    /// the same account (Luma and Kling chain only those).
    pub vendor_job_id: Option<String>,
}

/// Extra video-specific parameters extracted from a validated request.
#[derive(serde::Serialize)]
pub struct VideoExtras {
    /// The clip an operation works on, read and measured before billing
    /// (3D doc §5); never serialised.
    #[serde(skip)]
    pub source: Option<VideoSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_range: Option<crate::types::TimeRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace: Option<String>,
    pub duration_seconds: f64,
    pub aspect_ratio: Option<String>,
    pub resolution: Option<String>,
    pub fps: Option<u32>,
    pub extra: Option<serde_json::Value>,
    /// `fixed` | `auto`, when the request named one (PixelGenie L10).
    pub camera: Option<String>,
    /// `false` for a silent clip, `true` for audio, when named.
    pub audio: Option<bool>,
    /// `true` for a clip that ends on its first frame, when named.
    #[serde(rename = "loop")]
    pub r#loop: Option<bool>,
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

    /// Whether `model`'s jobs can be asked about again after the call that
    /// submitted them returned ([`poll_job`](Self::poll_job)): true for an
    /// adapter whose vendor runs jobs and records them in `extras.vendor_job`.
    fn resumable(&self, _model: &ModelSchema) -> bool {
        false
    }

    /// One look at job `job` of catalog model `model` (and the `resume` state
    /// it was recorded with):
    /// running, done (the image `generate` would have returned), or failed.
    /// A transient failure is a retryable `Err`.
    async fn poll_job(&self, model: &str, job: &str, resume: Option<&serde_json::Value>) -> Result<ImageJobPoll, ProviderError> {
        let _ = (job, resume);
        Err(ProviderError::InvalidRequest(format!("'{model}' jobs can't be resumed")))
    }

    /// Promptable segmentation (PixelGenie L7): the vendor's masks for what
    /// the prompts select, best first. Core makes each a black-and-white PNG
    /// the image's size and measures its box, so return them as they came.
    async fn segment(&self, model: &ModelSchema, input: &SegmentationInput) -> Result<Vec<VendorMask>, ProviderError> {
        let _ = input;
        Err(ProviderError::ModelNotSupported { model: model.id.clone(), provider: self.name().to_string() })
    }
}

/// A segmentation request, checked against the model and the image.
#[derive(Debug, Clone)]
pub struct SegmentationInput {
    /// The image as sent, and its type read from the bytes.
    pub image: bytes::Bytes,
    pub content_type: String,
    pub width: u32,
    pub height: u32,
    pub prompts: Vec<crate::types::SegmentationPrompt>,
    /// The most masks to return.
    pub max_masks: usize,
}

/// One mask as the vendor returned it.
#[derive(Debug, Clone)]
pub struct VendorMask {
    /// Any image container the `image` crate reads; white or opaque means selected.
    pub bytes: Vec<u8>,
    pub score: Option<f64>,
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

    /// [`poll_status`](Self::poll_status), asking `gate` once the vendor says
    /// the video is ready and before downloading it. On [`Fetch::StandAside`]
    /// it downloads nothing and reports the job still processing. An adapter
    /// that hands over the video's bytes (it downloads them in its poll)
    /// overrides this; the default never asks, and the caller claims after
    /// the poll, before it downloads the video from its link.
    async fn poll_status_gated(&self, handle: &VideoGenerationHandle, gate: &dyn FetchGate) -> Result<VideoGenerationPollResult, ProviderError> {
        let _ = gate;
        self.poll_status(handle).await
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        request: &VideoGenerationRequest,
    ) -> Result<CostEstimate, ProviderError>;

    async fn health_check(&self) -> HealthCheckResult;

    /// Whether `model`'s finished video comes with a public link litegen can
    /// hand out when the app keeps nothing (docs/storage-access.md). False
    /// for a vendor that returns bytes, or a link only it can read: such a
    /// request is refused before the vendor runs.
    fn hands_out_links(&self, _model: &ModelSchema) -> bool {
        true
    }

    /// Ask the vendor to stop a job the caller cancelled (L18). The row is
    /// already `cancelled`; this decides whether the VENDOR keeps working (and
    /// billing), and the answer is what `vendor_cancelled` reports. Vendors
    /// without a cancel endpoint answer `Unsupported`.
    async fn cancel(&self, _handle: &VideoGenerationHandle) -> Result<CancelOutcome, ProviderError> {
        Ok(CancelOutcome::Unsupported)
    }
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

/// The canonical 3D params of a validated request, as every adapter receives
/// them. What each one becomes on each vendor's wire is tabled in
/// `providers::model3d` (the module docs) and proven per adapter by tests that
/// assert the outbound field.
///
/// Every field has already passed the model's own spec: a param the model does
/// not declare arrives as `None` (dropped in lax mode, refused in strict), so
/// an adapter maps what it is given and never re-validates.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Model3dExtras {
    /// Containers this generation is committed to producing. Never empty —
    /// resolved through `ModelSchema::resolve_output_formats`, so an adapter can
    /// map it straight onto the vendor's field without re-deriving a default.
    /// Every entry must come back as a `Mesh` file or the generation fails;
    /// litegen derives the ones the vendor omits (whatever
    /// `proxy::model3d_convert::derivable_formats` covers), so an adapter asks
    /// the vendor for ONE container — see `prefers_vendor_native` there.
    pub output_formats: Vec<String>,
    /// Texture the mesh rather than returning bare geometry.
    pub texture: Option<bool>,
    /// PBR materials; `false` means flat shading, not "no material".
    pub pbr: Option<bool>,
    /// Approximate triangle budget.
    pub target_polycount: Option<u32>,
    /// `off` | `auto` | `on` (`types::Model3dSymmetry`).
    pub symmetry: Option<String>,
    /// `triangle` | `quad` (`types::Model3dTopology`).
    pub topology: Option<String>,
    /// Deliver the mesh with a skeleton.
    pub rig: Option<bool>,
    /// The skeleton to rig for (`types::Model3dSkeleton`).
    pub skeleton: Option<String>,
    /// The joint names to deliver (`types::Model3dJointNaming`); `None` keeps
    /// the vendor's own.
    pub joint_naming: Option<String>,
    /// The character's height in metres.
    pub height_m: Option<f64>,
    /// A `texture` model's map size (3D doc §3F).
    pub texture_resolution: Option<u32>,
    /// Paint the source's own UV layout.
    pub keep_uvs: Option<bool>,
    /// Opt-in placement (3D doc §3D), applied by litegen on delivery.
    pub ground: Option<bool>,
    pub center: Option<bool>,
    /// The clips to make, in order (3D doc §3C), presets resolved to the
    /// model's own ids.
    pub animations: Vec<AnimationClip>,
    /// Frames per second of the delivered clips.
    pub fps: Option<u32>,
    /// How FBX material and light colours are stored
    /// (`types::Model3dFbxColorSpace`), applied by litegen on delivery.
    pub fbx_color_space: Option<crate::types::Model3dFbxColorSpace>,
    /// The units FBX light strengths are stored in
    /// (`types::Model3dLightUnits`), applied by litegen on delivery.
    pub fbx_light_units: Option<crate::types::Model3dLightUnits>,
    /// The units USD light strengths are stored in
    /// (`types::Model3dLightUnits`), applied by litegen on delivery.
    pub usd_light_units: Option<crate::types::Model3dLightUnits>,
    /// How a Collada file stores a mesh's morph targets
    /// (`types::Model3dDaeMorphs`), applied by litegen on delivery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dae_morphs: Option<crate::types::Model3dDaeMorphs>,
    /// What the root layer of a usdz litegen writes is
    /// (`types::Model3dUsdzRoot`), applied by litegen on delivery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usdz_root: Option<crate::types::Model3dUsdzRoot>,
    /// Which clips a USD file litegen writes holds (`types::Model3dUsdClips`),
    /// applied by litegen on delivery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd_clips: Option<crate::types::Model3dUsdClips>,
    /// Where a promised fbx or usdz comes from: the adapter asks the vendor
    /// for its own export (`native`, the default) or its GLB (`glb`) — see
    /// `model3d_convert::asks_vendor_for`.
    pub format_source: Option<crate::types::Model3dFormatSource>,
    /// Also deliver each requested container's package (the file and its
    /// sidecars) as a zip (`output_packages`), applied by litegen on delivery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_packages: Option<bool>,
    /// What delivery does with a KTX2 texture (`ktx2_textures`), applied by
    /// litegen on delivery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ktx2_textures: Option<crate::types::Model3dKtx2Textures>,
    /// Vendor fields the canonical vocabulary does not cover, bounded by the
    /// model's `extra_allowlist` in strict mode. An adapter must never let one
    /// overwrite a field it derived from a canonical param above.
    pub extra: Option<serde_json::Value>,
    /// An operation's input mesh, already read, checked against the model's
    /// `source_model` spec and converted to a container it takes. `None` for
    /// generation models. Never serialised: it holds the mesh's bytes.
    #[serde(skip)]
    pub source: Option<SourceInput>,
    /// What the vendor's free preflight learned, for the submit to reuse
    /// (Tripo's uploaded file token and the rig type it detected). Never
    /// serialised.
    #[serde(skip)]
    pub preflight: Option<serde_json::Value>,
}

/// How an operation's source reached the vendor — recorded on its row
/// (`metadata.source.delivery`), so a chain can be audited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceDelivery {
    /// The vendor's own task id for the source (the source came from this
    /// vendor, on this account): nothing re-uploaded.
    VendorTask,
    /// Uploaded to the vendor's own storage.
    Upload,
    /// A short-lived URL litegen signed for the vendor to fetch.
    SignedUrl,
    /// Inline, as a `data:` URI.
    DataUri,
}

impl SourceDelivery {
    pub const fn as_str(self) -> &'static str {
        match self {
            SourceDelivery::VendorTask => "vendor_task",
            SourceDelivery::Upload => "upload",
            SourceDelivery::SignedUrl => "signed_url",
            SourceDelivery::DataUri => "data_uri",
        }
    }
}

/// One clip an animate request asks for, resolved against the model's catalog
/// row (3D doc §3C).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AnimationClip {
    /// The clip's name in the delivered file.
    pub name: String,
    /// The model's own preset id, resolved from a canonical one; `None` for a
    /// prompt clip.
    pub preset: Option<String>,
    /// Text-to-motion.
    pub prompt: Option<String>,
    /// A prompt clip's length in seconds.
    pub duration_s: Option<f64>,
    pub in_place: Option<bool>,
    pub loop_requested: Option<bool>,
}

impl AnimationClip {
    /// The request's clips with every preset as the model's own id.
    pub fn resolve(clips: &[crate::types::AnimationRequest], spec: &crate::capabilities::ParamSpecAnimationList) -> Vec<AnimationClip> {
        clips
            .iter()
            .map(|c| AnimationClip {
                name: c.name.clone(),
                preset: c.preset.as_deref().map(|p| {
                    spec.presets
                        .iter()
                        .find(|s| s.id == p)
                        .or_else(|| spec.presets.iter().find(|s| s.canonical.as_deref() == Some(p)))
                        .map_or_else(|| p.to_string(), |s| s.id.clone())
                }),
                prompt: c.prompt.clone(),
                duration_s: c.duration_s,
                in_place: c.in_place,
                loop_requested: c.loop_requested,
            })
            .collect()
    }
}

/// An operation's source mesh, as its adapter receives it.
#[derive(Debug, Clone)]
pub struct SourceInput {
    /// In `format`, a container the model takes.
    pub bytes: Vec<u8>,
    pub format: String,
    /// SHA-256 of the source as the caller sent it.
    pub sha256: String,
    pub facts: crate::proxy::model3d_source::SourceFacts,
    /// The source's own task id at this vendor, when it came from this same
    /// provider on this same vendor account: Tripo and Meshy chain by it, and
    /// some of their operations accept nothing else.
    pub vendor_task_id: Option<String>,
    /// The axis a litegen generation's delivered mesh faces, from its
    /// manifest (`+z`, or `unverified`); `None` for a file of the caller's own.
    pub front: Option<String>,
    /// The joint naming the source's rig was delivered in (3D doc §3B),
    /// from the plan its row recorded: the source's own, or for an
    /// animation the rig it animated. `None` for the vendor's names, for a
    /// file of the caller's own, and for a row whose plan is missing or
    /// does not read. An animation of the source that names none is
    /// delivered in it too (`DeliveryPlan::for_request`).
    pub joint_naming: Option<String>,
    /// Where the adapter says how it sent the source.
    pub delivered: Arc<std::sync::Mutex<Option<SourceDelivery>>>,
}

impl SourceInput {
    /// The adapter sent the source this way.
    pub fn record(&self, how: SourceDelivery) {
        *self.delivered.lock().unwrap_or_else(|p| p.into_inner()) = Some(how);
    }

    /// How the adapter said it sent the source.
    pub fn delivery(&self) -> Option<SourceDelivery> {
        *self.delivered.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// One file a provider produced, already downloaded. litegen re-hosts every one
/// of these on its own storage, so providers hand back BYTES, not vendor URLs —
/// several vendors expire their download URLs within minutes of task success.
#[derive(Debug, Clone)]
pub struct Model3dFile {
    pub kind: crate::types::Model3dAssetKind,
    /// Container/extension without the dot: "glb", "png", …. Free-form vendor
    /// text: rehost normalises it before it reaches a storage key.
    pub format: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    /// Meshes only; `None` when the provider does not report it. Never the
    /// requested `target_polycount` — that is the ask, not the result.
    pub polycount: Option<u32>,
    /// Pixel dimensions, for textures and previews whose vendor reports them.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// On delivered meshes: what litegen measured in them
    /// (`proxy::model3d_deliver`). Adapters leave it `None`.
    pub manifest: Option<crate::types::SceneManifest>,
    /// The vendor URL the bytes were downloaded from, which is what a
    /// generation that keeps nothing hands out (`keep_outputs: false`).
    /// `None` for bytes no URL serves, such as a file litegen derived.
    pub source_url: Option<String>,
}

/// Handle returned when a 3D generation is started (for polling).
#[derive(Debug, Clone)]
pub struct Model3dGenerationHandle {
    pub provider_job_id: String,
    pub provider: String,
    pub model: String,
    /// Opaque per-provider continuation state, written by `generate` and
    /// persisted on the row (`metadata.stage_context`) so the poller can
    /// rebuild the handle after a restart — fal keeps its queue URLs here.
    /// Only that provider's own `poll_status` / `start_stage` / `cancel` read it.
    ///
    /// Updated after submit through [`Model3dGenerationPollResult::stage_context`]
    /// and [`Model3dProvider::start_stage`]; both observers of a job persist
    /// the new value (see `proxy::model3d_stages`). By convention it records
    /// the current stage's name under `"stage"`.
    pub stage_context: Option<serde_json::Value>,
}

/// Status from polling a 3D generation.
///
/// On `Completed`, `files` MUST carry at least one `Mesh` — whatever the vendor
/// itself emitted, already downloaded. A completed generation without a mesh is
/// a contract violation, not a degraded success: every observer of completion
/// turns it into `failed`. Containers the caller asked for but the vendor did
/// not emit are derived by the caller of `poll_status`, never by the adapter.
///
/// A vendor's own verdict that the job failed — including "finished, but its
/// result cannot be retrieved" — is `Ok` with `status: Failed`, so it takes the
/// normal terminal path (and its webhook). `Err` is for not knowing yet:
/// retryable when asking again can help, non-retryable when it never will.
#[derive(Debug, Clone)]
pub struct Model3dGenerationPollResult {
    pub status: crate::types::GenerationStatus,
    pub progress: u8,
    pub files: Vec<Model3dFile>,
    pub error: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
    /// New continuation state, when this poll changed it (a vendor flow that
    /// moved to its next free stage, say). The observer persists it on the row
    /// and polls with it from then on.
    pub stage_context: Option<serde_json::Value>,
    /// The previous stage finished and this one, a BILLED vendor call, must
    /// start next. The observer does not start it directly: it claims the
    /// stage on the row first, and only the claimant calls
    /// [`Model3dProvider::start_stage`], so two observers never bill it twice.
    pub next_stage: Option<String>,
}

/// What an observer of a finished job may do with its files: download them
/// (it delivers the job), or stand aside (another observer does). Asked for
/// 3D files and for a finished video alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetch {
    /// Download them. `waited` is how long the observer waited for the
    /// right: an adapter whose links expire reads them again after
    /// [`REFRESH_LINKS_AFTER`].
    Go { waited: std::time::Duration },
    /// Another observer delivers the job: download nothing, and report it
    /// still processing, short of done, with no files and no error.
    StandAside,
}

/// Asked by an adapter once its vendor says a job finished, before it
/// downloads anything: whether this observer delivers the job. Only one
/// observer of a finished job (a client's poll or the poller, on any
/// replica) downloads its files; the server's delivery claim answers
/// (`proxy::delivery_lease::DeliveryGate`). An adapter asks once per poll.
#[async_trait]
pub trait FetchGate: Send + Sync {
    async fn ask(&self) -> Fetch;
}

/// A gate that always says go, at once: what a plain `poll_status` uses.
pub struct AlwaysFetch;

#[async_trait]
impl FetchGate for AlwaysFetch {
    async fn ask(&self) -> Fetch {
        Fetch::Go { waited: std::time::Duration::ZERO }
    }
}

/// An adapter that waited at least this long for its gate reads the job's
/// links again before downloading: a vendor's signed links can expire while
/// the poller waits for a delivery slot.
pub const REFRESH_LINKS_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

/// The core trait for 3D model (mesh) generation providers.
///
/// Async submit + poll, mirroring [`VideoProvider`], with one deliberate
/// divergence: `poll_status` returns downloaded bytes rather than provider URLs,
/// because litegen re-hosts every 3D output so that per-app BYO buckets work
/// identically across images, video, and meshes.
#[async_trait]
pub trait Model3dProvider: Send + Sync {
    fn name(&self) -> &str;
    fn configure(&mut self, config: ProviderInstanceConfig);
    fn is_configured(&self) -> bool;

    /// Start a 3D generation. Mode is inferred from `materialized.refs`: no refs
    /// → text-to-3D, an `init` ref → image-to-3D, `view-*` refs → multiview.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        materialized: &MaterializedRequest,
    ) -> Result<Model3dGenerationHandle, ProviderError>;

    /// Poll an in-flight generation. On `Completed`, every returned file must
    /// already be downloaded — the caller re-hosts within the same tick.
    async fn poll_status(
        &self,
        handle: &Model3dGenerationHandle,
    ) -> Result<Model3dGenerationPollResult, ProviderError>;

    /// [`poll_status`](Self::poll_status), asking `gate` once the vendor says
    /// the job finished and before downloading any of its files. On
    /// [`Fetch::StandAside`] it downloads nothing and reports the job still
    /// processing. An adapter that downloads overrides this, and its
    /// `poll_status` asks [`AlwaysFetch`]. The default never asks: the caller
    /// then claims after the poll.
    async fn poll_status_gated(
        &self,
        handle: &Model3dGenerationHandle,
        gate: &dyn FetchGate,
    ) -> Result<Model3dGenerationPollResult, ProviderError> {
        let _ = gate;
        self.poll_status(handle).await
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        request: &Model3dGenerationRequest,
    ) -> Result<CostEstimate, ProviderError>;

    async fn health_check(&self) -> HealthCheckResult;

    /// Start `stage` of a vendor flow whose previous stage finished — the
    /// call a poll asked for with `next_stage`. Called only by the observer
    /// that won the stage's claim. Returns the new stage context, which
    /// replaces the handle's from then on.
    ///
    /// An `Err` fails the generation: litegen never re-sends a billed call it
    /// cannot prove did not land. Providers without stages keep the default.
    async fn start_stage(
        &self,
        _handle: &Model3dGenerationHandle,
        stage: &str,
    ) -> Result<serde_json::Value, ProviderError> {
        Err(ProviderError::InvalidRequest(format!(
            "{} has no stage '{stage}' to start", self.name()
        )))
    }

    /// Run the vendor's free preflight (Tripo's rig-check) before the billed
    /// submit: once per request, after the source is prepared and the
    /// caller's limits are checked. A refusal answers the caller 422 and
    /// bills nothing. Providers without one proceed.
    async fn preflight(&self, _model: &ModelSchema, _extras: &Model3dExtras) -> Result<Preflight, ProviderError> {
        Ok(Preflight::Proceed { context: None })
    }

    /// Ask the vendor to stop a job the caller cancelled. Best-effort: the row
    /// is already `cancelled` and nothing reads the job again, so this only
    /// decides whether the VENDOR keeps working (and billing). Vendors without
    /// a cancel endpoint answer `Unsupported`.
    async fn cancel(&self, _handle: &Model3dGenerationHandle) -> Result<CancelOutcome, ProviderError> {
        Ok(CancelOutcome::Unsupported)
    }
}

/// What a vendor's free preflight said about a request, before anything is
/// billed (3D doc §3B).
#[derive(Debug, Clone, PartialEq)]
pub enum Preflight {
    /// Go ahead. `context` is what the submit may reuse (an uploaded file's
    /// token, a detected rig type); it reaches `generate` as
    /// `Model3dExtras::preflight`.
    Proceed { context: Option<serde_json::Value> },
    /// The vendor said no: the request is refused with `code` (422) and
    /// nothing is billed.
    Refuse { code: &'static str, message: String },
}

/// What a vendor said when asked to cancel a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    /// Accepted. A queued job will not run; a running one may still finish.
    Requested,
    /// The job had already finished; there was nothing to stop.
    AlreadyFinished,
    /// The vendor has no record of the job.
    NotFound,
    /// This provider has no way to cancel a job.
    Unsupported,
}

// ─── Shared Helpers ─────────────────────────────────────────────────────────

/// Whether a request that failed in transit may be sent again (PixelGenie
/// L1). One that never connected reached nobody. Once sent, a `billed` call —
/// a submit, or a generation answered in one call — may have been accepted
/// and charged whatever the client saw, so its timeout is final; a read (a
/// poll, a result download) may always be asked again.
pub fn may_resend_after(e: &reqwest::Error, billed: bool) -> bool {
    e.is_connect() || (!billed && e.is_timeout())
}

/// A vendor's 429 that says how long to wait (`Retry-After`), as
/// `RateLimited`, so the retry waits at least that long
/// (docs/gateway-settings.md). `None` for any other answer, which the caller
/// turns into its own error. Read `retry_after_ms` from the response's
/// headers before its body.
pub(crate) fn rate_limited(
    vendor: &str,
    status: reqwest::StatusCode,
    retry_after_ms: Option<u64>,
    message: &str,
    provider_error: Option<&serde_json::Value>,
) -> Option<ProviderError> {
    match (status.as_u16(), retry_after_ms) {
        (429, Some(retry_after_ms)) => Some(ProviderError::RateLimited {
            provider: vendor.to_string(),
            retry_after_ms,
            message: message.to_string(),
            provider_error: provider_error.cloned(),
        }),
        _ => None,
    }
}

/// A vendor's JSON answer, judged by its status before its body (PixelGenie
/// L15): a 5xx — often a gateway's page that is not JSON —, a 408 or a 429
/// turned the call away and may be sent again; any other error status is the
/// vendor's own refusal, its body kept as `provider_error` when it is JSON.
pub(crate) async fn read_vendor_json(resp: reqwest::Response, vendor: &str) -> Result<serde_json::Value, ProviderError> {
    let status = resp.status();
    let retry_after_ms = retry_after_ms(resp.headers());
    let text = resp.text().await.map_err(|e| ProviderError::RequestFailed {
        message: format!("{vendor}'s answer could not be read: {e}"),
        status_code: Some(status.as_u16()),
        provider_error: None,
        retryable: false,
    })?;
    let body = serde_json::from_str::<serde_json::Value>(&text).ok();
    if !status.is_success() {
        let shown = body.as_ref().map(|b| b.to_string()).unwrap_or_else(|| text.chars().take(200).collect());
        if let (429, Some(retry_after_ms)) = (status.as_u16(), retry_after_ms) {
            return Err(ProviderError::RateLimited {
                provider: vendor.to_string(),
                retry_after_ms,
                message: format!("{vendor} answered HTTP 429: {shown}"),
                provider_error: body,
            });
        }
        return Err(ProviderError::RequestFailed {
            message: format!("{vendor} answered HTTP {}: {shown}", status.as_u16()),
            status_code: Some(status.as_u16()),
            provider_error: body,
            retryable: status.is_server_error() || matches!(status.as_u16(), 408 | 429),
        });
    }
    body.ok_or_else(|| ProviderError::RequestFailed {
        message: format!("{vendor} answered HTTP {} with a body that is not JSON", status.as_u16()),
        status_code: Some(status.as_u16()),
        provider_error: None,
        retryable: false,
    })
}

/// How long a vendor asked to be left alone (`Retry-After`, RFC 9110 §10.2.3):
/// seconds, or an HTTP date (a date already past is 0). `None` when absent or
/// unreadable.
pub(crate) fn retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds.saturating_mul(1000));
    }
    let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some((at.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_milliseconds().max(0) as u64)
}

#[cfg(test)]
mod poll_budget_tests {
    use super::*;
    use std::time::{Duration, Instant};

    async fn rounds(mut budget: PollBudget) -> u32 {
        let mut n = 0;
        while budget.next().await {
            n += 1;
        }
        n
    }

    #[tokio::test]
    async fn the_poll_budget_ends_at_the_deadline_or_the_default() {
        // litegen's deadline for the call wins over the adapter's own budget.
        let started = Instant::now();
        // Bounded: a budget that ignored the deadline would wait out its hour.
        let n = tokio::time::timeout(
            Duration::from_secs(2),
            with_deadline(Instant::now() + Duration::from_millis(100), async {
                rounds(PollBudget::new(Duration::from_secs(3600), Duration::from_millis(30))).await
            }),
        )
        .await
        .expect("polled past the deadline");
        assert!((3..=5).contains(&n), "{n} rounds");
        assert!(started.elapsed() < Duration::from_millis(400), "ended at the deadline: {:?}", started.elapsed());

        // A deadline already past: not one round, not one interval's wait.
        let started = Instant::now();
        let n = tokio::time::timeout(
            Duration::from_secs(2),
            with_deadline(Instant::now(), async { rounds(PollBudget::new(Duration::from_secs(3600), Duration::from_secs(2))).await }),
        )
        .await
        .expect("waited although the deadline had passed");
        assert_eq!(n, 0);
        assert!(started.elapsed() < Duration::from_millis(100));

        // Called directly, with no deadline: the adapter's own budget.
        let started = Instant::now();
        let n = rounds(PollBudget::new(Duration::from_millis(100), Duration::from_millis(30))).await;
        assert!((3..=5).contains(&n), "{n} rounds");
        assert!(started.elapsed() < Duration::from_millis(400));
    }
}

#[cfg(test)]
mod rate_limit_tests {
    use super::*;

    fn answer(status: u16, retry_after: Option<String>) -> reqwest::Response {
        let mut b = axum::http::Response::builder().status(status);
        if let Some(v) = retry_after {
            b = b.header("retry-after", v);
        }
        reqwest::Response::from(b.body(r#"{"error":{"message":"slow down"}}"#).unwrap())
    }

    #[tokio::test]
    async fn a_429_with_retry_after_is_rate_limited_in_seconds_or_a_date() {
        match read_vendor_json(answer(429, Some("2".into())), "acme").await {
            Err(ProviderError::RateLimited { retry_after_ms, message, provider_error, .. }) => {
                assert_eq!(retry_after_ms, 2000);
                assert!(message.contains("429") && message.contains("slow down"), "{message}");
                assert_eq!(provider_error, Some(serde_json::json!({ "error": { "message": "slow down" } })), "the vendor's body is kept");
            }
            other => panic!("{other:?}"),
        }
        // An HTTP date, a few seconds out.
        let at = (chrono::Utc::now() + chrono::Duration::seconds(3)).to_rfc2822().replace("+0000", "GMT");
        match read_vendor_json(answer(429, Some(at)), "acme").await {
            Err(ProviderError::RateLimited { retry_after_ms, .. }) => assert!((1000..=3000).contains(&retry_after_ms), "{retry_after_ms}"),
            other => panic!("{other:?}"),
        }
        // A date already past waits nothing; a 429 without one, or a 503, stays a plain refusal.
        let past = (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc2822().replace("+0000", "GMT");
        assert!(matches!(read_vendor_json(answer(429, Some(past)), "acme").await, Err(ProviderError::RateLimited { retry_after_ms: 0, .. })));
        for (status, header) in [(429, None), (503, Some("2".to_string())), (429, Some("soon".to_string()))] {
            assert!(
                matches!(read_vendor_json(answer(status, header.clone()), "acme").await, Err(ProviderError::RequestFailed { retryable: true, .. })),
                "{status} {header:?}"
            );
        }
    }
}

#[cfg(test)]
mod resend_tests {
    use super::may_resend_after;

    #[tokio::test]
    async fn a_timed_out_billed_call_is_never_sent_again_but_a_read_may_be() {
        use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(any()).respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(5))).mount(&server).await;
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let timed_out = client.get(server.uri()).send().await.unwrap_err();
        assert!(timed_out.is_timeout(), "{timed_out}");
        assert!(!may_resend_after(&timed_out, true), "a submit that timed out may have been accepted and billed");
        assert!(may_resend_after(&timed_out, false), "a poll or a download is asked again");
        let refused = client.get("http://127.0.0.1:1/").send().await.unwrap_err();
        assert!(refused.is_connect(), "{refused}");
        assert!(may_resend_after(&refused, true), "a call that never connected reached nobody");
    }

    /// Every adapter takes the rule above rather than its own: an inline
    /// `is_timeout()` resend re-bills a job the vendor accepted.
    #[test]
    fn no_adapter_decides_for_itself_that_a_timed_out_call_may_be_sent_again() {
        fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (n, line) in text.lines().enumerate() {
                        if line.contains(concat!("retry", "able:")) && line.contains(concat!("is_time", "out()")) {
                            found.push(format!("{}:{}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        let mut found = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/providers"), &mut found);
        assert!(found.is_empty(), "use may_resend_after: {found:#?}");
    }
}

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
        tokens_required: usd_to_tokens(total_cost_usd, crate::types::generation::USD_PER_TOKEN),
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

/// Test only: how many shared rotation cursors the process holds.
#[cfg(test)]
fn pool_cursor_count() -> usize {
    POOL_CURSORS.get().map_or(0, |m| m.read().unwrap().len())
}

/// How many rotation cursors the process keeps before it drops the ones no
/// pool holds (registry NEW 2): one per distinct key pool otherwise piled up
/// for the life of the process, an org's own keys included.
const MAX_POOL_CURSORS: usize = 1024;

fn shared_cursor(fingerprint: u64) -> Arc<AtomicUsize> {
    let map = POOL_CURSORS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()));
    if let Some(c) = map.read().unwrap().get(&fingerprint) {
        return Arc::clone(c);
    }
    let mut cursors = map.write().unwrap();
    if cursors.len() >= MAX_POOL_CURSORS && !cursors.contains_key(&fingerprint) {
        // A pool in use holds its cursor, so it keeps its place; an idle
        // one starts its rotation again at its first key.
        cursors.retain(|_, c| Arc::strong_count(c) > 1);
    }
    Arc::clone(cursors.entry(fingerprint).or_insert_with(|| Arc::new(AtomicUsize::new(0))))
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

#[cfg(test)]
mod instance_config_tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mapping(pairs: &[(&str, &str)]) -> ProviderInstanceConfig {
        ProviderInstanceConfig {
            model_mapping: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn mapped_model_matches_the_catalog_id_then_the_bare_id() {
        let full = mapping(&[("bfl/flux-pro-1.1", "flux-pro-1.1-finetuned"), ("flux-pro-1.1", "bare-loses")]);
        assert_eq!(full.mapped_model("bfl/flux-pro-1.1"), Some("flux-pro-1.1-finetuned"), "the catalog id wins");
        let bare = mapping(&[("flux-dev", "my-flux")]);
        assert_eq!(bare.mapped_model("bfl/flux-dev"), Some("my-flux"), "the id without its provider prefix");
        assert_eq!(bare.mapped_model("flux-dev"), Some("my-flux"));
        assert_eq!(bare.mapped_model("bfl/flux-pro"), None);
        assert_eq!(ProviderInstanceConfig::default().mapped_model("bfl/flux-dev"), None);
    }

    #[tokio::test]
    async fn api_client_sends_extra_headers_and_a_request_header_wins() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let headers: HashMap<String, String> =
            [("cf-aig-authorization", "Bearer gw"), ("Authorization", "Bearer from-config")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        let client = api_client(std::time::Duration::from_secs(30), &headers);
        client.get(server.uri()).header("Authorization", "Bearer from-request").send().await.unwrap();
        let got = &server.received_requests().await.unwrap()[0];
        assert_eq!(got.headers.get("cf-aig-authorization").unwrap(), "Bearer gw");
        let auth: Vec<_> = got.headers.get_all("authorization").iter().collect();
        assert_eq!(auth, vec!["Bearer from-request"], "a header the request sets wins, once");
    }

    /// P8: adapters configured alike share one client, so one pool: two asked
    /// for with the same timeout and headers reach the vendor over one
    /// connection. Another header set is another client, with its headers.
    #[tokio::test]
    async fn adapters_configured_alike_share_one_client() {
        use std::sync::atomic::Ordering::SeqCst;
        let echo = axum::Router::new().route(
            "/",
            axum::routing::get(|h: axum::http::HeaderMap| async move { h.get("x-tenant").and_then(|v| v.to_str().ok()).unwrap_or("").to_string() }),
        );
        let (base, accepted) = crate::util::counting_vendor(echo).await;
        let set = |v: &str| -> HashMap<String, String> { [("x-tenant".to_string(), v.to_string())].into_iter().collect() };
        let timeout = std::time::Duration::from_secs(30);
        for _ in 0..2 {
            let got = api_client(timeout, &set("a")).get(format!("{base}/")).send().await.unwrap().text().await.unwrap();
            assert_eq!(got, "a");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(accepted.load(SeqCst), 1, "two clients configured alike shared one connection");
        let got = api_client(timeout, &set("b")).get(format!("{base}/")).send().await.unwrap().text().await.unwrap();
        assert_eq!(got, "b", "another header set gets its own client, with its own headers");
        assert_eq!(accepted.load(SeqCst), 2);
    }

    /// The shared client's key covers header names and values, framed, so
    /// two different sets (a secret's value, a split moved between name and
    /// value) never share a client; the order of the set doesn't matter.
    #[test]
    fn the_header_digest_frames_names_and_values() {
        let set = |pairs: &[(&str, &str)]| -> HashMap<String, String> { pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() };
        assert_ne!(headers_digest(&set(&[("k", "secret-1")])), headers_digest(&set(&[("k", "secret-2")])));
        assert_ne!(headers_digest(&set(&[("a", "bc")])), headers_digest(&set(&[("ab", "c")])));
        assert_ne!(headers_digest(&set(&[])), headers_digest(&set(&[("", "")])));
        assert_eq!(headers_digest(&set(&[("a", "1"), ("b", "2")])), headers_digest(&set(&[("b", "2"), ("a", "1")])));
    }

    /// Registry NEW 2 (code review 2026-11-11): the shared rotation cursors
    /// are bounded. Every distinct key pool (one per org's stored key, with
    /// hosted BYO) added one for the life of the process. A pool still in use
    /// keeps its place in the rotation across the pruning.
    #[test]
    fn the_shared_rotation_cursors_are_bounded_and_a_live_pool_keeps_its_place() {
        let pool = |keys: &[&str]| ApiKeyPool::shared(keys.iter().map(|k| crate::types::ApiKeyEntry { key: k.to_string(), weight: 1, label: None }).collect());
        let live = pool(&["new2-live-a", "new2-live-b"]);
        assert_eq!(live.next(), "new2-live-a");
        for i in 0..3000 {
            let _ = pool(&[&format!("new2-gone-{i}")]);
        }
        let n = pool_cursor_count();
        assert!(n <= 1025, "the process holds {n} rotation cursors after 3000 one-off pools");
        assert_eq!(pool(&["new2-live-a", "new2-live-b"]).next(), "new2-live-b", "a pool in use kept its place in the rotation");
        drop(live);
    }

    #[test]
    fn check_extra_headers_refuses_invalid_and_framing_headers() {
        let one = |k: &str, v: &str| -> HashMap<String, String> { [(k.to_string(), v.to_string())].into_iter().collect() };
        assert!(check_extra_headers(&one("Helicone-Auth", "Bearer x")).is_ok());
        assert!(check_extra_headers(&HashMap::new()).is_ok());
        for (k, v) in [
            ("bad header", "x"),
            ("x-ok", "line\r\nbreak"),
            ("Host", "evil.example"),
            ("content-length", "5"),
            ("Transfer-Encoding", "chunked"),
            ("connection", "close"),
            ("keep-alive", "1"),
            ("upgrade", "h2c"),
            ("te", "trailers"),
            ("trailer", "x"),
            ("proxy-authorization", "Basic x"),
            ("proxy-connection", "close"),
        ] {
            let err = check_extra_headers(&one(k, v)).expect_err(k);
            assert!(err.contains(k.trim()), "{err} names {k}");
        }
    }
}
