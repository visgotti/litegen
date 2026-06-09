// Replicate video generation provider
// API Reference: https://replicate.com/docs/reference/http
// POST /v1/predictions — create prediction (with version hash)
// GET  /v1/predictions/{id} — poll for status
// Models: AnimateDiff, SVD, SVD-XT, Zeroscope, ModelScope

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRequest, MaterializedRefForm};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Replicate video generation provider.
///
/// Hosts AnimateDiff, Stable Video Diffusion (SVD/SVD-XT), Zeroscope,
/// and ModelScope via the Replicate predictions API.
///
/// @see <https://replicate.com/docs/reference/http#create-a-prediction> — create (`POST /v1/predictions`)
/// @see <https://replicate.com/docs/reference/http#get-a-prediction> — poll (`GET /v1/predictions/{id}`)
pub struct ReplicateVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
}

/// Known Replicate video model version hashes.
/// Found on each model's Replicate page under "Versions".
struct ModelVersion {
    /// Full model owner/name (kept for documentation purposes)
    #[allow(dead_code)]
    model: &'static str,
    /// SHA256 version hash (from Replicate model page)
    version: &'static str,
    /// Default FPS for this model (informational; AnimateDiff hardcodes frame count,
    /// so this is not sent as an input field — kept for the documented named arms)
    #[allow(dead_code)]
    default_fps: u32,
    /// Max frames this model can generate (informational; not sent as an input field)
    #[allow(dead_code)]
    max_frames: u32,
    /// Whether this model requires an input image (kept for future use)
    #[allow(dead_code)]
    requires_image: bool,
}

/// Model version registry with hashes from Replicate.
///
/// @see <https://replicate.com/docs/reference/http#create-a-prediction> — the `version` hash sent in the create body
fn resolve_model_version(model: &str) -> ModelVersion {
    match model {
        // AnimateDiff — text-to-video
        // https://replicate.com/lucataco/animate-diff
        "replicate/animate-diff" | "replicate-animate-diff" => ModelVersion {
            model: "lucataco/animate-diff",
            version: "beecf59c4aee8d81bf04f0381033dfa10dc16e845b4ae00d281e2fa377e48a9f",
            default_fps: 8,
            max_frames: 32,
            requires_image: false,
        },

        // Stable Video Diffusion — image-to-video (14 frames)
        // https://replicate.com/stability-ai/stable-video-diffusion
        "replicate/svd" | "replicate-svd" => ModelVersion {
            model: "stability-ai/stable-video-diffusion",
            version: "3f0457e4619daac51203dedb472816fd4af51f3149fa7a9e0b5ffcf1b8172438",
            default_fps: 6,
            max_frames: 14,
            requires_image: true,
        },

        // Stable Video Diffusion XT — image-to-video (25 frames, longer)
        "replicate/svd-xt" | "replicate-svd-xt" => ModelVersion {
            model: "stability-ai/stable-video-diffusion",
            version: "3f0457e4619daac51203dedb472816fd4af51f3149fa7a9e0b5ffcf1b8172438",
            default_fps: 6,
            max_frames: 25,
            requires_image: true,
        },

        // Zeroscope v2 XL — text-to-video
        // https://replicate.com/anotherjesse/zeroscope-v2-xl
        "replicate/zeroscope" | "replicate-zeroscope" => ModelVersion {
            model: "anotherjesse/zeroscope-v2-xl",
            version: "9f747673945c62801b13b84701c783929c0ee784e4748ec062204894dda1a351",
            default_fps: 24,
            max_frames: 36,
            requires_image: false,
        },

        // ModelScope — text-to-video
        "replicate/modelscope" | "replicate-modelscope" => ModelVersion {
            model: "deforum/deforum_stable_diffusion",
            version: "e22e77495f2fb83c34d5fae2ad8ab63c0a87b6b573b6208e1535b23b89ea66d6",
            default_fps: 8,
            max_frames: 16,
            requires_image: false,
        },

        // Generic replicate/video model — fallback to AnimateDiff
        _ => ModelVersion {
            model: "lucataco/animate-diff",
            version: "beecf59c4aee8d81bf04f0381033dfa10dc16e845b4ae00d281e2fa377e48a9f",
            default_fps: 8,
            max_frames: 32,
            requires_image: false,
        },
    }
}

