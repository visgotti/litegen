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

/// Upload reference bytes to Leonardo's presigned-S3 init-image endpoint,
/// returning the resulting `imageId`. Shared by the image and video providers.
///
/// Two steps: `POST /init-image {extension}` returns `uploadInitImage{id, url,
/// fields}` (fields is a JSON string of S3 form fields); then POST the file to
/// the returned S3 `url` as multipart (fields + `file`). The `id` is the imageId.
///
/// @see <https://docs.leonardo.ai/reference/uploadinitimage> — "returns presigned details to upload an init image to S3"
pub(crate) async fn upload_init_image(
    client: &Client,
    api_base: &str,
    auth: &AuthSpec,
    creds: &ProviderCredentials,
    bytes: &[u8],
) -> Result<String, ProviderError> {
    let url = format!("{api_base}/init-image");
    let builder = crate::providers::auth::apply(
        auth,
        creds,
        client.post(&url).header("Content-Type", "application/json").json(&json!({ "extension": "png" })),
    )?;
    let data: Value = builder
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Leonardo init-image request failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?
        .json()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Leonardo init-image response: {e}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;

    let uii = &data["uploadInitImage"];
    let image_id = uii["id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
        message: "Leonardo init-image missing uploadInitImage.id".to_string(),
        status_code: None,
        provider_error: Some(data.clone()),
        retryable: false,
    })?;
    let s3_url = uii["url"].as_str().ok_or_else(|| ProviderError::RequestFailed {
        message: "Leonardo init-image missing uploadInitImage.url".to_string(),
        status_code: None,
        provider_error: Some(data.clone()),
        retryable: false,
    })?;
    let fields: HashMap<String, String> = serde_json::from_str(uii["fields"].as_str().unwrap_or("{}"))
        .map_err(|e| ProviderError::InvalidRequest(format!("Leonardo init-image fields parse: {e}")))?;

    // POST the file to S3 with the presigned form fields (no Authorization header).
    let mut form = Form::new();
    for (k, v) in fields {
        form = form.text(k, v);
    }
    form = form.part(
        "file",
        Part::bytes(bytes.to_vec())
            .file_name("image.png")
            .mime_str("image/png")
            .map_err(|e| ProviderError::InvalidRequest(format!("leonardo ref mime: {e}")))?,
    );
    let s3_resp = client.post(s3_url).multipart(form).send().await.map_err(|e| ProviderError::RequestFailed {
        message: format!("Leonardo S3 upload failed: {e}"),
        status_code: None,
        provider_error: None,
        retryable: true,
    })?;
    if !s3_resp.status().is_success() {
        return Err(ProviderError::RequestFailed {
            message: format!("Leonardo S3 upload returned HTTP {}", s3_resp.status()),
            status_code: Some(s3_resp.status().as_u16()),
            provider_error: None,
            retryable: false,
        });
    }
    Ok(image_id.to_string())
}

/// Leonardo.Ai image generation provider.
///
/// Async: `POST /generations` returns `sdGenerationJob.generationId`; poll
/// `GET /generations/{id}` until `generations_by_pk.status == "COMPLETE"`, then
/// download `generated_images[0].url`. Bearer auth. Reference images are
/// uploaded via the presigned init-image flow and passed as `init_image_id`.
///
/// @see <https://docs.leonardo.ai/reference/creategeneration> — POST /generations
///   Verbatim: "POST https://cloud.leonardo.ai/api/rest/v1/generations"
/// @see <https://docs.leonardo.ai/reference/getgenerationbyid> — GET /generations/{id}
///   Verbatim: "GET https://cloud.leonardo.ai/api/rest/v1/generations/{id}"
pub struct LeonardoImageProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

const DEFAULT_MODEL_ID: &str = "b24e16ff-06e3-43eb-8d33-4416c2d75876"; // Leonardo Diffusion XL

