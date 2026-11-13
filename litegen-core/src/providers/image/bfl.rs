use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Black Forest Labs (FLUX) direct image provider.
///
/// Asynchronous: `POST /v1/{model}` returns `{id, polling_url}`; poll the
/// returned `polling_url` until `status == "Ready"`, then download
/// `result.sample` (a signed URL valid ~10 min). Authenticated with the `x-key`
/// header. Image adapter: BFL's video endpoints (`POST /v1/flux-3-video`,
/// "Generate a video with FLUX 3.", and the flux-tools video-edit-v1 /
/// video-upscale-v1 in api.bfl.ai/openapi.json, 2026-11-09) are not carried.
///
/// @see <https://docs.bfl.ml/flux_models/flux_1_1_pro> — generate + poll (2026-11-09)
///   Verbatim (the page's `\` line breaks joined): "request=$(curl -X POST 'https://api.bfl.ai/v1/flux-pro-1.1' -H 'accept: application/json' -H \"x-key: ${BFL_API_KEY}\" -H 'Content-Type: application/json' -d '{ \"prompt\": \"A futuristic city skyline at sunset with flying cars\", \"width\": 1024, \"height\": 1024 }')"
///   Verbatim: "echo \"Result: $(echo $result | jq -r .result.sample)\""
/// @see <https://docs.bfl.ml/kontext/kontext_image_editing> — input_image (base64) editing
/// What litegen itself sends; `extra` cannot replace it. Every
/// `input_image*` field (`input_image_2` … `input_image_8`) is litegen's too,
/// and FLUX 3's `images` list.
const OWNED_FIELDS: &[&str] = &[
    "prompt", "width", "height", "aspect_ratio", "seed", "output_format", "image_prompt", "steps", "guidance", "images", "resolution",
];

/// litegen's tier as FLUX 3's `resolution`: "an equal-pixel-area tier:
/// `768sq`, `1k` (default), `1.5k`, `2k`, or `4k`", and FLUX 3 Image's
/// `match_input` ("edits only").
/// @see <https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3> (2026-11-03)
fn bfl_resolution(resolution: &str) -> Option<&'static str> {
    match resolution {
        "0.75K" => Some("768sq"),
        "1K" => Some("1k"),
        "1.5K" => Some("1.5k"),
        "2K" => Some("2k"),
        "4K" => Some("4k"),
        "match_input" => Some("match_input"),
        _ => None,
    }
}

/// Whether an endpoint takes its references as one `images` list rather than
/// `input_image`, `input_image_2` …: FLUX 3 Image ("images … Optional
/// reference image(s) the prompt edits or draws from; each is an http(s) URL
/// or base64, one to 10 total") and Klein ("Same request as FLUX 3 Image:
/// `prompt`, `images`, `aspect_ratio`, `resolution` …").
/// @see <https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3> (2026-11-03)
/// @see <https://docs.bfl.ml/release-notes> (FLUX 3 Image Klein, October 15, 2026)
fn takes_an_images_list(native: &str) -> bool {
    matches!(native, "flux-3-image" | "flux-3-image-klein")
}

/// litegen's layout as FLUX 3's bounding boxes: "Write a description using
/// `<id>` to name each element, then append a JSON array of rows", each
/// `{id, bbox, desc}`; "Every box is `[top, left, bottom, right]` in integers
/// from 0 to 1000, measured from the top-left corner", the grid stretching
/// with the frame as litegen's fractions do. A text box quotes its words
/// ("quote the exact words in `desc`, such as `text reading "Sauna"`").
/// @see <https://docs.bfl.ml/flux_3/flux3_image_bounding_boxes.md> (2026-11-03)
fn flux_3_layout_prompt(prompt: &str, layout: &[crate::types::LayoutItem]) -> String {
    use crate::capabilities::LayoutKind;
    let grid = |v: f64| (v * 1000.0).round().clamp(0.0, 1000.0) as u32;
    let (mut objects, mut texts) = (0, 0);
    let mut named = Vec::new();
    let mut rows = Vec::new();
    for item in layout {
        let (id, desc) = match item.kind {
            LayoutKind::Object => {
                objects += 1;
                (format!("object_{objects}"), item.prompt.clone().unwrap_or_default())
            }
            LayoutKind::Text => {
                texts += 1;
                (format!("text_{texts}"), format!("text reading \"{}\"", item.text.as_deref().unwrap_or_default()))
            }
        };
        let b = item.bounds;
        named.push(format!("{desc} <{id}>"));
        rows.push(json!({ "id": id, "bbox": [grid(b.y), grid(b.x), grid(b.y + b.h), grid(b.x + b.w)], "desc": desc }));
    }
    format!("{} It shows {}. {}", prompt.trim_end(), named.join(", "), Value::Array(rows))
}

/// FLUX 3 Image: "width x height at most 16822272 pixels, and at most 4194304
/// with reference images". The row's one size list serves both modes, so an
/// edit asking for more is refused here.
/// @see <https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3> (2026-11-03)
const FLUX_3_IMAGE_EDIT_MAX_PIXELS: u64 = 4_194_304;

