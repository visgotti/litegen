use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::MaterializedRequest;
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Recraft image generation provider (raster + vector).
///
/// Synchronous, OpenAI-SDK-compatible: `POST /v1/images/generations` returns
/// `{created, data: [{url | b64_json}]}`. We request `b64_json` so the bytes
/// come back inline (Recraft result URLs expire). Bearer auth. Image-only.
///
/// @see <https://www.recraft.ai/docs/api-reference/getting-started.md> — base + auth
///   Verbatim: "'https://external.api.recraft.ai/v1'"
///   Verbatim: "Authorization: Bearer RECRAFT_API_TOKEN"
/// @see <https://www.recraft.ai/docs/api-reference/endpoints.md>
///   Verbatim: "POST https://external.api.recraft.ai/v1/images/generations"
pub struct RecraftProvider {
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

impl RecraftProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(180), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Self::api_client(&Default::default()),
            asset_client: crate::util::ssrf::public_only_download_client(
                std::time::Duration::from_secs(120),
                std::time::Duration::from_secs(30),
            ),
            public_assets_only: true,
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://external.api.recraft.ai/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("recraft".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("recraft/").unwrap_or(model_id)
    }

    /// Recraft answers in the same call, so only a connection that never
    /// opened may be sent again: one that timed out may have run and billed.
    fn send_failed(e: reqwest::Error) -> ProviderError {
        ProviderError::RequestFailed {
            message: format!("Recraft request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_connect(),
        }
    }

    /// Recraft's answer: its JSON on success, else an error in Recraft's own
    /// words ("some errors return plain text, others JSON. Read the body as
    /// text first"), which may be sent again only when Recraft turned the
    /// call away (429, 5xx).
    /// @see <https://www.recraft.ai/docs/api-reference/error-handling.md> (2026-10-30)
    async fn read_reply(resp: reqwest::Response) -> Result<Value, ProviderError> {
        let status = resp.status();
        let retry_after = crate::providers::retry_after_ms(resp.headers());
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let data: Option<Value> = serde_json::from_str(&text).ok();
            let said = data.as_ref().map(Value::to_string).unwrap_or_else(|| text.chars().take(200).collect());
            if let Some(e) = crate::providers::rate_limited("recraft", status, retry_after, &format!("Recraft API error: {said}"), data.as_ref()) {
                return Err(e);
            }
            return Err(ProviderError::RequestFailed {
                message: format!("Recraft API error: {said}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data.unwrap_or(Value::String(text))),
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }
        serde_json::from_str(&text).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Recraft response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })
    }

    /// A result named by URL: fetched without the key, from a public address
    /// only, at most 64 MiB, and never an error page taken for an image.
    async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        super::fetch_result(&self.asset_client, url, self.public_assets_only, "recraft image").await
    }
}

/// What litegen itself sends to `POST /images/generations`; `extra` cannot
/// replace it (`extra.model` would run another model at this row's price).
const OWNED_FIELDS: &[&str] = &[
    "prompt", "model", "n", "response_format", "image_format", "size", "style", "negative_prompt", "random_seed", "text_layout",
];

