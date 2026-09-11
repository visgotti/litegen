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
/// @see <https://developer.ideogram.ai/api-reference/api-reference/generate-v3>
///   Verbatim: "The model version to use for describing images. Defaults to V_3."
/// @see <https://developer.ideogram.ai/llms-full.txt>
///   Verbatim: "curl -X POST https://api.ideogram.ai/v1/ideogram-v3/generate \
///     -H \"Api-Key: <apiKey>\" -H \"Content-Type: application/json\" \
///     -d '{\"prompt\": \"A picture of a cat\"}'"
/// @see <https://developer.ideogram.ai/api-reference/generate-images/generate-v4.md>
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
    /// @see <https://developer.ideogram.ai/openapi.json> (POST /v1/ideogram-v4/generate)
    fn rendering_speed(model_id: &str) -> Option<&'static str> {
        match model_id.strip_prefix("ideogram/").unwrap_or(model_id) {
            "ideogram-v3-turbo" | "ideogram-v4-turbo" => Some("TURBO"),
            "ideogram-v3-quality" | "ideogram-v4-quality" => Some("QUALITY"),
            _ => None,
        }
    }

    /// Which generation endpoint serves a catalog id. Only the three 4.0 ids
    /// route to v4; everything else keeps the 3.0 request unchanged.
    fn generation(model_id: &str) -> Generation {
        match model_id.strip_prefix("ideogram/").unwrap_or(model_id) {
            "ideogram-v4" | "ideogram-v4-turbo" | "ideogram-v4-quality" => Generation::V4,
            _ => Generation::V3,
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
    /// @see <https://developer.ideogram.ai/openapi.json> (POST /v1/ideogram-v4/generate)
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
    /// @see <https://developer.ideogram.ai/openapi.json> (ImageGenerationObjectV4, GenerateImageSafetyError)
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
        if Self::generation(&model.id) == Generation::V4 {
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
            // extra string params pass through as text fields.
            if let Some(Value::Object(map)) = &extras.extra {
                for (k, v) in map {
                    if let Some(s) = v.as_str() {
                        form = form.text(k.clone(), s.to_string());
                    }
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
            if let Some(Value::Object(map)) = &extras.extra {
                for (k, v) in map {
                    body.insert(k.clone(), v.clone());
                }
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
            size: None,
            aspect_ratio: Some("16:9".to_string()),
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            extra: None,
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
    // @see <https://developer.ideogram.ai/openapi.json> (post_generate_image_v4)

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
            assert_eq!(IdeogramProvider::generation(id), Generation::V4, "{id}");
        }
        for id in ["ideogram/ideogram-v3", "ideogram/ideogram-v3-turbo", "ideogram/ideogram-v3-quality"] {
            assert_eq!(IdeogramProvider::generation(id), Generation::V3, "{id}");
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
}