/// The poll statuses BFL documents besides `Ready`: "`Pending`, `Reasoning`,
/// `Generating` | In progress | Keep polling", and the endings.
/// @see <https://docs.bfl.ml/flux_3/flux3_image_generate.md> (2026-10-30)
const RUNNING: &[&str] = &["Pending", "Reasoning", "Generating"];
/// Error, Content/Request Moderated, Task not found, No Card Generated.
const ENDINGS: &[&str] = &["Error", "Content Moderated", "Request Moderated", "Task not found", "No Card Generated"];

/// Whether a poll answer names a status BFL documents.
fn documented_status(data: &Value) -> bool {
    data["status"].as_str().is_some_and(|s| s == "Ready" || RUNNING.contains(&s) || ENDINGS.contains(&s))
}

pub struct BflProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
    /// Result downloads: no redirects, so a URL that passed the public-address
    /// check cannot bounce into a private one.
    asset_client: Client,
    /// Whether a result URL must resolve to a public address. Only the tests,
    /// whose mock vendor runs on loopback, turn it off.
    pub(crate) public_assets_only: bool,
}

impl BflProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(180), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::raw_header("x-key"),
            client: Self::api_client(&Default::default()),
            asset_client: crate::util::ssrf::public_only_download_client(
                std::time::Duration::from_secs(120),
                std::time::Duration::from_secs(30),
            ),
            public_assets_only: true,
        }
    }

    /// Whether `url` may be polled with the key: on the configured base's
    /// origin, or an https host under `bfl.ai` ("polling_url points at the
    /// region that holds your task, so don't build the URL yourself").
    /// @see <https://docs.bfl.ml/flux_3/flux3_image_generate.md> (2026-10-30)
    fn trusted_polling_url(api_base: &str, url: &str) -> bool {
        crate::providers::model3d::fal::same_origin_url(api_base, Some(url)).is_some()
            || reqwest::Url::parse(url).is_ok_and(|u| {
                u.scheme() == "https" && u.host_str().is_some_and(|h| h == "bfl.ai" || h.ends_with(".bfl.ai"))
            })
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.bfl.ai")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("bfl".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// The native model id is the endpoint path segment (e.g. `flux-pro-1.1`).
    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("bfl/").unwrap_or(model_id)
    }

    async fn poll_result(&self, polling_url: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        // As long as litegen waits for this call (the vendor timeout,
        // docs/gateway-settings.md); 240 s when called directly.
        let started = std::time::Instant::now();
        let mut budget = crate::providers::PollBudget::new(std::time::Duration::from_secs(240), std::time::Duration::from_secs(2));
        while budget.next().await {
            let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(polling_url))?;
            let resp = match crate::providers::inject_trace_headers(builder).send().await {
                Ok(resp) => resp,
                // The job is BFL's now; a dropped poll is asked again.
                Err(_) => continue,
            };
            // "A failed task can come back as HTTP `503` with a normal JSON
            // body. Read `status` from the body before treating a `503` as a
            // retryable outage": a 503 naming a status BFL documents is the
            // job's answer; any other 503 (a gateway's own JSON) is the outage.
            // @see <https://docs.bfl.ml/flux_3/flux3_image_generate.md> (2026-11-03)
            let data = if resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                match serde_json::from_str::<Value>(&resp.text().await.unwrap_or_default()) {
                    Ok(data) if documented_status(&data) => data,
                    _ => continue,
                }
            } else {
                // A 5xx or 429 is asked again; any other error status is the answer.
                match crate::providers::video::read_poll_json(resp, "bfl").await {
                    Ok(data) => data,
                    Err(ProviderError::RequestFailed { retryable: true, .. }) => continue,
                    Err(e) => return Err(e),
                }
            };
            match data["status"].as_str() {
                Some("Ready") => return Ok(data),
                Some(running) if RUNNING.contains(&running) => continue,
                Some(failed) if ENDINGS.contains(&failed) => {
                    return Err(ProviderError::RequestFailed {
                        message: format!("BFL generation failed: {failed}"),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    });
                }
                other => return Err(super::unknown_state("bfl", other)),
            }
        }
        Err(ProviderError::Timeout { timeout_ms: budget.total_ms(started) })
    }

    /// `result.sample` comes from "region-specific delivery URLs under
    /// `delivery.*.bfl.ai`" and is fetched without the key.
    /// @see <https://docs.bfl.ml/api_integration/integration_guidelines.md> (2026-10-30)
    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        super::fetch_result(&self.asset_client, url, self.public_assets_only, "bfl image").await
    }
}

impl Default for BflProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl BflProvider {
    /// A finished job's result, as `generate` returns it: its image
    /// downloaded, with the vendor's ids.
    async fn finish(&self, id: &str, result: Value) -> Result<GenerationOutput, ProviderError> {
        let sample = result["result"]["sample"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "BFL result missing result.sample".to_string(),
            status_code: None,
            provider_error: Some(result.clone()),
            retryable: false,
        })?;