impl RecraftProvider {
    /// Replace background (3D doc §4, task background_swap): the image
    /// uploaded as `image` with the prompt, raster (`recraftv3`), PNG unless
    /// webp is asked, as base64; it answers a `data` array.
    ///
    /// @see <https://www.recraft.ai/docs/api-reference/tools/background.md> — `image`, `prompt` (required),
    ///   `model`, `negative_prompt`, `response_format`, `image_format`, and the `{created, credits, data}` response
    async fn replace_background(&self, base: &BaseGenerationRequest, extras: &ImageExtras, materialized: &MaterializedRequest) -> Result<GenerationOutput, ProviderError> {
        use crate::proxy::materializer::MaterializedRefForm;
        let creds = self.creds()?;
        let (bytes, content_type) = materialized
            .refs
            .iter()
            .find(|r| r.role == "init")
            .and_then(|r| match &r.form {
                MaterializedRefForm::MultipartField { bytes, content_type, .. } => Some((bytes.to_vec(), content_type.clone())),
                _ => None,
            })
            .ok_or_else(|| ProviderError::InvalidRequest("a background swap needs an init image".into()))?;
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name("image")
            .mime_str(&content_type)
            .map_err(|e| ProviderError::InvalidRequest(format!("init image type {content_type}: {e}")))?;
        let mut form = reqwest::multipart::Form::new()
            .part("image", part)
            .text("prompt", base.prompt.clone())
            .text("model", "recraftv3")
            .text("response_format", "b64_json")
            .text("image_format", extras.output_format.clone().unwrap_or_else(|| "png".into()));
        if let Some(np) = base.negative_prompt.as_deref() {
            form = form.text("negative_prompt", np.to_string());
        }
        let owned = |k: &str| ["image", "prompt", "model", "negative_prompt", "response_format", "image_format"].contains(&k);
        for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
            form = form.text(k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()));
        }
        let url = format!("{}/images/replaceBackground", self.api_base());
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.post(&url).multipart(form))?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(Self::send_failed)?;
        let data = Self::read_reply(resp).await?;
        let failed = |message: String| ProviderError::RequestFailed { message, status_code: None, provider_error: None, retryable: false };
        let first = &data["data"][0];
        let bytes = if let Some(b64) = first["b64_json"].as_str() {
            B64.decode(b64).map_err(|e| failed(format!("Failed to decode Recraft b64_json: {e}")))?
        } else if let Some(at) = first["url"].as_str() {
            self.fetch_url(at).await?
        } else {
            return Err(ProviderError::RequestFailed { message: "Recraft response has no image".into(), status_code: None, provider_error: Some(data), retryable: false });
        };
        Ok(GenerationOutput { data: bytes, content_type: "image/png".into(), metadata: HashMap::new(), more: Vec::new() })
    }

    /// Remove background (PixelGenie L8): the image uploaded as `file`, the
    /// result asked as base64 in `image_format` png (Recraft's default is
    /// lossless WebP) unless webp is asked. It answers one `image`, not a
    /// `data` array.
    ///
    /// @see <https://www.recraft.ai/docs/api-reference/tools/remove-background.md> — `file` (multipart),
    ///   `response_format`, `image_format` webp | png, and the `{created, credits, image}` response
    async fn remove_background(&self, extras: &ImageExtras, materialized: &MaterializedRequest) -> Result<GenerationOutput, ProviderError> {
        use crate::proxy::materializer::MaterializedRefForm;
        let creds = self.creds()?;
        let file = materialized.refs.iter().find(|r| r.role == "init").and_then(|r| match &r.form {
            MaterializedRefForm::MultipartField { bytes, content_type, .. } => Some((bytes.to_vec(), content_type.clone())),
            _ => None,
        });
        let (bytes, content_type) = file.ok_or_else(|| ProviderError::InvalidRequest("background removal needs an init image".into()))?;
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name("image")
            .mime_str(&content_type)
            .map_err(|e| ProviderError::InvalidRequest(format!("init image type {content_type}: {e}")))?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("response_format", "b64_json")
            .text("image_format", extras.output_format.clone().unwrap_or_else(|| "png".into()));
        let url = format!("{}/images/removeBackground", self.api_base());
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.post(&url).multipart(form))?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(Self::send_failed)?;
        let data = Self::read_reply(resp).await?;
        let failed = |message: String| ProviderError::RequestFailed { message, status_code: None, provider_error: None, retryable: false };
        let bytes = if let Some(b64) = data["image"]["b64_json"].as_str() {
            B64.decode(b64).map_err(|e| failed(format!("Failed to decode Recraft b64_json: {e}")))?
        } else if let Some(at) = data["image"]["url"].as_str() {
            self.fetch_url(at).await?
        } else {
            return Err(ProviderError::RequestFailed {
                message: "Recraft response has no image".into(),
                status_code: None,
                provider_error: Some(data),
                retryable: false,
            });
        };
        Ok(GenerationOutput { data: bytes, content_type: "image/png".into(), metadata: HashMap::new(), more: Vec::new() })
    }
}

impl Default for RecraftProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for RecraftProvider {
    fn name(&self) -> &str {
        "recraft"
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

    /// @see <https://www.recraft.ai/docs/api-reference/endpoints.md> — request fields
    ///   (`prompt`, `model`, `style`/`style_id`, `n`, `size`, `negative_prompt`,
    ///   `response_format`, `controls`) and the `data[].url`/`data[].b64_json` response.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        _materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        if model.capabilities.background_removal {
            return self.remove_background(extras, _materialized).await;
        }
        if model.task == crate::capabilities::ModelTask::BackgroundSwap {
            return self.replace_background(base, extras, _materialized).await;
        }
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/images/generations", self.api_base());
        let is_vector = native.contains("vector");