impl LeonardoImageProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
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
            .unwrap_or("https://cloud.leonardo.ai/api/rest/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("leonardo".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// Resolve the Leonardo modelId (UUID). Honors config.model_mapping; falls
    /// back to a known alias or the documented default.
    fn resolve_model_id(&self, model_id: &str) -> String {
        if let Some(cfg) = &self.config {
            if let Some(m) = cfg.model_mapping.get(model_id) {
                return m.clone();
            }
        }
        match model_id.strip_prefix("leonardo/").unwrap_or(model_id) {
            "diffusion-xl" => DEFAULT_MODEL_ID.to_string(),
            // Anything that already looks like a UUID is passed through.
            other if other.contains('-') && other.len() >= 32 => other.to_string(),
            _ => DEFAULT_MODEL_ID.to_string(),
        }
    }

    async fn poll_generation(&self, id: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/generations/{}", self.api_base(), id);
        let max_attempts = 90;
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
            let data: Value = crate::providers::inject_trace_headers(builder)
                .send()
                .await
                .map_err(|e| ProviderError::RequestFailed {
                    message: e.to_string(),
                    status_code: None,
                    provider_error: None,
                    retryable: true,
                })?
                .json()
                .await
                .unwrap_or_default();
            match data["generations_by_pk"]["status"].as_str() {
                Some("COMPLETE") => return Ok(data),
                Some("FAILED") => {
                    return Err(ProviderError::RequestFailed {
                        message: "Leonardo generation failed".to_string(),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    })
                }
                _ => continue,
            }
        }
        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to fetch Leonardo image: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        Ok(resp
            .bytes()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Leonardo image: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
            .to_vec())
    }
}

impl Default for LeonardoImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for LeonardoImageProvider {
    fn name(&self) -> &str {
        "leonardo"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::new(config.api_keys.clone()));
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
        let model_id = self.resolve_model_id(&model.id);

        // Upload the first reference image (if any) -> init_image_id.
        let mut init_image_id: Option<String> = None;
        for r in &materialized.refs {
            if let MaterializedRefForm::MultipartField { bytes, .. } = &r.form {
                init_image_id = Some(upload_init_image(&self.client, self.api_base(), &self.auth, &creds, bytes).await?);
                break;
            }
        }

        let (mut w, mut h) = (1024u64, 768u64);
        if let Some(size) = extras.size.as_deref() {
            if let Some((sw, sh)) = size.split_once('x') {
                if let (Ok(a), Ok(b)) = (sw.parse(), sh.parse()) {
                    w = a;
                    h = b;
                }
            }
        }

        let mut body = json!({
            "prompt": base.prompt,
            "modelId": model_id,
            "width": w,
            "height": h,
            "num_images": base.n.max(1),
        });
        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        if let Some(id) = init_image_id {
            body["init_image_id"] = Value::String(id);
            if let Some(strength) = extras.strength {
                body["init_strength"] = json!(strength);
            }
        }
        if let Some(Value::Object(map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let url = format!("{}/generations", self.api_base());
        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let submit: Value = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Leonardo request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?
            .json()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to parse Leonardo response: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;

        let gen_id = submit["sdGenerationJob"]["generationId"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Leonardo response missing sdGenerationJob.generationId".to_string(),
            status_code: None,
            provider_error: Some(submit.clone()),
            retryable: false,
        })?;

        let result = self.poll_generation(gen_id).await?;
        let image_url = result["generations_by_pk"]["generated_images"][0]["url"]
            .as_str()
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Leonardo result missing generated_images[0].url".to_string(),
                status_code: None,
                provider_error: Some(result.clone()),
                retryable: false,
            })?;

        let bytes = self.fetch_image_bytes(image_url).await?;
        let mut metadata = HashMap::new();
        metadata.insert("generation_id".to_string(), Value::String(gen_id.to_string()));
        metadata.insert("url".to_string(), Value::String(image_url.to_string()));

        Ok(GenerationOutput { data: bytes, content_type: "image/png".to_string(), metadata })
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
                message: "Leonardo provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Leonardo provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> LeonardoImageProvider {
        let mut p = LeonardoImageProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "leo-key".to_string(), api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.api_key = Some("leo-key".to_string());
        p.configure(cfg);
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(), model: model.to_string(), n: 1, negative_prompt: None,
            seed: None, reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            size: Some("1024x768".to_string()), aspect_ratio: None, quality: None, style: None,
            steps: None, guidance_scale: None, strength: None, response_format: "url".to_string(), extra: None,
        }
    }

    #[tokio::test]
    async fn generates_via_job_and_poll() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/img.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sdGenerationJob": { "generationId": "leo-gen-1", "apiCreditCost": 8 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/generations/leo-gen-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "generations_by_pk": { "id": "leo-gen-1", "status": "COMPLETE",
                    "generated_images": [{ "id": "img1", "url": image_url.clone() }] }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"LEOPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("leonardo/diffusion-xl");
        let base = make_base("an enchanted forest", "leonardo/diffusion-xl");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"LEOPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("authorization").unwrap(), "Bearer leo-key");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["prompt"], "an enchanted forest");
        assert_eq!(body["modelId"], DEFAULT_MODEL_ID);
        assert_eq!(body["width"], 1024);
    }
}
