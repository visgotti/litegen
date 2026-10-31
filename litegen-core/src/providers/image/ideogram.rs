use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
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

/// The Ideogram generation endpoint a catalog id is served by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Generation {
    /// `POST /v1/ideogram-v3/generate`: `prompt`, `aspect_ratio`, `seed`,
    /// `style_type`, reference images; JSON or multipart.
    V3,
    /// `POST /v1/ideogram-v4/generate`: `text_prompt`, `resolution`,
    /// `rendering_speed`; multipart/form-data only.
    V4,
    /// `POST /v2/image/precise-edit/ideogram-4-5`: `image`, `mask`,
    /// `context_window`, `prompt`, `quality`, `seed`; multipart/form-data.
    /// Synchronous by default, answering the same `data[]` shape.
    ///
    /// @see <https://developer.ideogram.ai/v2/api-reference/images/precise-edit/ideogram-4-5.md>
    ///   Verbatim: "`y_min,x_min,y_max,x_max` names the region explicitly, by its
    ///   corners. Rows come first" and "The maximum edges are exclusive".
    PreciseEdit45,
}

/// Ideogram 3.0 and 4.0 image generation provider.
///
/// Synchronous: `POST /v1/ideogram-v3/generate` returns `{created, data: [{url,
/// seed, resolution, is_image_safe, style_type}]}`. Auth via the `Api-Key`
/// header (NOT Bearer). Reference images (style/character) are multipart file
/// uploads, so requests with refs use `multipart/form-data`; prompt-only
/// requests use JSON. Image-only. Result URLs expire, so bytes are fetched
/// immediately.
///
/// The `ideogram/ideogram-v4*` ids go to `POST /v1/ideogram-v4/generate`
/// instead (see [`Generation::V4`]): a different request contract with the
/// same response shape.
///
/// @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (2026-10-30) — the v1 spec
/// @see <https://developer.ideogram.ai/v1/api-reference/generate-images/generate-v3> (2026-10-30)
///   Verbatim (the page's Python sample, a JSON body): `json={ "prompt": "A picture of a cat",
///   "rendering_speed": "TURBO" }`
/// @see <https://developer.ideogram.ai/v1/api-reference/generate-images/generate-v4.md> (2026-10-30)
pub struct IdeogramProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl IdeogramProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::raw_header("Api-Key"),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.ideogram.ai")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("ideogram".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// Map model id → optional rendering_speed (TURBO/QUALITY/DEFAULT).
    ///
    /// 4.0 has the same tiers as 3.0 (`RenderingSpeed` enum). Its `FLASH` is
    /// "coming soon": "requests with `rendering_speed=FLASH` currently return
    /// a 400", so no catalog id maps to it.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (POST /v1/ideogram-v4/generate)
    fn rendering_speed(model_id: &str) -> Option<&'static str> {
        match model_id.strip_prefix("ideogram/").unwrap_or(model_id) {
            "ideogram-v3-turbo" | "ideogram-v4-turbo" => Some("TURBO"),
            "ideogram-v3-quality" | "ideogram-v4-quality" => Some("QUALITY"),
            _ => None,
        }
    }

    /// Which generation endpoint serves a catalog id. Only the three 4.0 ids
    /// route to v4; everything else keeps the 3.0 request unchanged.
    fn generation(model_id: &str) -> Option<Generation> {
        Some(match model_id.strip_prefix("ideogram/").unwrap_or(model_id) {
            "ideogram-v3" | "ideogram-v3-turbo" | "ideogram-v3-quality" => Generation::V3,
            "ideogram-v4" | "ideogram-v4-turbo" | "ideogram-v4-quality" => Generation::V4,
            "ideogram-4-5-precise-edit" => Generation::PreciseEdit45,
            // An id with no arm is refused, not run as Ideogram 3.0.
            _ => return None,
        })
    }

    /// What litegen itself sends to `POST /v1/ideogram-v3/generate`; `extra`
    /// cannot replace it.
    const V3_OWNED_FIELDS: &'static [&'static str] = &[
        "prompt", "aspect_ratio", "resolution", "rendering_speed", "negative_prompt", "num_images", "seed", "style_type",
        "style_reference_images", "character_reference_images", "character_reference_images_mask",
    ];

    /// A 3.0 request's extras: only the row's `extra_allowlist`, and never a
    /// field litegen sends (`rendering_speed` QUALITY on a TURBO row would
    /// bill another tier).
    fn v3_extras<'a>(model: &ModelSchema, extras: &'a ImageExtras) -> Vec<(&'a String, &'a Value)> {
        match &extras.extra {
            Some(Value::Object(map)) => map
                .iter()
                .filter(|(k, _)| model.extra_allowlist.iter().any(|a| a == *k) && !Self::V3_OWNED_FIELDS.contains(&k.as_str()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The text fields of a `POST /v1/ideogram-v4/generate` request.
    ///
    /// The spec declares only a `multipart/form-data` body, with `text_prompt`
    /// (mutually exclusive with the structured `json_prompt`), `resolution`
    /// (`ResolutionV4`, "WxH"), `rendering_speed` and
    /// `enable_copyright_detection`. There is no `prompt`, `aspect_ratio`,
    /// `seed`, `negative_prompt`, `num_images`, `style_type` or reference
    /// image field, so none is sent. Extras are limited to the model's
    /// `extra_allowlist` (the validator only filters them in strict mode), and
    /// multipart carries scalars as text (`enable_copyright_detection` is a
    /// boolean).
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (POST /v1/ideogram-v4/generate)
    fn v4_fields(
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
    ) -> Vec<(String, String)> {
        let mut fields = vec![("text_prompt".to_string(), base.prompt.clone())];
        if let Some(size) = extras.size.as_deref() {
            // The validator accepts `WxH` or `WXH`; ResolutionV4 values are lowercase.
            fields.push(("resolution".to_string(), size.replace('X', "x")));
        }
        if let Some(s) = Self::rendering_speed(&model.id) {
            fields.push(("rendering_speed".to_string(), s.to_string()));
        }
        if let Some(Value::Object(map)) = &extras.extra {
            for (k, v) in map {
                if !model.extra_allowlist.iter().any(|a| a == k) {
                    continue;
                }
                let text = match v {
                    Value::String(s) => s.clone(),
                    Value::Bool(_) | Value::Number(_) => v.to_string(),
                    _ => continue,
                };
                fields.push((k.clone(), text));
            }
        }
        fields
    }

    /// Send a generate request and download the first image. Both endpoints
    /// answer `{created, data: [{url, seed, resolution, is_image_safe, …}]}`;
    /// `url` is null when `is_image_safe` is false ("If false, the url field
    /// will be empty"), and a 422 carries `{"error": …}` for a prompt that
    /// failed the safety check.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (ImageGenerationObjectV4, GenerateImageSafetyError)
    async fn send(
        &self,
        builder: reqwest::RequestBuilder,
        creds: &ProviderCredentials,
    ) -> Result<GenerationOutput, ProviderError> {
        let builder = crate::providers::auth::apply(&self.auth, creds, builder)?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Ideogram request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Ideogram response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Ideogram API error: {data}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.as_u16() >= 500,
            });
        }

        let first = &data["data"][0];
        let image_url = match first["url"].as_str() {
            Some(url) if !url.is_empty() => url,
            _ => {
                let message = if first["is_image_safe"] == Value::Bool(false) {
                    "Ideogram withheld the image: data[0].is_image_safe is false".to_string()
                } else {
                    "Ideogram response missing data[0].url".to_string()
                };
                return Err(ProviderError::RequestFailed {
                    message,
                    status_code: None,
                    provider_error: Some(data.clone()),
                    retryable: false,
                });
            }
        };

        let bytes = self.fetch_image_bytes(image_url).await?;
        let mut metadata = HashMap::new();
        metadata.insert("url".to_string(), Value::String(image_url.to_string()));
        if let Some(seed) = first["seed"].as_i64() {
            metadata.insert("seed".to_string(), Value::Number(seed.into()));
        }

        Ok(GenerationOutput {
            data: bytes,
            content_type: "image/png".to_string(),
            metadata,
            more: Vec::new(),
        })
    }

    /// Ideogram uses `WxH` aspect-ratio tokens (e.g. "16x9"), not "16:9".
    fn ideogram_aspect(ar: &str) -> String {
        ar.replace(':', "x")
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to fetch Ideogram image: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: true,
        })?;
        if !resp.status().is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Ideogram image URL returned HTTP {}", resp.status()),
                status_code: Some(resp.status().as_u16()),
                provider_error: None,
                retryable: false,
            });
        }
        Ok(resp
            .bytes()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Ideogram image bytes: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
            .to_vec())
    }
}