        let bytes = self.fetch_image_bytes(sample).await?;
        // png unless the caller named jpeg or webp ("Can be 'jpeg', 'png', or
        // 'webp'"): label the bytes as what they are.
        // @see <https://api.bfl.ai/openapi.json> (`output_format`, 2026-11-09)
        let content_type = crate::proxy::materializer::sniff_image_mime(&bytes).unwrap_or("image/png");
        let mut metadata = HashMap::new();
        metadata.insert("id".to_string(), Value::String(id.to_string()));
        metadata.insert("url".to_string(), Value::String(sample.to_string()));

        Ok(GenerationOutput {
            data: bytes,
            content_type: content_type.to_string(),
            metadata,
            more: Vec::new(),
        })
    }
}

#[async_trait]
impl ImageProvider for BflProvider {
    fn name(&self) -> &str {
        "bfl"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
        }
        self.client = Self::api_client(&config.extra_headers);
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
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/v1/{}", self.api_base(), crate::providers::wire_model(self.config.as_ref(), &model.id, native));

        if native == "flux-3-image" && !materialized.refs.is_empty() {
            let pixels = extras.size.as_deref().and_then(|s| s.split_once('x')).and_then(|(w, h)| Some(w.parse::<u64>().ok()? * h.parse::<u64>().ok()?));
            if let Some(pixels) = pixels.filter(|p| *p > FLUX_3_IMAGE_EDIT_MAX_PIXELS) {
                return Err(ProviderError::InvalidRequest(format!(
                    "FLUX 3 Image makes at most {FLUX_3_IMAGE_EDIT_MAX_PIXELS} pixels with reference images; the size asks for {pixels}"
                )));
            }
        }

        // A layout rides in the prompt, on the rows that declare one (FLUX 3).
        let prompt = match extras.layout.as_deref() {
            Some(layout) if model.params.contains_key("layout") => flux_3_layout_prompt(&base.prompt, layout),
            _ => base.prompt.clone(),
        };
        let mut body = json!({ "prompt": prompt });
        // Width/height from size; else aspect_ratio for ultra/kontext models.
        if let Some(size) = extras.size.as_deref() {
            if let Some((w, h)) = size.split_once('x') {
                if let (Ok(w), Ok(h)) = (w.parse::<u64>(), h.parse::<u64>()) {
                    body["width"] = Value::Number(w.into());
                    body["height"] = Value::Number(h.into());
                }
            }
        } else if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        if let Some(tier) = extras.resolution.as_deref() {
            let r = bfl_resolution(tier).ok_or_else(|| ProviderError::InvalidRequest(format!("{}: BFL has no resolution {tier}", model.id)))?;
            body["resolution"] = Value::String(r.into());
        }
        // flux-dev: "steps … Number of steps for the image generation process"
        // and "guidance … Guidance scale for image generation"; only rows whose
        // endpoint takes them declare `steps` / `guidance_scale`.
        // @see https://docs.bfl.ml/api-reference/models/generate-an-image-with-flux1-%5Bdev%5D (2026-11-03)
        if let Some(steps) = extras.steps {
            body["steps"] = Value::Number(steps.into());
        }
        if let Some(guidance) = extras.guidance_scale.and_then(serde_json::Number::from_f64) {
            body["guidance"] = Value::Number(guidance);
        }
        // The asked container (PixelGenie L9); png when none is asked — to an
        // endpoint that takes one, which its row declares. FLUX 3 Image has no
        // `output_format` and refuses a field it lacks
        // (`additionalProperties: false`).
        // @see https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3 (2026-11-03)
        if model.params.contains_key("output_format") {
            body["output_format"] = Value::String(extras.output_format.clone().unwrap_or_else(|| "png".to_string()));
        }

        // Reference images: a Redux image is `image_prompt`; the rest are
        // `input_image`, `input_image_2` … in the order given ("Reference
        // multiple images simultaneously - up to 8 via API"; the row's
        // max_total says how many its endpoint takes).
        // @see https://docs.bfl.ml/flux_2/flux2_image_editing.md (2026-11-03)
        // FLUX 3 takes them all, in order, as one `images` list.
        let mut inputs = 0usize;
        let mut images = Vec::new();
        for r in &materialized.refs {
            let image = match &r.form {
                MaterializedRefForm::Base64(b64) => b64.clone(),
                MaterializedRefForm::Url(u) => u.clone(),
                _ => continue,
            };
            if matches!(r.role.as_str(), "redux" | "image_prompt") {
                body["image_prompt"] = Value::String(image);
            } else if takes_an_images_list(native) {
                images.push(Value::String(image));
            } else {
                inputs += 1;
                let field = if inputs == 1 { "input_image".to_string() } else { format!("input_image_{inputs}") };
                body[field.as_str()] = Value::String(image);
            }
        }
        if !images.is_empty() {
            body["images"] = Value::Array(images);
        }