impl ReplicateVideoProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.replicate.com/v1")
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("replicate".into()))
    }
}

impl Default for ReplicateVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for ReplicateVideoProvider {
    fn name(&self) -> &str {
        "replicate"
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
            .is_some_and(|c| !c.api_key.is_empty() || self.key_pool.is_some())
    }

    /// Create a prediction (`POST {api_base}/predictions`) with a `version` hash.
    ///
    /// @see <https://replicate.com/docs/reference/http#create-a-prediction> — create a prediction.
    ///   Proves the `{version, input}` body, the `input` fields (`prompt`, `n_prompt`,
    ///   `seed`, `init_image`, plus `steps`/`guidance_scale` via `extra`), and the
    ///   `id`/`urls.get` response fields. The backing AnimateDiff model does not accept
    ///   `fps`/`num_frames`/`negative_prompt`, so those are intentionally not sent.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let api_key = self.api_key()?;
        let mv = resolve_model_version(&model.id);
        // Honor an operator-supplied `version` override via model_mapping
        // (value form: "owner/name:version" or just a bare "version" hash).
        let version = self
            .config
            .as_ref()
            .and_then(|c| c.model_mapping.get(&model.id))
            .map(|mapped| {
                mapped
                    .rsplit_once(':')
                    .map(|(_, v)| v.to_string())
                    .unwrap_or_else(|| mapped.clone())
            })
            .unwrap_or_else(|| mv.version.to_string());
        let url = format!("{}/predictions", self.api_base());

        // Build input payload.
        //
        // The only catalog-exposed video id (`replicate/video`) resolves to
        // lucataco/animate-diff, whose Cog input schema is exactly
        // {motion_module, path, prompt, n_prompt, steps, guidance_scale, seed}.
        // It does NOT accept `fps`, `num_frames`, or `negative_prompt`; it uses
        // `n_prompt` for the negative prompt and hardcodes the frame count. So we
        // send only the widely-safe fields it accepts (prompt, n_prompt, seed) and
        // let `extras.extra` supply `steps`/`guidance_scale` via the shallow-merge
        // below. `fps`/`num_frames`/`negative_prompt` are intentionally NOT sent —
        // Replicate rejects unknown input keys (or silently drops them), so emitting
        // them either 422s the request or makes duration/fps a no-op.
        let mut input = json!({
            "prompt": base.prompt,
        });

        if let Some(seed) = base.seed {
            input["seed"] = json!(seed);
        }

        if let Some(np) = base.negative_prompt.as_deref() {
            input["n_prompt"] = Value::String(np.to_string());
        }

