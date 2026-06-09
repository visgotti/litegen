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

/// Runway Gen-4 image generation provider.
///
/// Shares the Runway developer API host (`api.dev.runwayml.com/v1`), Bearer auth,
/// and mandatory `X-Runway-Version` header with the Runway video provider.
/// Asynchronous: `POST /text_to_image` returns a task id; poll `GET /tasks/{id}`
/// until `SUCCEEDED`, then download the first `output` URL.
///
/// @see <https://docs.dev.runwayml.com/guides/using-the-api/> — text_to_image
///   Verbatim: "curl -X POST https://api.dev.runwayml.com/v1/text_to_image"
///   Verbatim: "\"promptText\": \"@EiffelTower painted in the style of @StarryNight\", \"model\": \"gen4_image\", \"ratio\": \"1920:1080\","
///   Verbatim: "-H \"Authorization: Bearer $RUNWAYML_API_SECRET\" -H \"X-Runway-Version: 2024-11-06\""
pub struct RunwayImageProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

const RUNWAY_VERSION: &str = "2024-11-06";

impl RunwayImageProvider {
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
            .unwrap_or("https://api.dev.runwayml.com/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("runway".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        match model_id.strip_prefix("runway/").unwrap_or(model_id) {
            "gen4_image_turbo" | "gen4-image-turbo" => "gen4_image_turbo",
            _ => "gen4_image",
        }
    }

    /// Map a unified size / aspect ratio to a Runway `ratio` (pixel WxH string).
    fn resolve_ratio(extras: &ImageExtras) -> String {
        if let Some(size) = extras.size.as_deref() {
            if let Some((w, h)) = size.split_once('x') {
                if w.parse::<u32>().is_ok() && h.parse::<u32>().is_ok() {
                    return format!("{w}:{h}");
                }
            }
        }
        match extras.aspect_ratio.as_deref() {
            Some("9:16") => "1080:1920",
            Some("1:1") => "1024:1024",
            Some("4:3") => "1440:1080",
            Some("3:4") => "1080:1440",
            Some("21:9") => "2112:912",
            Some("3:2") => "1808:1152",
            Some("2:3") => "1152:1808",
            _ => "1920:1080",
        }
        .to_string()
    }

    async fn poll_task(&self, id: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/tasks/{}", self.api_base(), id);
        let max_attempts = 90; // 90 * 2s = 3 min
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let builder = crate::providers::auth::apply(
                &self.auth,
                &creds,
                self.client.get(&url).header("X-Runway-Version", RUNWAY_VERSION),
            )?;
            let resp = crate::providers::inject_trace_headers(builder)
                .send()
                .await
                .map_err(|e| ProviderError::RequestFailed {
                    message: e.to_string(),
                    status_code: e.status().map(|s| s.as_u16()),
                    provider_error: None,
                    retryable: true,
                })?;
            let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to parse Runway poll response: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;
            match data["status"].as_str() {
                Some("SUCCEEDED") => return Ok(data),
                Some("FAILED") | Some("CANCELLED") => {
                    let err = data["failure"].as_str().or_else(|| data["error"].as_str()).unwrap_or("Unknown error").to_string();
                    return Err(ProviderError::RequestFailed {
                        message: format!("Runway task failed: {err}"),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    });
                }
                _ => continue,
            }
        }
        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to fetch Runway image: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: true,
        })?;
        if !resp.status().is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Runway image URL returned HTTP {}", resp.status()),
                status_code: Some(resp.status().as_u16()),
                provider_error: None,
                retryable: false,
            });
        }
        Ok(resp
            .bytes()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Runway image bytes: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
            .to_vec())
    }
}

impl Default for RunwayImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for RunwayImageProvider {
    fn name(&self) -> &str {
        "runway"
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

    /// Submit `POST /text_to_image`, poll the task, fetch the first output.
    ///
    /// @see <https://docs.dev.runwayml.com/guides/using-the-api/> — `promptText`,
    ///   `model`, `ratio`, `referenceImages[]` ({uri, tag}), and `GET /tasks/{id}`.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/text_to_image", self.api_base());

        let mut body = json!({
            "promptText": base.prompt,
            "model": native,
            "ratio": Self::resolve_ratio(extras),
        });
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }

        // Reference images: {uri, tag}. uri may be a URL or a base64 data URI.
        let mut refs: Vec<Value> = Vec::new();
        for (i, r) in materialized.refs.iter().enumerate() {
            let tag = if r.role.is_empty() || r.role == "init" { format!("ref{i}") } else { r.role.clone() };
            match &r.form {
                MaterializedRefForm::Url(u) => refs.push(json!({ "uri": u, "tag": tag })),
                MaterializedRefForm::Base64(b64) => {
                    refs.push(json!({ "uri": format!("data:image/png;base64,{b64}"), "tag": tag }))
                }
                _ => {}
            }
        }
        if !refs.is_empty() {
            body["referenceImages"] = Value::Array(refs);
        }

        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in extra_map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("X-Runway-Version", RUNWAY_VERSION)
                .json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Runway image request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let submit: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Runway response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            let err = submit["error"].as_str().unwrap_or("Unknown error").to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("Runway API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(submit),
                retryable: status.as_u16() >= 500,
            });
        }

        let id = submit["id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Runway response missing task id".to_string(),
            status_code: None,
            provider_error: Some(submit.clone()),
            retryable: false,
        })?;

        let final_result = self.poll_task(id).await?;
        let image_url = match &final_result["output"] {
            Value::Array(arr) => arr.first().and_then(|v| v.as_str()).map(str::to_string),
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
        .ok_or_else(|| ProviderError::RequestFailed {
            message: "Runway task output missing image URL".to_string(),
            status_code: None,
            provider_error: Some(final_result.clone()),
            retryable: false,
        })?;

        let bytes = self.fetch_image_bytes(&image_url).await?;
        let mut metadata = HashMap::new();
        metadata.insert("task_id".to_string(), Value::String(id.to_string()));
        metadata.insert("url".to_string(), Value::String(image_url));

        Ok(GenerationOutput {
            data: bytes,
            content_type: "image/png".to_string(),
            metadata,
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
                message: "Runway image provider not configured".into(),
                latency_ms: None,
            };
        }
        // Runway has no cheap unauthenticated ping; report configured.
        HealthCheckResult {
            healthy: true,
            message: "Runway image provider configured".into(),
            latency_ms: None,
        }
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

    fn make_provider(api_base: &str) -> RunwayImageProvider {
        let mut p = RunwayImageProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: "rw-secret".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("rw-secret".to_string());
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
    async fn generates_gen4_image_via_polling() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/out.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/text_to_image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "task_1" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/tasks/task_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "task_1", "status": "SUCCEEDED", "output": [image_url.clone()]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"RUNWAYPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4_image");
        let base = make_base("a neon tokyo alley", "runway/gen4_image");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"RUNWAYPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("authorization").unwrap(), "Bearer rw-secret");
        assert_eq!(post.headers.get("x-runway-version").unwrap(), "2024-11-06");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["promptText"], "a neon tokyo alley");
        assert_eq!(body["model"], "gen4_image");
        assert_eq!(body["ratio"], "1920:1080");
    }
}
