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

/// Where an adapter that submits a job and then waits for it records the
/// vendor's job id, the moment the vendor accepts it.
///
/// The router reads it after a failure. A job the vendor accepted is never
/// sent again, since the vendor may bill it, and a timeout reports its id.
/// Adapters whose call is a single request-response leave it empty.
#[derive(Debug, Clone, Default)]
pub struct VendorJobSink(Arc<std::sync::Mutex<Option<String>>>);

impl VendorJobSink {
    /// The vendor accepted job `id`.
    pub fn record(&self, id: impl Into<String>) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(id.into());
    }

    /// The job id recorded since the last [`clear`](Self::clear), if any.
    pub fn get(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Forget the recorded id, before a fresh attempt.
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = None;
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
    /// The clips to make, in order (3D doc §3C), presets resolved to the
    /// model's own ids.
    pub animations: Vec<AnimationClip>,
    /// Frames per second of the delivered clips.
    pub fps: Option<u32>,
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