        // Reference image for image-to-video models
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Url(img_url) => {
                    input["init_image"] = Value::String(img_url.clone());
                    break;
                }
                MaterializedRefForm::Base64(b64) => {
                    input["init_image"] =
                        Value::String(format!("data:image/png;base64,{b64}"));
                    break;
                }
                _ => {}
            }
        }

        // Shallow-merge extra
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(input_obj) = input.as_object_mut() {
                for (k, v) in extra_map {
                    input_obj.insert(k.clone(), v.clone());
                }
            }
        }

        let body = json!({
            "version": version,
            "input": input,
        });

        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(&url)
                .header("Authorization", format!("Bearer {api_key}"))
                .header("Content-Type", "application/json")
                .json(&body),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Replicate request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;

        let status = resp.status();
        let resp_json: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Replicate response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        if !status.is_success() {
            let err = resp_json["detail"]
                .as_str()
                .or_else(|| resp_json["error"].as_str())
                .unwrap_or("Unknown error")
                .to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("Replicate API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                retryable: status.as_u16() >= 500,
            });
        }

        let prediction_id = resp_json["id"]
            .as_str()
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Replicate response missing id".to_string(),
                status_code: None,
                provider_error: Some(resp_json.clone()),
                retryable: false,
            })?
            .to_string();

        // Store prediction_id plus optional poll_url
        let job_data = json!({
            "prediction_id": prediction_id,
            "poll_url": resp_json["urls"]["get"].as_str().unwrap_or(""),
        });

        Ok(VideoGenerationHandle {
            provider_job_id: job_data.to_string(),
            provider: "replicate".to_string(),
            model: model.id.clone(),
        })
    }

    /// Poll a prediction (`GET {api_base}/predictions/{id}`), reading `status`
    /// (`succeeded`/`failed`/`processing`/`starting`) and the `output` URL(s).
    ///
    /// @see <https://replicate.com/docs/reference/http#get-a-prediction> — get a prediction
    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let api_key = self.api_key()?;

        let job_data: Value =
            serde_json::from_str(&handle.provider_job_id).unwrap_or_default();
        let prediction_id = job_data["prediction_id"].as_str().unwrap_or("");

        let poll_url = if let Some(url) = job_data["poll_url"].as_str() {
            if !url.is_empty() {
                url.to_string()
            } else {
                format!("{}/predictions/{prediction_id}", self.api_base())
            }
        } else {
            format!("{}/predictions/{prediction_id}", self.api_base())
        };

        // GET /v1/predictions/{id} — https://replicate.com/docs/reference/http#get-a-prediction
        let resp = crate::providers::inject_trace_headers(
            self.client
                .get(&poll_url)
                .header("Authorization", format!("Bearer {api_key}")),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: e.to_string(),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;

        let data: Value = resp.json().await.unwrap_or_default();

        let status = match data["status"].as_str() {
            Some("succeeded") => GenerationStatus::Completed,
            Some("failed") | Some("canceled") => GenerationStatus::Failed,
            Some("processing") => GenerationStatus::Processing,
            Some("starting") => GenerationStatus::Pending,
            _ => GenerationStatus::Pending,
        };

        // Video URL from output (string or array of strings)
        let video_url = data["output"]
            .as_str()
            .map(String::from)
            .or_else(|| {
                data["output"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });

        let error = data["error"].as_str().map(String::from);

        let mut metadata = HashMap::new();
        if let Some(predict_time) = data["metrics"]["predict_time"].as_f64() {
            metadata.insert("predict_time".into(), json!(predict_time));
        }

        Ok(VideoGenerationPollResult {
            status,
            progress: match status {
                GenerationStatus::Completed => 100,
                GenerationStatus::Processing => 50,
                _ => 10,
            },
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error,
            metadata,
        })
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &VideoGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(
            model.pricing.base_cost_usd,
            0.0,
            CostSource::Estimated,
            Some(json!({ "model": model.id })),
        ))
    }

    /// Validate the API token via `GET {api_base}/account`.
    ///
    /// @see <https://replicate.com/docs/reference/http#get-the-authenticated-account> — Get the authenticated account
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Replicate Video provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        let api_key = match self.api_key() {
            Ok(k) => k,
            Err(_) => {
                return HealthCheckResult {
                    healthy: false,
                    message: "No API key".into(),
                    latency_ms: None,
                }
            }
        };
        match self
            .client
            .get(format!("{}/account", self.api_base()))
            .header("Authorization", format!("Bearer {api_key}"))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Replicate API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Replicate returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Replicate health check failed: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn empty_materialized() -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    fn make_provider(api_base: &str) -> ReplicateVideoProvider {
        let mut p = ReplicateVideoProvider::new();
        p.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: "test-key".to_string(),
            api_keys: vec![],
            api_base: Some(api_base.to_string()),
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });
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

    fn make_extras() -> VideoExtras {
        VideoExtras {
            duration_seconds: 4.0,
            aspect_ratio: None,
            resolution: None,
            fps: None,
            extra: None,
        }
    }

    #[tokio::test]
    async fn submits_replicate_prediction_and_returns_handle() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/predictions"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": "rep_video_1",
                "status": "starting",
                "urls": {
                    "get": "https://api.replicate.com/v1/predictions/rep_video_1"
                }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("replicate/video");
        let base = make_base("a time lapse of clouds", "replicate/video");
        let extras = make_extras();
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let handle = result.unwrap();

        // provider_job_id is JSON containing prediction_id
        let job_data: Value = serde_json::from_str(&handle.provider_job_id).unwrap();
        assert_eq!(job_data["prediction_id"], "rep_video_1");

        assert_eq!(handle.provider, "replicate");
        assert_eq!(handle.model, "replicate/video");

        // Verify outbound request body
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert!(body["version"].as_str().is_some(), "version hash missing");
        assert_eq!(body["input"]["prompt"], "a time lapse of clouds");
    }
}