impl Default for IdeogramProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for IdeogramProvider {
    fn name(&self) -> &str {
        "ideogram"
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
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let generation = Self::generation(&model.id)
            .ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "ideogram".into() })?;
        if generation == Generation::PreciseEdit45 {
            let url = format!("{}/v2/image/precise-edit/ideogram-4-5", self.api_base());
            let mut form = Form::new().text("prompt", base.prompt.clone());
            // The region, as Ideogram's own box: rows first, ends exclusive.
            if let Some(w) = extras.edit.as_ref().and_then(|e| e.window) {
                form = form.text("context_window", format!("{},{},{},{}", w.y, w.x, w.y + w.h, w.x + w.w));
            }
            if let Some(q) = extras.quality.as_deref() {
                form = form.text("quality", q.to_string());
            }
            if let Some(seed) = base.seed {
                form = form.text("seed", seed.to_string());
            }
            // `image` and `mask` (already black-edits: litegen converted it).
            let mut has_image = false;
            for r in &materialized.refs {
                if let MaterializedRefForm::MultipartField { field_name, bytes, content_type } = &r.form {
                    has_image |= field_name == "image";
                    let part = Part::bytes(bytes.to_vec())
                        .file_name(format!("{field_name}.png"))
                        .mime_str(content_type)
                        .map_err(|e| ProviderError::InvalidRequest(format!("ideogram precise edit: {e}")))?;
                    form = form.part(field_name.clone(), part);
                }
            }
            if !has_image {
                return Err(ProviderError::InvalidRequest(format!("'{}' edits an image: send one with role init", model.id)));
            }
            return self.send(self.client.post(&url).multipart(form), &creds).await;
        }
        if generation == Generation::V4 {
            let url = format!("{}/v1/ideogram-v4/generate", self.api_base());
            let mut form = Form::new();
            for (k, v) in Self::v4_fields(model, base, extras) {
                form = form.text(k, v);
            }
            return self.send(self.client.post(&url).multipart(form), &creds).await;
        }

        let url = format!("{}/v1/ideogram-v3/generate", self.api_base());
        let speed = Self::rendering_speed(&model.id);

        // Collect the common string fields once (shared by JSON + multipart).
        let mut fields: Vec<(&str, String)> = vec![("prompt", base.prompt.clone())];
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            fields.push(("aspect_ratio", Self::ideogram_aspect(ar)));
        }
        if let Some(size) = extras.size.as_deref() {
            fields.push(("resolution", size.to_string()));
        }
        if let Some(s) = speed {
            fields.push(("rendering_speed", s.to_string()));
        }
        if let Some(np) = base.negative_prompt.as_deref() {
            fields.push(("negative_prompt", np.to_string()));
        }
        if base.n > 1 {
            fields.push(("num_images", base.n.to_string()));
        }
        if let Some(seed) = base.seed {
            fields.push(("seed", seed.to_string()));
        }
        if let Some(style) = extras.style.as_deref() {
            fields.push(("style_type", style.to_string()));
        }

        let has_refs = materialized
            .refs
            .iter()
            .any(|r| matches!(r.form, MaterializedRefForm::MultipartField { .. }));

        let base_builder = self.client.post(&url);
        let builder = if has_refs {
            // multipart/form-data: text fields + reference-image file parts.
            let mut form = Form::new();
            for (k, v) in &fields {
                form = form.text(k.to_string(), v.clone());
            }
            // Extras as text parts. The spec does not say how multipart
            // carries a list or an object: `style_codes` ("A list of
            // 8-character hexadecimal codes") goes as repeated parts,
            // `color_palette` as JSON text.
            for (k, v) in Self::v3_extras(model, extras) {
                match v {
                    Value::String(text) => form = form.text(k.clone(), text.clone()),
                    Value::Array(items) => {
                        for item in items {
                            form = form.text(k.clone(), item.as_str().map(str::to_string).unwrap_or_else(|| item.to_string()));
                        }
                    }
                    other => form = form.text(k.clone(), other.to_string()),
                }
            }
            for r in &materialized.refs {
                if let MaterializedRefForm::MultipartField { field_name, bytes, content_type } = &r.form {
                    let part = Part::bytes(bytes.to_vec())
                        .file_name("ref")
                        .mime_str(content_type)
                        .map_err(|e| ProviderError::InvalidRequest(format!("ideogram ref mime: {e}")))?;
                    form = form.part(field_name.clone(), part);
                }
            }
            base_builder.multipart(form)
        } else {
            // JSON body for prompt-only requests.
            let mut body = serde_json::Map::new();
            for (k, v) in &fields {
                // num_images/seed are numeric in JSON.
                let val = match *k {
                    "num_images" | "seed" => v.parse::<i64>().map(Value::from).unwrap_or(Value::String(v.clone())),
                    _ => Value::String(v.clone()),
                };
                body.insert(k.to_string(), val);
            }
            for (k, v) in Self::v3_extras(model, extras) {
                body.insert(k.clone(), v.clone());
            }
            base_builder.header("Content-Type", "application/json").json(&Value::Object(body))
        };

        self.send(builder, &creds).await
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
                message: "Ideogram provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult {
            healthy: true,
            message: "Ideogram provider configured".into(),
            latency_ms: None,
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

    fn make_provider(api_base: &str) -> IdeogramProvider {
        let mut p = IdeogramProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: " id-key".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("id-key".to_string());
        p.configure(cfg);
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
            size: None,
            aspect_ratio: Some("16:9".to_string()),
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
    async fn generates_ideogram_v3_json() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/img.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/v1/ideogram-v3/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": "2026-05-30T00:00:00Z",
                "data": [{ "url": image_url.clone(), "seed": 42, "is_image_safe": true, "resolution": "1344x768" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"IDEOGRAMPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("ideogram/ideogram-v3");
        let base = make_base("a vintage travel poster of Mars", "ideogram/ideogram-v3");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"IDEOGRAMPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("api-key").unwrap(), "id-key");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["prompt"], "a vintage travel poster of Mars");
        assert_eq!(body["aspect_ratio"], "16x9"); // ':' -> 'x'
    }

    // ─── Ideogram 4.0: POST /v1/ideogram-v4/generate ────────────────────────
    // @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (post_generate_image_v4)

    /// Extract a simple text field's value out of a multipart/form-data body.
    fn multipart_field(body: &[u8], name: &str) -> Option<String> {
        let s = String::from_utf8_lossy(body).into_owned();
        let marker = format!("name=\"{name}\"");
        let at = s.find(&marker)? + marker.len();
        let rest = &s[at..];
        let value_at = rest.find("\r\n\r\n")? + 4;
        let value = &rest[value_at..];
        let end = value.find("\r\n")?;
        Some(value[..end].to_string())
    }

    fn v4_extras(size: Option<&str>) -> ImageExtras {
        ImageExtras { size: size.map(str::to_string), aspect_ratio: None, ..make_extras() }
    }

    /// Start an image host serving b"V4PNG", then mount the v4 generate
    /// response built from that host's image URL. Returns (api, image host).
    async fn mock_v4(respond: impl FnOnce(String) -> ResponseTemplate) -> (MockServer, MockServer) {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"V4PNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/ideogram-v4/generate"))
            .respond_with(respond(format!("{}/v4.png", image_server.uri())))
            .mount(&server)
            .await;
        (server, image_server)
    }

    fn v4_ok(image_url: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "response_type": "url",
            "created": "2026-09-11T00:00:00Z",
            "data": [{ "url": image_url, "prompt": "a cat, magic-prompted", "resolution": "1280x720", "is_image_safe": true, "seed": 7 }]
        }))
    }

    fn empty_materialized() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    #[test]
    fn only_the_v4_ids_route_to_the_v4_endpoint() {
        for id in ["ideogram/ideogram-v4", "ideogram/ideogram-v4-turbo", "ideogram/ideogram-v4-quality"] {
            assert_eq!(IdeogramProvider::generation(id), Some(Generation::V4), "{id}");
        }
        for id in ["ideogram/ideogram-v3", "ideogram/ideogram-v3-turbo", "ideogram/ideogram-v3-quality"] {
            assert_eq!(IdeogramProvider::generation(id), Some(Generation::V3), "{id}");
        }
    }

    /// Text-only submit: multipart to the v4 path with `text_prompt` (not
    /// `prompt`), `resolution` from `size`, the Api-Key header, and none of the
    /// v3-only fields. The default tier sends no `rendering_speed`.
    #[tokio::test]
    async fn v4_submits_multipart_text_prompt_to_the_v4_path() {
        let (server, _img) = mock_v4(|url| v4_ok(&url)).await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("ideogram/ideogram-v4");
        let base = make_base("a neon sign that says OPEN", "ideogram/ideogram-v4");
        let out = provider
            .generate(&schema, &base, &v4_extras(Some("1280x720")), &empty_materialized())
            .await
            .expect("generate");
        assert_eq!(out.data, b"V4PNG");
        assert_eq!(out.metadata.get("seed"), Some(&json!(7)));

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let post = &received[0];
        assert_eq!(post.url.path(), "/v1/ideogram-v4/generate");
        assert_eq!(post.headers.get("api-key").unwrap(), "id-key");
        let ct = post.headers.get("content-type").unwrap().to_str().unwrap();
        assert!(ct.starts_with("multipart/form-data"), "content-type {ct}");
        assert_eq!(multipart_field(&post.body, "text_prompt").as_deref(), Some("a neon sign that says OPEN"));
        assert_eq!(multipart_field(&post.body, "resolution").as_deref(), Some("1280x720"));
        for absent in ["prompt", "rendering_speed", "aspect_ratio", "num_images", "seed", "style_type", "negative_prompt"] {
            assert_eq!(multipart_field(&post.body, absent), None, "{absent} must not be sent to v4");
        }
    }

    /// The -turbo / -quality rows send their tier; allowlisted extras pass as
    /// text, and keys outside the allowlist are not forwarded.
    #[tokio::test]
    async fn v4_tiers_send_rendering_speed_and_allowlisted_extras() {
        for (id, speed) in [("ideogram/ideogram-v4-turbo", "TURBO"), ("ideogram/ideogram-v4-quality", "QUALITY")] {
            let (server, _img) = mock_v4(|url| v4_ok(&url)).await;

            let provider = make_provider(&server.uri());
            let schema = ref_schema(id);
            let mut extras = v4_extras(None);
            extras.extra = Some(json!({ "enable_copyright_detection": true, "magic_prompt": "OFF" }));
            provider
                .generate(&schema, &make_base("a cat", id), &extras, &empty_materialized())
                .await
                .expect("generate");

            let body = &server.received_requests().await.unwrap()[0].body;
            assert_eq!(multipart_field(body, "rendering_speed").as_deref(), Some(speed), "{id}");
            assert_eq!(multipart_field(body, "enable_copyright_detection").as_deref(), Some("true"), "{id}");
            assert_eq!(multipart_field(body, "magic_prompt"), None, "{id}: not in the v4 allowlist");
            assert_eq!(multipart_field(body, "resolution"), None, "{id}: no size, no resolution");
        }
    }

    /// Vendor failures surface as errors carrying the vendor's message: a 400
    /// (e.g. FLASH on v4) and the 422 safety error `{"error": …}`.
    #[tokio::test]
    async fn v4_vendor_errors_carry_the_vendor_message() {
        for (status, body) in [
            (400, json!({ "detail": "rendering_speed=FLASH is not supported for Ideogram V4" })),
            (422, json!({ "error": "Prompt failed the safety check" })),
        ] {
            let (server, _img) = mock_v4(|_| ResponseTemplate::new(status).set_body_json(body.clone())).await;
            let provider = make_provider(&server.uri());
            let err = provider
                .generate(&ref_schema("ideogram/ideogram-v4"), &make_base("x", "ideogram/ideogram-v4"), &v4_extras(None), &empty_materialized())
                .await
                .expect_err("vendor error must fail");
            match err {
                ProviderError::RequestFailed { message, status_code, provider_error, retryable } => {
                    assert_eq!(status_code, Some(status));
                    assert_eq!(provider_error, Some(body.clone()));
                    let vendor_text = body.as_object().unwrap().values().next().unwrap().as_str().unwrap();
                    assert!(message.contains(vendor_text), "{message}");
                    assert!(!retryable);
                }
                other => panic!("expected RequestFailed, got {other:?}"),
            }
        }
    }

    /// A 200 without a usable image fails instead of fetching nothing: an
    /// empty `data`, and `url: null` with `is_image_safe: false`.
    #[tokio::test]
    async fn v4_success_without_an_image_url_fails() {
        for (body, expect) in [
            (json!({ "created": "2026-09-11T00:00:00Z", "data": [] }), "missing data[0].url"),
            (
                json!({ "created": "2026-09-11T00:00:00Z", "data": [{ "url": null, "prompt": "p", "resolution": "1024x1024", "is_image_safe": false, "seed": 1 }] }),
                "is_image_safe is false",
            ),
        ] {
            let (server, _img) = mock_v4(|_| ResponseTemplate::new(200).set_body_json(body)).await;
            let provider = make_provider(&server.uri());
            let err = provider
                .generate(&ref_schema("ideogram/ideogram-v4"), &make_base("x", "ideogram/ideogram-v4"), &v4_extras(None), &empty_materialized())
                .await
                .expect_err("no image must fail");
            assert!(err.to_string().contains(expect), "{err}");
        }
    }

    // ─── Ideogram audit, 2026-10-30 ─────────────────────────────────────────

    /// F11: every catalog row has its own endpoint, and an id with none is
    /// refused rather than run as Ideogram 3.0.
    #[test]
    fn every_catalog_row_has_its_own_endpoint_and_an_unknown_id_is_refused() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        for m in catalog.for_provider("ideogram") {
            assert!(IdeogramProvider::generation(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(IdeogramProvider::generation("ideogram/ideogram-4-5").is_none());
    }

    /// Ideogram answers every 3.0 generate with a 400, so only the outbound
    /// body matters.
    async fn refusing_v3() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/ideogram-v3/generate"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "stop" })))
            .mount(&server)
            .await;
        server
    }

    /// F4: on 3.0 `extra` carries only the row's allowlist, and never over
    /// what litegen sends (`rendering_speed` QUALITY on a TURBO row would bill
    /// another tier, `num_images` more images).
    #[tokio::test]
    async fn a_3_0_request_sends_only_allowlisted_extras_and_never_over_what_litegen_sends() {
        let server = refusing_v3().await;
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "prompt": "other", "rendering_speed": "QUALITY", "num_images": 4, "magic_prompt": "OFF", "not_allowlisted": 1
        }));
        let _ = make_provider(&server.uri())
            .generate(&ref_schema("ideogram/ideogram-v3-turbo"), &make_base("a lantern", "ideogram/ideogram-v3-turbo"), &extras, &empty_materialized())
            .await;
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["prompt"], "a lantern");
        assert_eq!(body["rendering_speed"], "TURBO");
        assert!(body.get("num_images").is_none() && body.get("not_allowlisted").is_none(), "{body}");
        assert_eq!(body["magic_prompt"], "OFF");
    }

    /// F4: with reference images the body is multipart, and an allowlisted
    /// list (`style_codes`, "A list of 8-character hexadecimal codes") or
    /// object (`color_palette`) goes too, not only strings. The spec does not
    /// say how multipart carries them: a list as repeated parts, an object as
    /// JSON text.
    /// @see <https://developer.ideogram.ai/v1/api-reference/generate-images/generate-v3> (2026-10-30)
    #[tokio::test]
    async fn a_3_0_request_with_references_sends_every_allowlisted_extra() {
        let server = refusing_v3().await;
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "style_codes": ["AAAAAAAA", "BBBBBBBB"], "color_palette": { "name": "EMBER" }, "rendering_speed": "QUALITY"
        }));
        let refs = MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "style".into(),
                form: MaterializedRefForm::MultipartField {
                    field_name: "style_reference_images".into(),
                    bytes: bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\n"),
                    content_type: "image/png".into(),
                },
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        };
        let _ = make_provider(&server.uri())
            .generate(&ref_schema("ideogram/ideogram-v3-turbo"), &make_base("x", "ideogram/ideogram-v3-turbo"), &extras, &refs)
            .await;
        let received = server.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&received[0].body).to_string();
        assert_eq!(body.matches("name=\"style_codes\"").count(), 2, "{body}");
        assert!(body.contains("AAAAAAAA") && body.contains("BBBBBBBB"), "{body}");
        assert_eq!(multipart_field(&received[0].body, "color_palette").as_deref(), Some("{\"name\":\"EMBER\"}"));
        assert_eq!(body.matches("name=\"rendering_speed\"").count(), 1, "{body}");
        assert_eq!(multipart_field(&received[0].body, "rendering_speed").as_deref(), Some("TURBO"));
    }
}