        if let Some(obj) = body.as_object_mut() {
            let owned = |k: &str| OWNED_FIELDS.contains(&k) || k.starts_with("input_image");
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
                obj.insert(k.clone(), v.clone());
            }
        }

        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("BFL request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // Only a connection that never opened is safe to retry: a
                // request that timed out may have been accepted and billed.
                retryable: e.is_connect(),
            })?;

        let status = resp.status();
        let retry_after = crate::providers::retry_after_ms(resp.headers());
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let submit: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let said = if submit.is_null() { text.chars().take(200).collect() } else { submit.to_string() };
            if let Some(e) = crate::providers::rate_limited("bfl", status, retry_after, &format!("BFL API error: {said}"), Some(&submit)) {
                return Err(e);
            }
            return Err(ProviderError::RequestFailed {
                message: format!("BFL API error: {said}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(submit),
                // Turned away, not run.
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }
        let submit: Value = serde_json::from_str(&text).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse BFL response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        let id = submit["id"].as_str().unwrap_or_default().to_string();
        // Prefer the returned polling_url; fall back to the documented path.
        let polling_url = submit["polling_url"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("{}/v1/get_result?id={id}", self.api_base()));
        // Accepted: from here on a failure must not re-send the job (L1).
        // The polling URL names the region holding it, for a later look.
        extras.vendor_job.record_resumable(id.clone(), json!({ "polling_url": polling_url }));
        // The poll carries the key, so it goes only to BFL.
        if !Self::trusted_polling_url(self.api_base(), &polling_url) {
            return Err(ProviderError::RequestFailed {
                message: format!("BFL named a polling URL off its own hosts: {polling_url}"),
                status_code: None,
                provider_error: Some(submit),
                retryable: false,
            });
        }

        let result = self.poll_result(&polling_url).await?;
        self.finish(&id, result).await
    }

    fn resumable(&self, _model: &ModelSchema) -> bool {
        true
    }

    /// One look at an accepted job, read as `generate`'s own poll loop reads it.
    async fn poll_job(&self, _model: &str, job: &str, resume: Option<&Value>) -> Result<crate::providers::ImageJobPoll, ProviderError> {
        let url = resume
            .and_then(|r| r["polling_url"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{}/v1/get_result?id={job}", self.api_base()));
        // The poll carries the key, so it goes only to BFL.
        if !Self::trusted_polling_url(self.api_base(), &url) {
            return Ok(crate::providers::ImageJobPoll::Failed(format!("BFL named a polling URL off its own hosts: {url}")));
        }
        match super::poll_once(self.poll_result(&url)).await {
            Ok(Some(result)) => self.finish(job, result).await.map(crate::providers::ImageJobPoll::Done),
            Ok(None) => Ok(crate::providers::ImageJobPoll::Running),
            Err(e) => super::job_failed(e),
        }
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(
            model.pricing.base_cost_usd,
            0.0,
            CostSource::Estimated,
            Some(json!({ "model": model.id })),
        ))
    }

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "BFL provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult {
            healthy: true,
            message: "BFL provider configured".into(),
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> BflProvider {
        let mut p = BflProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: "bfl-key".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("bfl-key".to_string());
        p.configure(cfg);
        p.public_assets_only = false;
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(),
            model: model.to_string(),
            n: 1,
            negative_prompt: None,
            seed: Some(7),
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: Some("1024x1024".to_string()),
            resolution: None, aspect_ratio: None,
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            edit: None, views: Vec::new(),
            extra: None,
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        }
    }

    #[tokio::test]
    async fn generates_flux_pro_via_polling_url() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/sample.png", image_server.uri());
        let polling_url = format!("{}/v1/get_result", server.uri());

        Mock::given(method("POST"))
            .and(path("/v1/flux-pro-1.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "bfl-1", "polling_url": format!("{}?id=bfl-1", polling_url)
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/get_result"))
            .and(query_param("id", "bfl-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "Ready", "result": { "sample": image_url.clone() }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"FLUXPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("bfl/flux-pro-1.1");
        let base = make_base("a cyberpunk fox", "bfl/flux-pro-1.1");
        let mut extras = make_extras();
        extras.output_format = Some("webp".into());
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        // The vendor accepted the job before the poll: the router must never re-send it.
        assert_eq!(extras.vendor_job.get().as_deref(), Some("bfl-1"));
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"FLUXPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("x-key").unwrap(), "bfl-key");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["prompt"], "a cyberpunk fox");
        assert_eq!(body["width"], 1024);
        assert_eq!(body["output_format"], "webp", "PixelGenie L9: the asked container");
        assert_eq!(body["height"], 1024);
        assert_eq!(body["seed"], 7);
    }

    // ─── BFL audit, 2026-10-30 ──────────────────────────────────────────────

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    /// F2: `extra` adds BFL's own knobs but never replaces what litegen sends.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": "stop" })))
            .mount(&server)
            .await;
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "prompt": "other", "width": 64, "seed": 9, "output_format": "jpeg", "input_image": "x", "safety_tolerance": 2
        }));
        let _ = make_provider(&server.uri()).generate(&ref_schema("bfl/flux-pro-1.1"), &make_base("a fern", "bfl/flux-pro-1.1"), &extras, &no_refs()).await;
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["prompt"], "a fern");
        assert_eq!(body["width"], 1024);
        assert_eq!(body["seed"], 7);
        assert_eq!(body["output_format"], "png");
        assert!(body.get("input_image").is_none(), "{body}");
        assert_eq!(body["safety_tolerance"], 2);
    }

    /// F3: "`Pending`, `Reasoning`, `Generating` | In progress | Keep polling";
    /// every other status but `Ready` (`No Card Generated` among them) is an
    /// ending, not a job to poll until the cap.
    /// @see <https://docs.bfl.ml/flux_3/flux3_image_generate.md> (2026-10-30)
    #[tokio::test]
    async fn only_pending_reasoning_or_generating_is_still_running() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "j1", "polling_url": format!("{}/v1/get_result?id=j1", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/get_result"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "j1", "status": "No Card Generated" })))
            .mount(&server)
            .await;
        let started = std::time::Instant::now();
        let err = make_provider(&server.uri())
            .generate(&ref_schema("bfl/flux-pro-1.1"), &make_base("x", "bfl/flux-pro-1.1"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ProviderError::RequestFailed { retryable: false, message, .. } if message.contains("No Card Generated")),
            "{err:?}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "polled past an ending");
    }

    /// F13: the key goes only to BFL: a `polling_url` on another origin is
    /// not polled with it.
    #[tokio::test]
    async fn the_key_goes_only_to_bfl() {
        let elsewhere = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "Pending" })))
            .mount(&elsewhere)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "j1", "polling_url": format!("{}/v1/get_result?id=j1", elsewhere.uri())
            })))
            .mount(&server)
            .await;
        let err = make_provider(&server.uri())
            .generate(&ref_schema("bfl/flux-pro-1.1"), &make_base("x", "bfl/flux-pro-1.1"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
        assert!(elsewhere.received_requests().await.unwrap().is_empty(), "the key went to another host");
        assert!(BflProvider::trusted_polling_url("https://api.bfl.ai", "https://api.eu1.bfl.ai/v1/get_result?id=a"));
        assert!(!BflProvider::trusted_polling_url("https://api.bfl.ai", "https://bfl.ai.example.com/v1/get_result"));
        assert!(!BflProvider::trusted_polling_url("https://api.bfl.ai", "http://api.eu1.bfl.ai/v1/get_result"));
    }

    /// F13: a submit that timed out may have been accepted and billed, so it
    /// is never sent again; a 429 was turned away and may be.
    #[tokio::test]
    async fn a_timed_out_submit_is_never_sent_again_and_a_rate_limited_one_may_be() {
        let slow = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "j" })).set_delay(std::time::Duration::from_secs(5)))
            .mount(&slow)
            .await;
        let mut p = make_provider(&slow.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let schema = ref_schema("bfl/flux-pro-1.1");
        let base = make_base("x", "bfl/flux-pro-1.1");
        let err = p.generate(&schema, &base, &make_extras(), &no_refs()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");

        let limited = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "detail": "Too many active tasks" })))
            .mount(&limited)
            .await;
        let err = make_provider(&limited.uri()).generate(&schema, &base, &make_extras(), &no_refs()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: true, status_code: Some(429), .. }), "{err:?}");
    }

    /// F13: the result is fetched without the key, only from a public address,
    /// and at most 64 MiB.
    #[tokio::test]
    async fn a_result_is_fetched_only_from_a_public_address() {
        let server = MockServer::start().await;
        let cdn = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "j1", "polling_url": format!("{}/v1/get_result?id=j1", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/get_result"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "Ready", "result": { "sample": format!("{}/sample.png", cdn.uri()) }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"\x89PNG\r\n\x1a\n".to_vec())).mount(&cdn).await;
        let mut provider = make_provider(&server.uri());
        provider.public_assets_only = true;
        let err = provider
            .generate(&ref_schema("bfl/flux-pro-1.1"), &make_base("x", "bfl/flux-pro-1.1"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
        assert!(cdn.received_requests().await.unwrap().is_empty(), "a private address was fetched");
    }

    /// `output_format`: "Output format for the generated image. Can be 'jpeg',
    /// 'png', or 'webp'." litegen sends png only when the caller names nothing,
    /// so a caller who asks for jpeg or webp gets bytes labelled as what they
    /// are, not as png.
    /// @see <https://api.bfl.ai/openapi.json> (`FluxPro11Inputs.output_format`, 2026-11-09)
    #[tokio::test]
    async fn a_result_is_labelled_with_the_type_its_bytes_are() {
        for (bytes, want) in [
            (b"\xFF\xD8\xFF\xE0\x00\x10JFIF".to_vec(), "image/jpeg"),
            (b"RIFF\x24\x00\x00\x00WEBPVP8 ".to_vec(), "image/webp"),
            (b"\x89PNG\r\n\x1a\n\x00\x00".to_vec(), "image/png"),
        ] {
            let server = MockServer::start().await;
            let cdn = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "id": "j1", "polling_url": format!("{}/v1/get_result?id=j1", server.uri())
                })))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/v1/get_result"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "status": "Ready", "result": { "sample": format!("{}/sample", cdn.uri()) }
                })))
                .mount(&server)
                .await;
            Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(bytes)).mount(&cdn).await;
            let out = make_provider(&server.uri())
                .generate(&ref_schema("bfl/flux-pro-1.1"), &make_base("x", "bfl/flux-pro-1.1"), &make_extras(), &no_refs())
                .await
                .expect("generate");
            assert_eq!(out.content_type, want);
        }
    }

    /// A job still running when litegen stops waiting: the adapter stops
    /// polling at litegen's deadline for the call, not its own 4 minutes.
    #[tokio::test]
    async fn an_endless_pending_ends_at_litegens_deadline() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "bfl-slow", "polling_url": format!("{}/v1/get_result?id=bfl-slow", server.uri()) })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "Pending" })))
            .mount(&server)
            .await;
        let provider = make_provider(&server.uri());
        let extras = make_extras();
        let started = std::time::Instant::now();
        // Bounded, so an adapter that ignores the deadline fails here rather than polling for minutes.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            crate::providers::with_deadline(
                std::time::Instant::now() + std::time::Duration::from_secs(3),
                provider.generate(&ref_schema("bfl/flux-pro-1.1"), &make_base("slow", "bfl/flux-pro-1.1"), &extras, &no_refs()),
            ),
        )
        .await
        .expect("the adapter polled past litegen's deadline");
        assert!(matches!(result, Err(ProviderError::Timeout { .. })), "{:?}", result.err());
        let took = started.elapsed();
        assert!(took >= std::time::Duration::from_secs(2) && took < std::time::Duration::from_secs(6), "{took:?}");
        assert_eq!(extras.vendor_job.get().as_deref(), Some("bfl-slow"), "the accepted job is reported");
    }

    // ─── Vendor feature audit, 2026-11-03 ───────────────────────────────────

    fn refs(roles: &[&str]) -> MaterializedRequest {
        MaterializedRequest {
            refs: roles
                .iter()
                .enumerate()
                .map(|(i, role)| crate::proxy::materializer::MaterializedRef {
                    role: (*role).into(),
                    form: MaterializedRefForm::Base64(format!("IMG{i}")),
                    time_s: None,
                })
                .collect(),
            cleanup: Cleanup::empty(),
        }
    }

    async fn sent_body(id: &str, extras: &ImageExtras, materialized: &MaterializedRequest) -> Value {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": "stop" })))
            .mount(&server)
            .await;
        let _ = make_provider(&server.uri()).generate(&ref_schema(id), &make_base("x", id), extras, materialized).await;
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap()
    }

    /// flux-dev: "steps … Number of steps for the image generation process"
    /// and "guidance … Guidance scale for image generation" carry the canonical
    /// `steps` and `guidance_scale`; `extra` cannot replace either.
    /// @see <https://docs.bfl.ml/api-reference/models/generate-an-image-with-flux1-%5Bdev%5D> (2026-11-03)
    #[tokio::test]
    async fn flux_dev_sends_steps_and_guidance_as_its_own_fields() {
        let mut extras = make_extras();
        extras.steps = Some(30);
        extras.guidance_scale = Some(4.5);
        extras.extra = Some(json!({ "steps": 2, "guidance": 1.5 }));
        let body = sent_body("bfl/flux-dev", &extras, &no_refs()).await;
        assert_eq!((body["steps"].as_i64(), body["guidance"].as_f64()), (Some(30), Some(4.5)), "{body}");
        assert!(body.get("guidance_scale").is_none(), "BFL's field is `guidance`: {body}");
    }

    /// FLUX.2: "Reference multiple images simultaneously - up to 8 via API" as
    /// `input_image`, `input_image_2` … `input_image_8`: each reference goes to
    /// its own field, in the order given.
    /// @see <https://docs.bfl.ml/api-reference/models/generate-or-edit-an-image-with-flux2-%5Bpro%5D> (2026-11-03)
    #[tokio::test]
    async fn every_reference_goes_to_its_own_input_image() {
        let body = sent_body("bfl/flux-2-pro", &make_extras(), &refs(&["init", "init", "init"])).await;
        assert_eq!(body["input_image"], "IMG0", "{body}");
        assert_eq!(body["input_image_2"], "IMG1", "{body}");
        assert_eq!(body["input_image_3"], "IMG2", "{body}");
        assert!(body.get("input_image_4").is_none(), "{body}");
        let one = sent_body("bfl/flux-2-pro", &make_extras(), &refs(&["init"])).await;
        assert!(one.get("input_image_2").is_none(), "{one}");
        // `extra` cannot add or replace one.
        let mut extras = make_extras();
        extras.extra = Some(json!({ "input_image_2": "other", "input_image_8": "other" }));
        let body = sent_body("bfl/flux-2-pro", &extras, &refs(&["init"])).await;
        assert!(body.get("input_image_2").is_none() && body.get("input_image_8").is_none(), "{body}");
    }

    /// Ultra: "image_prompt … Optional image to remix in base64 format".
    /// @see <https://docs.bfl.ml/api-reference/models/generate-an-image-with-flux11-%5Bpro%5D-ultra-mode> (2026-11-03)
    #[tokio::test]
    async fn an_ultra_image_prompt_goes_as_image_prompt() {
        let mut extras = make_extras();
        extras.size = None;
        extras.extra = Some(json!({ "image_prompt_strength": 0.4 }));
        let body = sent_body("bfl/flux-pro-1.1-ultra", &extras, &refs(&["redux"])).await;
        assert_eq!(body["image_prompt"], "IMG0", "{body}");
        assert_eq!(body["image_prompt_strength"], 0.4, "{body}");
        assert!(body.get("input_image").is_none(), "{body}");
    }

    // ─── FLUX 3 Image and FLUX 3 Image Klein, 2026-11-03 ────────────────────

    /// What the validator hands a FLUX 3 row: no size (FLUX 3 Image declares
    /// none) and an aspect ratio.
    fn flux_3_extras(extra: Value) -> ImageExtras {
        let mut extras = make_extras();
        extras.size = None;
        extras.aspect_ratio = Some("16:9".into());
        extras.extra = Some(extra);
        extras
    }

    /// FLUX 3 Image: "images … Optional reference image(s) the prompt edits or
    /// draws from; each is an http(s) URL or base64, one to 10 total"; Klein:
    /// "Same request as FLUX 3 Image: `prompt`, `images` …", "The first is the
    /// image being edited". Every reference goes in that one list, in the order
    /// given, none as an `input_image`, and `extra` can replace none of it.
    /// @see <https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3> (2026-11-03)
    /// @see <https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3-klein> (2026-11-03)
    #[tokio::test]
    async fn flux_3_rows_send_every_reference_in_one_images_list() {
        let extras = flux_3_extras(json!({ "images": ["other"], "input_image": "other" }));
        for id in ["bfl/flux-3-image", "bfl/flux-3-image-klein"] {
            let body = sent_body(id, &extras, &refs(&["init", "init", "init"])).await;
            assert_eq!(body["images"], json!(["IMG0", "IMG1", "IMG2"]), "{id}: {body}");
            assert!(body.as_object().unwrap().keys().all(|k| !k.starts_with("input_image")), "{id}: {body}");
        }
        let none = sent_body("bfl/flux-3-image-klein", &extras, &no_refs()).await;
        assert!(none.get("images").is_none(), "no references, no list: {none}");
    }

    /// FLUX 3 Image refuses any field it lacks (`additionalProperties: false`)
    /// and has no `output_format`, `seed` or `steps`, so it gets exactly its
    /// own fields: the container goes only to a row that declares one (Klein:
    /// "Output image format: `jpeg` (default) or `png`"). The tier passes
    /// through as `resolution`. (The validator drops an undeclared seed.)
    /// @see <https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3> (2026-11-03)
    #[tokio::test]
    async fn output_format_goes_only_to_endpoints_that_take_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": "stop" })))
            .mount(&server)
            .await;
        let mut base = make_base("x", "bfl/flux-3-image");
        base.seed = None;
        let mut extras = flux_3_extras(json!({ "grounding": false }));
        extras.resolution = Some("2K".into());
        let _ = make_provider(&server.uri()).generate(&ref_schema("bfl/flux-3-image"), &base, &extras, &no_refs()).await;
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body, json!({ "prompt": "x", "aspect_ratio": "16:9", "resolution": "2k", "grounding": false }));
        let mut extras = flux_3_extras(json!({}));
        extras.resolution = Some("2K".into());
        let klein = sent_body("bfl/flux-3-image-klein", &extras, &no_refs()).await;
        assert_eq!((klein["output_format"].as_str(), klein["resolution"].as_str()), (Some("png"), Some("2k")), "{klein}");
    }

    /// FLUX 3 Image: "width x height at most 16822272 pixels, and at most
    /// 4194304 with reference images" — an edit asking for more is refused
    /// before any call; without references the same size goes.
    /// @see <https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3> (2026-11-03)
    #[tokio::test]
    async fn a_flux_3_image_edit_larger_than_4_mp_is_refused_before_any_call() {
        let server = MockServer::start().await;
        let mut base = make_base("x", "bfl/flux-3-image");
        base.seed = None;
        let mut extras = flux_3_extras(json!({}));
        extras.size = Some("2560x2048".into());
        let err = make_provider(&server.uri())
            .generate(&ref_schema("bfl/flux-3-image"), &base, &extras, &refs(&["init"]))
            .await
            .unwrap_err();
        assert!(matches!(&err, ProviderError::InvalidRequest(m) if m.contains("4194304")), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing was sent");
        let body = sent_body("bfl/flux-3-image", &extras, &no_refs()).await;
        assert_eq!((body["width"].as_u64(), body["height"].as_u64()), (Some(2560), Some(2048)), "{body}");
    }

    /// "A failed task can come back as HTTP `503` with a normal JSON body. Read
    /// `status` from the body before treating a `503` as a retryable outage."
    /// The failure ends the job at once; a 503 that names no status, or one
    /// BFL does not document (a gateway's own JSON), is still an outage, asked
    /// again.
    /// @see <https://docs.bfl.ml/flux_3/flux3_image_generate.md> (2026-11-03)
    #[tokio::test]
    async fn a_failure_answered_with_a_503_ends_the_job() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "j1", "polling_url": format!("{}/v1/get_result?id=j1", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/get_result"))
            .respond_with(ResponseTemplate::new(503).set_body_string("<html>busy</html>"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/get_result"))
            .respond_with(ResponseTemplate::new(503).set_body_json(json!({ "status": "overloaded" })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/get_result"))
            .respond_with(ResponseTemplate::new(503).set_body_json(json!({ "id": "j1", "status": "Error", "details": "boom" })))
            .mount(&server)
            .await;
        let provider = make_provider(&server.uri());
        let started = std::time::Instant::now();
        // Bounded, so an adapter that retries the failure fails here rather than polling for minutes.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            crate::providers::with_deadline(
                std::time::Instant::now() + std::time::Duration::from_secs(10),
                provider.generate(&ref_schema("bfl/flux-pro-1.1"), &make_base("x", "bfl/flux-pro-1.1"), &make_extras(), &no_refs()),
            ),
        )
        .await
        .expect("the adapter polled past litegen's deadline");
        let err = result.unwrap_err();
        assert!(matches!(&err, ProviderError::RequestFailed { retryable: false, message, .. } if message.contains("Error")), "{err:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(8), "polled past an ending");
        let polls = server.received_requests().await.unwrap().iter().filter(|r| r.method.as_str() == "GET").count();
        assert_eq!(polls, 3, "both outages were asked again, the failure was not");
    }

    /// FLUX 3's tier: "`768sq`, `1k` (default), `1.5k`, `2k`, or `4k`", and
    /// FLUX 3 Image's `match_input` — litegen's `0.75K` … `4K`. Every tier a
    /// BFL row declares maps; nothing else does.
    /// @see <https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3> (2026-11-03)
    #[test]
    fn every_declared_resolution_is_one_of_bfls() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let reg = crate::capabilities::CapabilityRegistry::from_dir(&dir).expect("load");
        let mut seen = 0;
        for m in reg.for_provider("bfl") {
            if let Some(crate::capabilities::ParamSpec::String(s)) = m.params.get("resolution") {
                for v in &s.enum_values {
                    seen += 1;
                    assert!(matches!(bfl_resolution(v), Some("768sq" | "1k" | "1.5k" | "2k" | "4k" | "match_input")), "{}: {v}", m.id);
                }
            }
        }
        assert!(seen > 0, "no BFL row declares a resolution");
        assert_eq!((bfl_resolution("0.5K"), bfl_resolution("2k")), (None, None));
    }

    /// The tier goes as BFL's `resolution`, and `extra` cannot replace it.
    #[tokio::test]
    async fn the_tier_goes_as_bfls_resolution_and_extra_cannot_replace_it() {
        let mut extras = flux_3_extras(json!({ "resolution": "4k" }));
        extras.resolution = Some("0.75K".into());
        let body = sent_body("bfl/flux-3-image-klein", &extras, &no_refs()).await;
        assert_eq!(body["resolution"], "768sq", "{body}");
    }

    /// litegen's layout as FLUX 3's boxes: "Write a description using `<id>`
    /// to name each element, then append a JSON array of rows" — `{id, bbox,
    /// desc}`, "Every box is `[top, left, bottom, right]` in integers from 0 to
    /// 1000, measured from the top-left corner"; a text box quotes its words
    /// ("`text reading "Sauna"`").
    /// @see <https://docs.bfl.ml/flux_3/flux3_image_bounding_boxes.md> (2026-11-03)
    #[tokio::test]
    async fn a_layout_goes_as_flux_3_boxes_in_the_prompt() {
        let layout: Vec<crate::types::LayoutItem> = serde_json::from_value(json!([
            { "box": { "x": 0.15, "y": 0.1, "w": 0.7, "h": 0.5 }, "kind": "object", "prompt": "a red fox" },
            { "box": { "x": 0.25, "y": 0.8, "w": 0.5, "h": 0.1 }, "kind": "text", "text": "OPEN LATE" },
        ]))
        .unwrap();
        let mut extras = flux_3_extras(json!({}));
        extras.layout = Some(layout);
        let body = sent_body("bfl/flux-3-image-klein", &extras, &no_refs()).await;
        let prompt = body["prompt"].as_str().unwrap();
        let (caption, rows) = prompt.split_once(" [").map(|(c, r)| (c.to_string(), format!("[{r}"))).expect(prompt);
        assert!(caption.starts_with("x "), "{caption}");
        assert!(caption.contains("a red fox <object_1>") && caption.contains("text reading \"OPEN LATE\" <text_1>"), "{caption}");
        let rows: Value = serde_json::from_str(&rows).expect(prompt);
        assert_eq!(
            rows,
            json!([
                { "id": "object_1", "bbox": [100, 150, 600, 850], "desc": "a red fox" },
                { "id": "text_1", "bbox": [800, 250, 900, 750], "desc": "text reading \"OPEN LATE\"" },
            ])
        );
        // Without a layout the prompt goes as written.
        assert_eq!(sent_body("bfl/flux-3-image-klein", &flux_3_extras(json!({})), &no_refs()).await["prompt"], "x");
    }
}
