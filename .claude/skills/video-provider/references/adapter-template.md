# Video adapter template

This is an annotated skeleton for a video vendor with two endpoints
(text-to-video, and image-to-video with an optional last frame), discrete
durations, and a poll path that depends on the endpoint. That combination
shows every pattern you're likely to need. "Acme" is a placeholder: replace
every URL, field and status with what the vendor's docs say, and quote them
in the `@see … Verbatim:` lines. For real files in the same style, read
`providers/video/vidu.rs` (choosing an endpoint from the roles) and
`providers/video/kling.rs` (a `{kind}/{task_id}` handle).

## Contents

- [Adapter](#adapter-litegen-coresrcprovidersvideoacmers)
- [Tests](#tests-same-file)
- [Catalog row](#catalog-row-modelsacmeyaml)
- [Live test](#live-test-litegen-coretestslive_providersrs)

## Adapter (`litegen-core/src/providers/video/acme.rs`)

```rust
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    build_cost_estimate, ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError,
    ProviderInstanceConfig, VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider,
};
use crate::types::*;

/// Acme video provider.
///
/// Async. `POST {api_base}/v2/videos/{text|image}` returns `{id}`. Poll
/// `GET {api_base}/v2/videos/{text|image}/{id}` until `status` is terminal,
/// then read `output.url`, a public CDN URL kept for 7 days. Auth is
/// `Authorization: Bearer <key>`.
///
/// @see <https://docs.acme.example/reference/text-to-video>
///   Verbatim: "POST https://api.acme.example/v2/videos/text"
/// @see <https://docs.acme.example/reference/image-to-video>
///   Verbatim: "first_frame (required), last_frame (optional): base64-encoded image"
/// @see <https://docs.acme.example/reference/get-video>
///   Verbatim: "status: queued | running | succeeded | failed | cancelled"
///   Verbatim: "output.url remains available for 7 days"
/// @see <https://docs.acme.example/errors> — `{"error": {"code", "message"}}`
pub struct AcmeVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

/// Which endpoint a job went to. It is also the poll path segment, so it is
/// stored in the handle; the poller has nothing else to go on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Text,
    Image,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Image => "image",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "text" => Some(Kind::Text),
            "image" => Some(Kind::Image),
            _ => None,
        }
    }
}

impl Default for AcmeVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AcmeVideoProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(60)) // submits only; well under the router's 120 s
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.acme.example")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("acme".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// One arm per video row in models/acme.yaml. Unknown ids are errors,
    /// never a default model.
    fn resolve_model(id: &str) -> Result<&'static str, ProviderError> {
        match id {
            // @see <https://docs.acme.example/models/clip-2> — API page
            // @see <https://docs.acme.example/openapi.json> (2026-10-12) — CreateTextVideo, CreateImageVideo
            // @see <https://acme.example/pricing.md> (2026-10-12) — "$0.05 per second (720p)"
            "acme/clip-2" => Ok("clip-2"),
            other => Err(ProviderError::ModelNotSupported { model: other.to_string(), provider: "acme".into() }),
        }
    }

    /// Acme accepts `duration` of 5 or 10 only, and the catalog can only say
    /// 5..=10. Snap deterministically; ties go to the shorter clip.
    /// @see <https://docs.acme.example/openapi.json> — Verbatim: "duration: integer, one of 5, 10"
    fn resolve_duration(requested: f64) -> u32 {
        if requested <= 7.5 { 5 } else { 10 }
    }
}

fn acme_error_message(body: &Value) -> String {
    body["error"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string().chars().take(300).collect())
}

#[async_trait]
impl VideoProvider for AcmeVideoProvider {
    fn name(&self) -> &str {
        "acme"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
        }
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.key_pool.is_some())
    }

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id)?;

        // Choose the endpoint from the roles present, and refuse a mode the
        // row doesn't support BEFORE any vendor call.
        let frame = |role: &str| -> Result<Option<String>, ProviderError> {
            match materialized.refs.iter().find(|r| r.role == role).map(|r| &r.form) {
                None => Ok(None),
                Some(MaterializedRefForm::Base64(b64)) => Ok(Some(b64.clone())),
                Some(_) => Err(ProviderError::InvalidRequest(format!(
                    "{}: {role} did not arrive as base64", model.id
                ))),
            }
        };
        let first = frame("first_frame")?;
        let last = frame("last_frame")?;
        let kind = match (&first, &last) {
            (Some(_), _) => Kind::Image,
            (None, Some(_)) => {
                return Err(ProviderError::InvalidRequest(format!(
                    "{}: a last_frame needs a first_frame", model.id
                )))
            }
            (None, None) if model.capabilities.text_to_video => Kind::Text,
            (None, None) => {
                return Err(ProviderError::InvalidRequest(format!(
                    "{} has no text-to-video mode; attach a first_frame", model.id
                )))
            }
        };

        // `extra` first, so the fields this adapter owns win (lax mode passes
        // `extra` through unfiltered).
        let mut body = match &extras.extra {
            Some(Value::Object(m)) => Value::Object(m.clone()),
            _ => json!({}),
        };
        body["model"] = json!(native);
        body["prompt"] = json!(base.prompt);
        body["duration"] = json!(Self::resolve_duration(extras.duration_seconds)); // an INTEGER on the wire
        if let Some(res) = extras.resolution.as_deref() {
            body["resolution"] = json!(res); // the YAML enum is the vendor enum
        }
        if let Some(seed) = base.seed {
            body["seed"] = json!(seed);
        }
        match kind {
            // CreateTextVideo.aspect_ratio exists. CreateImageVideo has no
            // such field: the frame decides it, so the field is never sent there.
            Kind::Text => {
                if let Some(ar) = extras.aspect_ratio.as_deref() {
                    body["aspect_ratio"] = json!(ar);
                }
            }
            Kind::Image => {
                body["first_frame"] = json!(first);
                if let Some(l) = last {
                    body["last_frame"] = json!(l);
                }
            }
        }

        let url = format!("{}/v2/videos/{}", self.api_base(), kind.as_str());
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.post(&url).json(&body))?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Acme submit failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // Only a connect failure proves nothing was sent; a timeout
                // after sending may hide a job Acme accepted and billed.
                retryable: e.is_connect(),
            })?;
        let status = resp.status();
        let data: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Acme API error (HTTP {status}): {}", acme_error_message(&data)),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.is_server_error() || status.as_u16() == 429,
            });
        }
        let id = data["id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Acme submit response is missing `id`".into(),
            status_code: Some(status.as_u16()),
            provider_error: Some(data.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            // Everything poll_status needs. The poller rebuilds the handle
            // from the DB row with only these three fields.
            provider_job_id: format!("{}/{id}", kind.as_str()),
            provider: "acme".into(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(&self, handle: &VideoGenerationHandle) -> Result<VideoGenerationPollResult, ProviderError> {
        let (kind, id) = handle
            .provider_job_id
            .split_once('/')
            .and_then(|(k, id)| Kind::parse(k).map(|k| (k, id)))
            .ok_or_else(|| ProviderError::RequestFailed {
                message: format!("malformed Acme job id {:?}", handle.provider_job_id),
                status_code: None,
                provider_error: None,
                retryable: false, // can never succeed; fail the row
            })?;
        let creds = self.creds()?;
        let url = format!("{}/v2/videos/{}/{id}", self.api_base(), kind.as_str());
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Acme poll failed: {e}"),
                status_code: None,
                provider_error: None,
                retryable: true, // transient: the poller asks again next tick
            })?;
        // 4xx → terminal, 5xx/408/429 → retryable, non-JSON → error.
        let data = super::read_poll_json(resp, "acme").await?;

        let mut metadata = HashMap::new();
        metadata.insert("acme_job_id".to_string(), json!(id));

        let status = match data["status"].as_str() {
            Some("queued") => GenerationStatus::Pending,
            Some("running") => GenerationStatus::Processing,
            Some("succeeded") => GenerationStatus::Completed,
            Some("failed") | Some("cancelled") => GenerationStatus::Failed,
            other => {
                // Undocumented: loud, but not fatal. The poller keeps
                // asking and reaps the row after 2 h.
                return Err(ProviderError::RequestFailed {
                    message: format!("Acme job {id} reported an undocumented status {other:?}"),
                    status_code: None,
                    provider_error: Some(data),
                    retryable: true,
                });
            }
        };
        let video_url = data["output"]["url"].as_str().map(str::to_string);

        // Success without a URL is a failure: the poller would otherwise
        // store a completed row with nothing in it.
        if status == GenerationStatus::Completed && video_url.is_none() {
            return Ok(VideoGenerationPollResult {
                status: GenerationStatus::Failed,
                progress: 100,
                video_url: None,
                video_data: None,
                content_type: None,
                error: Some(format!("Acme job {id} succeeded without output.url")),
                metadata,
            });
        }

        let progress = match status {
            GenerationStatus::Completed | GenerationStatus::Failed => 100,
            // Acme reports 0–100. Cap at 99 until it's actually done.
            _ => data["progress"].as_u64().map(|p| p.min(99) as u8).unwrap_or(50),
        };
        Ok(VideoGenerationPollResult {
            status,
            progress,
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error: (status == GenerationStatus::Failed).then(|| acme_error_message(&data)),
            metadata,
        })
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &VideoGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, Some(json!({ "model": model.id }))))
    }

    async fn health_check(&self) -> HealthCheckResult {
        HealthCheckResult {
            healthy: self.is_configured(),
            message: if self.is_configured() { "Acme provider configured" } else { "Acme provider not configured" }.into(),
            latency_ms: None,
        }
    }
}
```

## Tests (same file)

The helpers (`registry`, `ref_schema`, `make_provider`, `make_base`,
`no_refs`, `submitted_body` and a pooled provider) have the same shape as in
`.claude/skills/image-provider/references/adapter-template.md`. The
video-specific cases:

```rust
fn make_extras() -> VideoExtras {
    VideoExtras { duration_seconds: 5.0, aspect_ratio: None, resolution: None, fps: None, extra: None }
}

fn frame(role: &str) -> MaterializedRef {
    MaterializedRef { role: role.into(), form: MaterializedRefForm::Base64("QUJD".into()) }
}

#[tokio::test]
async fn a_first_frame_routes_to_image_to_video_and_aspect_ratio_is_not_sent() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v2/videos/image"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "v1" })))
        .mount(&server).await;
    let mut extras = make_extras();
    extras.aspect_ratio = Some("16:9".into()); // set, so the absence check below means something
    let refs = MaterializedRequest { refs: vec![frame("last_frame"), frame("first_frame")], cleanup: Cleanup::empty() };

    let h = make_provider(&server.uri())
        .generate(&ref_schema("acme/clip-2"), &make_base("acme/clip-2"), &extras, &refs)
        .await
        .unwrap();
    assert_eq!(h.provider_job_id, "image/v1");

    let body = submitted_body(&server).await;
    assert_eq!(body["first_frame"], "QUJD");
    assert_eq!(body["last_frame"], "QUJD");
    assert_eq!(body["duration"], 5);            // an integer, as the schema says
    assert!(body.get("aspect_ratio").is_none(), "CreateImageVideo has no aspect_ratio");
}

#[tokio::test]
async fn a_text_only_request_to_an_image_only_row_is_refused_without_a_vendor_call() {
    let server = MockServer::start().await; // nothing mounted
    let err = make_provider(&server.uri())
        .generate(&ref_schema("acme/clip-2-i2v"), &make_base("acme/clip-2-i2v"), &make_extras(), &no_refs())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidRequest(_)));
    assert!(server.received_requests().await.unwrap().is_empty(), "nothing may be billed");
}

#[test]
fn durations_snap_to_the_two_values_acme_accepts() {
    for (asked, sent) in [(5.0, 5), (6.0, 5), (7.5, 5), (7.6, 10), (10.0, 10)] {
        assert_eq!(AcmeVideoProvider::resolve_duration(asked), sent, "asked {asked}");
    }
}

#[tokio::test]
async fn every_documented_status_maps_and_success_without_a_url_fails() {
    let server = MockServer::start().await;
    let p = make_provider(&server.uri());
    // The poller rebuilds the handle from the row like this; nothing else survives a restart.
    let handle = VideoGenerationHandle { provider_job_id: "text/v9".into(), provider: "acme".into(), model: "acme/clip-2".into() };
    for (vendor, ours, url) in [
        ("queued", GenerationStatus::Pending, None),
        ("running", GenerationStatus::Processing, None),
        ("succeeded", GenerationStatus::Completed, Some("https://cdn.acme.example/v9.mp4")),
        ("succeeded", GenerationStatus::Failed, None),     // no URL → failed, not an empty completion
        ("failed", GenerationStatus::Failed, None),
        ("cancelled", GenerationStatus::Failed, None),
    ] {
        server.reset().await;
        let mut body = json!({ "status": vendor });
        if let Some(u) = url { body["output"] = json!({ "url": u }); }
        Mock::given(method("GET")).and(path("/v2/videos/text/v9"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server).await;
        let r = p.poll_status(&handle).await.unwrap();
        assert_eq!(r.status, ours, "vendor status {vendor} with url {url:?}");
    }
}

#[tokio::test]
async fn a_revoked_key_on_poll_is_terminal_but_a_5xx_is_retried() {
    let server = MockServer::start().await;
    let p = make_provider(&server.uri());
    let handle = VideoGenerationHandle { provider_job_id: "text/v9".into(), provider: "acme".into(), model: "acme/clip-2".into() };
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    assert!(!p.poll_status(&handle).await.unwrap_err().is_retryable());
    server.reset().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    assert!(p.poll_status(&handle).await.unwrap_err().is_retryable());
}
```

Also add the catalog-iterating resolution test (filtered on
`MediaType::Video`), the pooled-key test, and the "`n` and `extra.model`
don't leak" test, all in the image template's shape.

## Catalog row (`models/acme.yaml`)

```yaml
  # ─── Video ────────────────────────────────────────────────────────────────
  # Acme Clip 2 on POST /v2/videos/{text|image} (model `clip-2`).
  # @see https://docs.acme.example/openapi.json (2026-10-12) — CreateTextVideo, CreateImageVideo
  - id: acme/clip-2
    provider: acme
    media_type: video
    display_name: Acme Clip 2
    description: Acme Clip 2 text-to-video and image-to-video with an optional end frame.
    docs_url: https://docs.acme.example/models/clip-2
    # "$0.05 per second (720p)", "$0.09 per second (1080p)".
    # Default request (720p, 5 s): $0.05 × 5 = $0.25. A 10 s 1080p clip costs
    # $0.90 and is billed flat at the base.
    # @see https://acme.example/pricing.md (2026-10-12)
    pricing: { base_cost_usd: 0.25 }
    capabilities: { text_to_video: true, image_to_video: true }
    prompt: { required: true, max_length: 2000 }
    params:
      # "duration: integer, one of 5, 10". The schema can only state a range;
      # the adapter snaps to 5 or 10 (resolve_duration).
      duration_seconds: { kind: float, min: 5.0, max: 10.0, default: 5.0 }
      # The text and image endpoints accept the same resolution enum.
      resolution: { kind: string, enum_values: [720p, 1080p], default: 720p }
      # Text-to-video only. CreateImageVideo has no aspect_ratio (the first
      # frame sets the shape), so image-to-video requests ignore it.
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["16:9", "9:16", "1:1"]
        default: "16:9"
      seed: { kind: seed, min: 0, max: 4294967295 }
    ref_inputs:
      max_total: 2
      default_role: first_frame
      provider_format: { form: base64 }
      roles:
        first_frame: { required: false, min_count: 0, max_count: 1 }
        last_frame:  { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: [camera_fixed]
    tags: [text-to-video, image-to-video]
```

An image-to-video-only sibling (`acme/clip-2-i2v`) declares
`capabilities: { image_to_video: true }` and makes `first_frame`
`{ required: true, min_count: 1, max_count: 1 }`. Its conformance test asserts
`!text_to_video`.

## Live test (`litegen-core/tests/live_providers.rs`)

```rust
#[tokio::test]
#[ignore = "live: set ACME_API_KEY"]
async fn live_acme_video() {
    skip_if_missing!("ACME_API_KEY");
    let mut p = litegen::providers::video::acme::AcmeVideoProvider::new();
    p.configure(api_key_cfg(&env("ACME_API_KEY").unwrap()));
    let h = p
        .generate(&schema("acme/clip-2"), &base("a paper boat on a stream", "acme/clip-2"), &vid_extras(), &empty_mat())
        .await
        .expect("acme submit");
    let url = drive_video(&p, h).await.expect("a video url");
    // The customer receives this URL as-is, so it must work WITHOUT our credentials.
    let resp = reqwest::get(&url).await.expect("fetch video");
    assert!(resp.status().is_success(), "video url returned {}", resp.status());
    let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(ct.starts_with("video/"), "video url served {ct:?}");
}
```

`vid_extras()` in the live file sends 5 s at 16:9. Check that your row
accepts that, or build extras that suit it.