        let mut body = json!({
            "prompt": base.prompt,
            "model": crate::providers::wire_model(self.config.as_ref(), &model.id, native),
            "n": base.n.max(1),
            "response_format": "b64_json",
        });
        // A raster image comes back as webp unless asked ("`image_format` |
        // … default is `webp`"); vector models answer SVG and take no format.
        // @see <https://www.recraft.ai/docs/api-reference/endpoints.md> (2026-10-30)
        if !is_vector {
            body["image_format"] = Value::String("png".into());
        }
        // `size` takes an exact `WxH` or a `w:h` ratio ("Works with every
        // model. Prefer it."); an exact size the caller sent wins.
        // @see <https://www.recraft.ai/docs/api-reference/appendix.md> ("Image sizes", 2026-11-03)
        if let Some(size) = extras.size.as_deref().or(extras.aspect_ratio.as_deref()) {
            body["size"] = Value::String(size.to_string());
        }
        // Recraft only accepts `style` (and `style_id`) on V2/V3 models. V4/V4.1
        // models reject it with a 400; for those, style selection is implicit in
        // the model id (e.g. the `_vector` suffix). So only forward style for
        // recraftv2/recraftv3* native ids.
        let supports_style = native.starts_with("recraftv2") || native.starts_with("recraftv3");
        if supports_style {
            if let Some(style) = extras.style.as_deref() {
                body["style"] = Value::String(style.to_string());
            }
        }
        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }
        // Recraft's seed parameter is `random_seed`, supported on all models.
        // @see <https://www.recraft.ai/docs/api-reference/endpoints.md>
        if let Some(seed) = base.seed {
            body["random_seed"] = Value::Number(seed.into());
        }
        if let Some(obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
                obj.insert(k.clone(), v.clone());
            }
        }
        // PixelGenie L16: a palette as preferred colours, inside whatever
        // `controls` `extra` sent; a hint, never forced.
        // @see <https://www.recraft.ai/docs/api-reference/endpoints.md> — Controls, `colors`
        if let Some(palette) = extras.palette.as_ref() {
            let colours: Vec<Value> = palette
                .iter()
                .filter_map(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok())
                .map(|v| json!({ "rgb": [(v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff] }))
                .collect();
            if !body["controls"].is_object() {
                body["controls"] = json!({});
            }
            body["controls"]["colors"] = Value::Array(colours);
        }
        // 3D doc §4: the layout's text boxes as Recraft's `text_layout`, one
        // word and four relative points each, clockwise from the top-left.
        // Written after `extra`, which cannot replace it.
        // @see <https://www.recraft.ai/docs/api-reference/endpoints.md> — Text Layout
        if let Some(layout) = extras.layout.as_ref() {
            let words: Vec<Value> = layout
                .iter()
                .filter(|item| item.kind == crate::capabilities::LayoutKind::Text)
                .map(|item| {
                    let b = item.bounds;
                    json!({
                        "text": item.text.clone().unwrap_or_default(),
                        "bbox": [[b.x, b.y], [b.x + b.w, b.y], [b.x + b.w, b.y + b.h], [b.x, b.y + b.h]],
                    })
                })
                .collect();
            body["text_layout"] = Value::Array(words);
        }

        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(Self::send_failed)?;
        let data = Self::read_reply(resp).await?;

        let first = data["data"].get(0).ok_or_else(|| ProviderError::RequestFailed {
            message: "Recraft response missing data[0]".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;

        let bytes = if let Some(b64) = first["b64_json"].as_str() {
            B64.decode(b64).map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to decode Recraft b64_json: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
        } else if let Some(url) = first["url"].as_str() {
            // Fallback if the account forces url responses.
            self.fetch_url(url).await?
        } else {
            return Err(ProviderError::RequestFailed {
                message: "Recraft data[0] has neither b64_json nor url".to_string(),
                status_code: None,
                provider_error: Some(data.clone()),
                retryable: false,
            });
        };

        let mut metadata = HashMap::new();
        metadata.insert("model".to_string(), Value::String(crate::providers::wire_model(self.config.as_ref(), &model.id, native).to_string()));

        Ok(GenerationOutput {
            data: bytes,
            content_type: if is_vector { "image/svg+xml".to_string() } else { "image/png".to_string() },
            metadata,
            more: Vec::new(),
        })
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
                message: "Recraft provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        let creds = match self.creds() {
            Ok(c) => c,
            Err(_) => return HealthCheckResult { healthy: false, message: "No API key".into(), latency_ms: None },
        };
        let builder = match crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.get(format!("{}/users/me", self.api_base())).timeout(std::time::Duration::from_secs(10)),
        ) {
            Ok(b) => b,
            Err(e) => return HealthCheckResult { healthy: false, message: e.to_string(), latency_ms: None },
        };
        match builder.send().await {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Recraft API token valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Recraft returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Recraft health check failed: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> RecraftProvider {
        let mut p = RecraftProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: "rc-token".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("rc-token".to_string());
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
            seed: None,
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
            style: Some("digital_illustration".to_string()),
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

    /// An aspect ratio goes as `size: "w:h"` ("Works with every model.
    /// Prefer it."); an exact `size` the caller also sent wins.
    /// @see <https://www.recraft.ai/docs/api-reference/appendix.md> ("Image sizes"), read 2026-11-03
    #[tokio::test]
    async fn an_aspect_ratio_goes_as_size_and_an_exact_size_wins() {
        for (size, ratio, want) in [(None, Some("16:9"), "16:9"), (Some("1024x1024"), Some("16:9"), "1024x1024")] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/images/generations"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": [{ "b64_json": "iVBORw0KGgo=" }] })))
                .mount(&server)
                .await;
            let extras = ImageExtras {
                size: size.map(str::to_string),
                resolution: None, aspect_ratio: ratio.map(str::to_string),
                style: None,
                ..make_extras()
            };
            let _ = make_provider(&server.uri())
                .generate(&ref_schema("recraft/recraftv4_1"), &make_base("a lighthouse", "recraft/recraftv4_1"), &extras, &MaterializedRequest { refs: vec![], cleanup: crate::proxy::materializer::Cleanup::empty() })
                .await;
            let req = &server.received_requests().await.unwrap()[0];
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(body["size"], want, "{body}");
        }
    }

    #[tokio::test]
    async fn generates_recraftv3_image() {
        let server = MockServer::start().await;
        let fake_png = b"RECRAFTPNG";
        let b64 = B64.encode(fake_png);

        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": 1700000000,
                "data": [{ "b64_json": b64 }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("recraft/recraftv3");
        let base = make_base("a friendly robot mascot", "recraft/recraftv3");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, fake_png);

        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Bearer rc-token");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "recraftv3");
        assert_eq!(body["prompt"], "a friendly robot mascot");
        assert_eq!(body["size"], "1024x1024");
        assert_eq!(body["style"], "digital_illustration");
        assert_eq!(body["response_format"], "b64_json");
    }

    /// Recraft's seed parameter is `random_seed`, not `seed`.
    /// @see <https://www.recraft.ai/docs/api-reference/endpoints.md>
    #[tokio::test]
    async fn seed_is_sent_as_random_seed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "b64_json": B64.encode(b"png") }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("recraft/recraftv3");
        let mut base = make_base("a race car", "recraft/recraftv3");
        base.seed = Some(4242);

        provider
            .generate(
                &schema,
                &base,
                &make_extras(),
                &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() },
            )
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["random_seed"], 4242);
        assert!(body.get("seed").is_none(), "Recraft has no `seed` field");
    }

    fn multipart_field(body: &[u8], name: &str) -> Option<String> {
        let s = String::from_utf8_lossy(body).into_owned();
        let marker = format!("name=\"{name}\"");
        let at = s.find(&marker)? + marker.len();
        let rest = &s[at..];
        let value = &rest[rest.find("\r\n\r\n")? + 4..];
        Some(value[..value.find("\r\n")?].to_string())
    }

    /// Remove background uploads the image as `file` and answers one `image`,
    /// not a `data` array; litegen asks for PNG (Recraft's default is WebP)
    /// as base64.
    /// @see <https://www.recraft.ai/docs/api-reference/tools/remove-background.md>
    #[tokio::test]
    async fn remove_background_uploads_the_file_and_reads_the_one_image() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/images/removeBackground"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": 1, "credits": 10, "image": { "image_id": "i", "b64_json": B64.encode(b"CUTOUT") }
            })))
            .mount(&server)
            .await;
        let materialized = MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                    field_name: "file".to_string(),
                    bytes: bytes::Bytes::from_static(b"src"),
                    content_type: "image/png".to_string(),
                },
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        };
        let extras = ImageExtras { size: None, style: None, ..make_extras() };
        let out = make_provider(&server.uri())
            .generate(&ref_schema("recraft/remove-background"), &make_base("", "recraft/remove-background"), &extras, &materialized)
            .await
            .expect("generate");
        assert_eq!(out.data, b"CUTOUT");
        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Bearer rc-token");
        let body = &received[0].body;
        assert!(String::from_utf8_lossy(body).contains("name=\"file\""), "the file part");
        assert_eq!(multipart_field(body, "response_format").as_deref(), Some("b64_json"));
        assert_eq!(multipart_field(body, "image_format").as_deref(), Some("png"));
        for absent in ["prompt", "model", "n", "size", "style"] {
            assert!(multipart_field(body, absent).is_none(), "{absent} is not a removeBackground field");
        }
    }

    /// Text layout: one word per element, its bbox four relative points
    /// clockwise from the top-left; "supported by Recraft V3 and Recraft V3
    /// Vector models only".
    /// @see <https://www.recraft.ai/docs/api-reference/endpoints.md> — Text Layout
    #[tokio::test]
    async fn a_text_layout_is_sent_as_recrafts_four_point_boxes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "b64_json": B64.encode(b"png") }] })))
            .mount(&server)
            .await;
        let layout: Vec<crate::types::LayoutItem> = serde_json::from_value(json!([
            { "box": { "x": 0.3, "y": 0.45, "w": 0.3, "h": 0.1 }, "kind": "text", "text": "Recraft" },
        ]))
        .unwrap();
        let extras = ImageExtras { layout: Some(layout), extra: Some(json!({ "text_layout": [] })), ..make_extras() };
        make_provider(&server.uri())
            .generate(&ref_schema("recraft/recraftv3"), &make_base("a sign", "recraft/recraftv3"), &extras, &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() })
            .await
            .expect("generate");
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        let bbox = &body["text_layout"][0]["bbox"];
        assert_eq!(body["text_layout"][0]["text"], "Recraft", "{body}");
        let pts: Vec<(f64, f64)> = bbox.as_array().unwrap().iter().map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap())).collect();
        let close = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9;
        for (got, want) in pts.iter().zip([(0.3, 0.45), (0.6, 0.45), (0.6, 0.55), (0.3, 0.55)]) {
            assert!(close(*got, want), "{pts:?}");
        }
    }

    /// Replace background (task background_swap) uploads the image as `image`
    /// with the prompt, asks recraftv3 for PNG as base64, and reads `data[0]`.
    /// @see <https://www.recraft.ai/docs/api-reference/tools/background.md>
    #[tokio::test]
    async fn replace_background_uploads_the_image_and_the_new_backgrounds_prompt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/images/replaceBackground"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "created": 1, "credits": 40, "data": [{ "b64_json": B64.encode(b"SWAPPED") }] })))
            .mount(&server)
            .await;
        let materialized = MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                    field_name: "image".to_string(),
                    bytes: bytes::Bytes::from_static(b"src"),
                    content_type: "image/png".to_string(),
                },
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        };
        let extras = ImageExtras { size: None, style: None, ..make_extras() };
        let out = make_provider(&server.uri())
            .generate(&ref_schema("recraft/replace-background"), &make_base("a beach at dusk", "recraft/replace-background"), &extras, &materialized)
            .await
            .expect("generate");
        assert_eq!(out.data, b"SWAPPED");
        let body = &server.received_requests().await.unwrap()[0].body;
        assert!(String::from_utf8_lossy(body).contains("name=\"image\""), "the image part");
        for (field, want) in [("prompt", "a beach at dusk"), ("model", "recraftv3"), ("response_format", "b64_json"), ("image_format", "png")] {
            assert_eq!(multipart_field(body, field).as_deref(), Some(want), "{field}");
        }
    }

    /// A palette is a hint here (PixelGenie L16): Recraft's `controls.colors`,
    /// "An array of preferable colors", `{rgb: [r, g, b]}` each, at most 10;
    /// other `controls` sent in `extra` are kept.
    /// @see <https://www.recraft.ai/docs/api-reference/endpoints.md> — Controls
    #[tokio::test]
    async fn a_palette_is_sent_as_recrafts_preferred_colours() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "b64_json": B64.encode(b"png") }] })))
            .mount(&server)
            .await;
        let extras = ImageExtras {
            palette: Some(vec!["#ff6432".into(), "#081820".into()]),
            extra: Some(json!({ "controls": { "no_text": true } })),
            ..make_extras()
        };
        make_provider(&server.uri())
            .generate(&ref_schema("recraft/recraftv3"), &make_base("a fox", "recraft/recraftv3"), &extras, &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() })
            .await
            .expect("generate");
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["controls"], json!({ "no_text": true, "colors": [{ "rgb": [255, 100, 50] }, { "rgb": [8, 24, 32] }] }), "{body}");
    }

    // ─── Recraft audit, 2026-10-30 ──────────────────────────────────────────

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: crate::proxy::materializer::Cleanup::empty() }
    }

    async fn refusing_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "code": "stop" })))
            .mount(&server)
            .await;
        server
    }

    /// F1, F5: a raster image is asked for as PNG (Recraft's `image_format`
    /// defaults to webp), a vector one is not, and `extra` never replaces
    /// what litegen sends (`extra.model` on recraftv3 would run a $0.30 model).
    /// @see <https://www.recraft.ai/docs/api-reference/endpoints.md> (2026-10-30)
    #[tokio::test]
    async fn a_raster_image_is_asked_for_as_png_and_extra_cannot_replace_what_litegen_sends() {
        let server = refusing_server().await;
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "model": "recraftv4_1_pro_vector", "response_format": "url", "image_format": "webp", "n": 4, "prompt": "other", "style_id": "abc"
        }));
        let _ = make_provider(&server.uri()).generate(&ref_schema("recraft/recraftv3"), &make_base("a lighthouse", "recraft/recraftv3"), &extras, &no_refs()).await;
        let mut vector = make_extras();
        vector.style = None;
        vector.size = None;
        let _ = make_provider(&server.uri())
            .generate(&ref_schema("recraft/recraftv4_1_vector"), &make_base("a lighthouse", "recraft/recraftv4_1_vector"), &vector, &no_refs())
            .await;
        let bodies: Vec<Value> = server.received_requests().await.unwrap().iter().map(|r| serde_json::from_slice(&r.body).unwrap()).collect();
        let raster = &bodies[0];
        assert_eq!(raster["model"], "recraftv3");
        assert_eq!(raster["response_format"], "b64_json");
        assert_eq!(raster["image_format"], "png");
        assert_eq!(raster["n"], 1);
        assert_eq!(raster["prompt"], "a lighthouse");
        assert_eq!(raster["style_id"], "abc");
        assert!(bodies[1].get("image_format").is_none(), "{}", bodies[1]);
    }

    /// F10: "some errors return plain text, others JSON. Read the body as text
    /// first" — a plain-text 401 keeps Recraft's words, and a 429 or 5xx was
    /// turned away and may be sent again.
    /// @see <https://www.recraft.ai/docs/api-reference/error-handling.md> (2026-10-30)
    #[tokio::test]
    async fn an_error_says_what_recraft_said_in_whatever_form() {
        for (status, body, retryable) in [(401, "request unauthorized", false), (503, "upstream unavailable", true), (429, "Too many requests", true)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status).set_body_string(body).insert_header("content-type", "text/plain"))
                .mount(&server)
                .await;
            let err = make_provider(&server.uri())
                .generate(&ref_schema("recraft/recraftv3"), &make_base("x", "recraft/recraftv3"), &make_extras(), &no_refs())
                .await
                .unwrap_err();
            assert!(
                matches!(&err, ProviderError::RequestFailed { status_code: Some(s), retryable: r, message, .. }
                    if *s == status && *r == retryable && message.contains(body)),
                "{status}: {err:?}"
            );
        }
    }

    /// Recraft answers in the same call, so one that timed out may have run
    /// and been billed: it is never sent again.
    #[tokio::test]
    async fn a_timed_out_call_is_never_sent_again() {
        let slow = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })).set_delay(std::time::Duration::from_secs(5)))
            .mount(&slow)
            .await;
        let mut p = make_provider(&slow.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let err = p.generate(&ref_schema("recraft/recraftv3"), &make_base("x", "recraft/recraftv3"), &make_extras(), &no_refs()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
    }

    /// F5: a result named by URL is fetched without the key, only from a
    /// public address, at most 64 MiB, and an error status is not an image.
    #[tokio::test]
    async fn a_url_result_is_fetched_only_from_a_public_address() {
        let server = MockServer::start().await;
        let cdn = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "url": format!("{}/r.png", cdn.uri()) }] })))
            .mount(&server)
            .await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"\x89PNG\r\n\x1a\n".to_vec())).mount(&cdn).await;
        let mut provider = make_provider(&server.uri());
        provider.public_assets_only = true;
        let err = provider
            .generate(&ref_schema("recraft/recraftv3"), &make_base("x", "recraft/recraftv3"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
        assert!(cdn.received_requests().await.unwrap().is_empty(), "a private address was fetched");
    }
}
