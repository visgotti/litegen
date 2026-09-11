use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{tc3, AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, CredentialPool, GenerationOutput, HealthCheckResult, ImageExtras, ImageProvider,
    ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

const TC3_CONTENT_TYPE: &str = "application/json; charset=utf-8";
const IMAGE_VERSION: &str = "2023-09-01";

/// Hunyuan Image 3.0 is served by a different Tencent product than Image 2.0:
/// aiart (`aiart.tencentcloudapi.com`, version 2022-12-29, TC3 service
/// `aiart`). `SubmitTextToImageJob` returns `Response.JobId`;
/// `QueryTextToImageJob` reports `JobStatusCode` 1 (queued) | 2 (running) |
/// 4 (failed, `JobErrorMsg`) | 5 (done, `ResultImage` URLs valid for 1 h).
/// Only this catalog id takes that path.
///
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/aiart/v20221229/client.go>
///   (`SubmitTextToImageJob` / `QueryTextToImageJob`: `InitBaseRequest(…, "aiart", APIVersion, …)`)
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/aiart/v20221229/models.go>
///   (`SubmitTextToImageJobRequestParams`, `QueryTextToImageJobResponseParams`)
const IMAGE3_MODEL_ID: &str = "hunyuan/hunyuan-image-3";
const AIART_VERSION: &str = "2022-12-29";

/// A Tencent Cloud product the adapter calls: its TC3 service name (also the
/// `<service>.tencentcloudapi.com` host prefix) and its API version.
#[derive(Clone, Copy)]
struct Product {
    service: &'static str,
    version: &'static str,
}

/// Hunyuan Image 2.0 (`SubmitHunyuanImageJob`).
const HUNYUAN: Product = Product { service: "hunyuan", version: IMAGE_VERSION };
/// Hunyuan Image 3.0 (`SubmitTextToImageJob`).
const AIART: Product = Product { service: "aiart", version: AIART_VERSION };

/// Tencent Hunyuan image generation provider.
///
/// RPC-style Tencent Cloud API: POST to `/` with the action in the
/// `X-TC-Action` header, signed with TC3-HMAC-SHA256 (SecretId=key_id,
/// SecretKey=key_secret, region). Async: `SubmitHunyuanImageJob` returns
/// `Response.JobId`; poll `QueryHunyuanImageJob` until `JobStatusCode == "5"`,
/// then download `Response.ResultImage[0]`. Region is `ap-guangzhou` only.
///
/// @see <https://cloud.tencent.com/document/product/1729/105969>
///   Verbatim: "本接口仅支持其中的: ap-guangzhou ... X-TC-Action: SubmitHunyuanImageJob, X-TC-Version: 2023-09-01, domain hunyuan.tencentcloudapi.com, POST to path /."
/// @see <https://www.tencentcloud.com/document/product/845/32207> — TC3-HMAC-SHA256
pub struct HunyuanImageProvider {
    config: Option<ProviderInstanceConfig>,
    cred_pool: Option<CredentialPool>,
    auth: AuthSpec,
    client: Client,
}

impl HunyuanImageProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            cred_pool: None,
            auth: AuthSpec::TencentTc3 { service: "hunyuan".into(), default_region: "ap-guangzhou".into() },
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// The configured `api_base` (tests, proxies) wins for every product;
    /// otherwise the product's own `https://<service>.tencentcloudapi.com/`.
    fn endpoint(&self, product: Product) -> String {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("https://{}.tencentcloudapi.com/", product.service))
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("hunyuan".into()))?;
        if let Some(pool) = &self.cred_pool {
            return Ok(base.with_signing(pool.next()));
        }
        Ok(base)
    }

    fn region(&self, creds: &ProviderCredentials) -> String {
        creds.region.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "ap-guangzhou".to_string())
    }

    /// Issue one TC3-signed RPC call (POST `/`, action via X-TC-Action header).
    async fn rpc(
        &self,
        creds: &ProviderCredentials,
        region: &str,
        product: Product,
        action: &str,
        body: &Value,
    ) -> Result<Value, ProviderError> {
        let body_bytes = serde_json::to_vec(body).map_err(|e| ProviderError::InvalidRequest(e.to_string()))?;
        let endpoint = self.endpoint(product);
        let url = reqwest::Url::parse(&endpoint).map_err(|e| ProviderError::InvalidRequest(format!("bad hunyuan url: {e}")))?;
        let signed = tc3::sign(creds, product.service, &url, TC3_CONTENT_TYPE, &body_bytes)?;

        let mut req = self
            .client
            .post(url)
            .header("Content-Type", TC3_CONTENT_TYPE)
            .header("X-TC-Action", action)
            .header("X-TC-Version", product.version)
            .header("X-TC-Region", region);
        for (k, v) in &signed {
            req = req.header(k, v);
        }
        let data: Value = crate::providers::inject_trace_headers(req.body(body_bytes))
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Hunyuan {action} failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?
            .json()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to parse Hunyuan response: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;
        // Tencent error envelope: Response.Error{Code, Message}.
        if let Some(err) = data["Response"]["Error"].as_object() {
            return Err(ProviderError::RequestFailed {
                message: format!("Hunyuan API error: {}", err.get("Message").and_then(|m| m.as_str()).unwrap_or("unknown")),
                status_code: None,
                provider_error: Some(data.clone()),
                retryable: false,
            });
        }
        Ok(data)
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to fetch Hunyuan image: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: true,
        })?;
        if !resp.status().is_success() {
            // A non-2xx (e.g. an expired/throttled presigned CDN URL) delivers a
            // short error body, not image bytes. Returning it would store an
            // error blob as the generated image and bill for a broken asset.
            return Err(ProviderError::RequestFailed {
                message: format!("Hunyuan image URL returned HTTP {}", resp.status()),
                status_code: Some(resp.status().as_u16()),
                provider_error: None,
                retryable: false,
            });
        }
        Ok(resp
            .bytes()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Hunyuan image: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
            .to_vec())
    }

    /// Hunyuan Image 3.0: `SubmitTextToImageJob` on aiart, then poll
    /// `QueryTextToImageJob`. The request takes exactly Prompt, Images,
    /// Resolution, Seed, LogoAdd, LogoParam and Revise — no NegativePrompt,
    /// which the catalog row does not advertise.
    async fn generate_image3(
        &self,
        creds: &ProviderCredentials,
        region: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let mut submit = json!({ "Prompt": base.prompt });
        if let Some(size) = extras.size.as_deref() {
            // Resolution is `W:H`; the validator accepts `WxH` or `WXH`.
            submit["Resolution"] = Value::String(size.replace(['x', 'X'], ":"));
        }
        if let Some(seed) = base.seed {
            // The catalog bounds the seed to Tencent's 1..=4294967295.
            submit["Seed"] = Value::Number(seed.into());
        }
        // Images: "参考图，最多三张图 - Base64 或 Url" — a flat list of strings,
        // each either raw base64 or a URL.
        let images: Vec<Value> = materialized
            .refs
            .iter()
            .filter_map(|r| match &r.form {
                MaterializedRefForm::Base64(b64) => Some(Value::String(b64.clone())),
                MaterializedRefForm::Url(u) => Some(Value::String(u.clone())),
                _ => None,
            })
            .collect();
        if !images.is_empty() {
            submit["Images"] = Value::Array(images);
        }
        if let Some(Value::Object(map)) = &extras.extra {
            if let Some(obj) = submit.as_object_mut() {
                for (k, v) in map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let submit_resp = self.rpc(creds, region, AIART, "SubmitTextToImageJob", &submit).await?;
        let job_id = submit_resp["Response"]["JobId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Hunyuan Image 3.0 submit missing Response.JobId".to_string(),
                status_code: None,
                provider_error: Some(submit_resp.clone()),
                retryable: false,
            })?;

        // JobStatusCode: 1 queued, 2 running, 4 failed, 5 done. Prompt
        // rewriting (Revise, on by default) adds about 20 s.
        let max_attempts = 90;
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let q = self.rpc(creds, region, AIART, "QueryTextToImageJob", &json!({ "JobId": job_id })).await?;
            let resp = &q["Response"];
            let non_empty = |k: &str| resp[k].as_str().filter(|s| !s.is_empty()).map(String::from);
            match resp["JobStatusCode"].as_str() {
                Some("5") => {
                    // A finished job with no image will never produce one.
                    let image_url = resp["ResultImage"][0]
                        .as_str()
                        .filter(|u| !u.is_empty())
                        .ok_or_else(|| ProviderError::RequestFailed {
                            message: "Hunyuan Image 3.0 job is done but returned no ResultImage".to_string(),
                            status_code: None,
                            provider_error: Some(q.clone()),
                            retryable: false,
                        })?;
                    let bytes = self.fetch_image_bytes(image_url).await?;
                    let mut metadata = HashMap::new();
                    metadata.insert("job_id".to_string(), Value::String(job_id.to_string()));
                    return Ok(GenerationOutput { data: bytes, content_type: "image/png".to_string(), metadata });
                }
                Some("4") => {
                    let why = non_empty("JobErrorMsg")
                        .or_else(|| non_empty("JobErrorCode"))
                        .or_else(|| non_empty("JobStatusMsg"))
                        .unwrap_or_else(|| "unknown error".to_string());
                    return Err(ProviderError::RequestFailed {
                        message: format!("Hunyuan Image 3.0 job failed: {why}"),
                        status_code: None,
                        provider_error: Some(q.clone()),
                        retryable: false,
                    });
                }
                _ => continue,
            }
        }
        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
    }
}

impl Default for HunyuanImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for HunyuanImageProvider {
    fn name(&self) -> &str {
        "hunyuan"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.credentials.credential_sets.is_empty() {
            self.cred_pool = Some(CredentialPool::shared(config.credentials.credential_sets.clone()));
        }
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.cred_pool.is_some())
    }

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let region = self.region(&creds);
        if model.id == IMAGE3_MODEL_ID {
            return self.generate_image3(&creds, &region, base, extras, materialized).await;
        }

        let mut submit = json!({ "Prompt": base.prompt });
        if let Some(np) = base.negative_prompt.as_deref() {
            submit["NegativePrompt"] = Value::String(np.to_string());
        }
        if let Some(size) = extras.size.as_deref() {
            // Resolution takes `W:H`. The validator accepts a size spelled
            // `WxH` or `WXH` (`parse_size`), so convert either separator.
            submit["Resolution"] = Value::String(size.replace(['x', 'X'], ":"));
        }
        if let Some(seed) = base.seed {
            submit["Seed"] = Value::Number(seed.max(0).into());
        }
        // Optional reference/control image: nested ContentImage{ImageBase64|ImageUrl}.
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Base64(b64) => submit["ContentImage"] = json!({ "ImageBase64": b64 }),
                MaterializedRefForm::Url(u) => submit["ContentImage"] = json!({ "ImageUrl": u }),
                _ => {}
            }
        }
        if let Some(Value::Object(map)) = &extras.extra {
            if let Some(obj) = submit.as_object_mut() {
                for (k, v) in map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let submit_resp = self.rpc(&creds, &region, HUNYUAN, "SubmitHunyuanImageJob", &submit).await?;
        let job_id = submit_resp["Response"]["JobId"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Hunyuan submit missing Response.JobId".to_string(),
            status_code: None,
            provider_error: Some(submit_resp.clone()),
            retryable: false,
        })?;

        // Poll QueryHunyuanImageJob until JobStatusCode 5 (done) / 4 (failed).
        let max_attempts = 90;
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let q = self.rpc(&creds, &region, HUNYUAN, "QueryHunyuanImageJob", &json!({ "JobId": job_id })).await?;
            let resp = &q["Response"];
            match resp["JobStatusCode"].as_str() {
                Some("5") => {
                    let image_url = resp["ResultImage"][0].as_str().ok_or_else(|| ProviderError::RequestFailed {
                        message: "Hunyuan result missing ResultImage[0]".to_string(),
                        status_code: None,
                        provider_error: Some(q.clone()),
                        retryable: false,
                    })?;
                    let bytes = self.fetch_image_bytes(image_url).await?;
                    let mut metadata = HashMap::new();
                    metadata.insert("job_id".to_string(), Value::String(job_id.to_string()));
                    return Ok(GenerationOutput { data: bytes, content_type: "image/png".to_string(), metadata });
                }
                Some("4") => {
                    return Err(ProviderError::RequestFailed {
                        message: format!("Hunyuan job failed: {}", resp["JobStatusMsg"].as_str().unwrap_or("")),
                        status_code: None,
                        provider_error: Some(q.clone()),
                        retryable: false,
                    })
                }
                _ => continue,
            }
        }
        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
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
                message: "Hunyuan provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Hunyuan provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> HunyuanImageProvider {
        let mut p = HunyuanImageProvider::new();
        let mut cfg = ProviderInstanceConfig { api_base: Some(format!("{api_base}/")), ..Default::default() };
        cfg.credentials.key_id = Some("AKIDtest".to_string());
        cfg.credentials.key_secret = Some("secret".to_string());
        cfg.credentials.region = Some("ap-guangzhou".to_string());
        p.configure(cfg);
        p
    }

    #[tokio::test]
    async fn fetch_image_bytes_errors_on_non_success_status() {
        // An expired/throttled presigned CDN URL returns a non-2xx with a short
        // error body. Draining it as if it were the image would store an error
        // blob and bill the customer for a broken asset, so the fetch must error.
        let cdn = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/expired.png"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_string("<?xml version=\"1.0\"?><Error><Code>AccessDenied</Code></Error>"),
            )
            .mount(&cdn)
            .await;

        let p = make_provider(&cdn.uri());
        let result = p.fetch_image_bytes(&format!("{}/expired.png", cdn.uri())).await;
        assert!(
            result.is_err(),
            "fetch_image_bytes must error on HTTP 403, got Ok with {:?} bytes",
            result.map(|b| b.len())
        );
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(), model: model.to_string(), n: 1, negative_prompt: None,
            seed: None, reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            size: Some("1024x1024".to_string()), aspect_ratio: None, quality: None, style: None,
            steps: None, guidance_scale: None, strength: None, response_format: "url".to_string(), extra: None,
        }
    }

    #[tokio::test]
    async fn submits_and_polls_with_tc3() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/h.png", image_server.uri());

        // Submit returns a JobId.
        Mock::given(method("POST"))
            .and(header("x-tc-action", "SubmitHunyuanImageJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Response": { "JobId": "hy-1", "RequestId": "r1" } })))
            .mount(&server)
            .await;
        // Query returns done with a result URL.
        Mock::given(method("POST"))
            .and(header("x-tc-action", "QueryHunyuanImageJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Response": { "JobStatusCode": "5", "JobStatusMsg": "done", "ResultImage": [image_url.clone()] }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"HUNYUANPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("hunyuan/hunyuan-image");
        let base = make_base("a tranquil zen garden", "hunyuan/hunyuan-image");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"HUNYUANPNG");

        let received = server.received_requests().await.unwrap();
        let submit = received.iter().find(|r| r.headers.get("x-tc-action").map(|v| v == "SubmitHunyuanImageJob").unwrap_or(false)).unwrap();
        let auth = submit.headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("TC3-HMAC-SHA256 Credential=AKIDtest/"), "auth: {auth}");
        assert!(submit.headers.get("x-tc-timestamp").is_some());
        let body: Value = serde_json::from_slice(&submit.body).unwrap();
        assert_eq!(body["Prompt"], "a tranquil zen garden");
        assert_eq!(body["Resolution"], "1024:1024");
    }

    /// `SubmitHunyuanImageJob.Resolution` takes `W:H`. The request validator
    /// accepts `WXH` as well as `WxH`, and the uppercase form used to reach
    /// Tencent verbatim (`768X1024`), which is not a Resolution value.
    /// @see <https://cloud.tencent.com/document/product/1729/105969>
    #[tokio::test]
    async fn resolution_converts_an_uppercase_size_separator() {
        let server = MockServer::start().await;
        // Fail the submit so generate() returns without polling; only the
        // outbound body matters here.
        Mock::given(method("POST"))
            .and(header("x-tc-action", "SubmitHunyuanImageJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Response": { "Error": { "Code": "InvalidParameterValue", "Message": "stop" }, "RequestId": "r1" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("hunyuan/hunyuan-image");
        let base = make_base("a paper crane", "hunyuan/hunyuan-image");
        let mut extras = make_extras();
        extras.size = Some("768X1024".to_string());
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let _ = provider.generate(&schema, &base, &extras, &materialized).await;

        let received = server.received_requests().await.unwrap();
        let submit = received
            .iter()
            .find(|r| r.headers.get("x-tc-action").map(|v| v == "SubmitHunyuanImageJob").unwrap_or(false))
            .expect("submit request");
        let body: Value = serde_json::from_slice(&submit.body).unwrap();
        assert_eq!(body["Resolution"], "768:1024");
    }

    // ─── Hunyuan Image 3.0 (aiart SubmitTextToImageJob) ─────────────────────

    const IMAGE3: &str = "hunyuan/hunyuan-image-3";

    fn requests_for<'a>(received: &'a [wiremock::Request], action: &str) -> Vec<&'a wiremock::Request> {
        received
            .iter()
            .filter(|r| r.headers.get("x-tc-action").map(|v| v == action).unwrap_or(false))
            .collect()
    }

    /// Mount SubmitTextToImageJob (only with aiart's version header, so a
    /// request sent as Image 2.0's action or version gets wiremock's 404).
    async fn mount_image3_submit(server: &MockServer, response: Value) {
        Mock::given(method("POST"))
            .and(header("x-tc-action", "SubmitTextToImageJob"))
            .and(header("x-tc-version", "2022-12-29"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Response": response })))
            .mount(server)
            .await;
    }

    async fn mount_image3_query(server: &MockServer, response: Value) {
        Mock::given(method("POST"))
            .and(header("x-tc-action", "QueryTextToImageJob"))
            .and(header("x-tc-version", "2022-12-29"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Response": response })))
            .mount(server)
            .await;
    }

    /// With no api_base, each product goes to its own host: Image 2.0 to
    /// hunyuan, Image 3.0 to aiart.
    #[test]
    fn default_endpoints_are_per_product() {
        let p = HunyuanImageProvider::new();
        assert_eq!(p.endpoint(HUNYUAN), "https://hunyuan.tencentcloudapi.com/");
        assert_eq!(p.endpoint(AIART), "https://aiart.tencentcloudapi.com/");
    }

    #[tokio::test]
    async fn image3_submits_text_to_image_job_on_aiart_and_polls_until_done() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/h3.png", image_server.uri());
        mount_image3_submit(&server, json!({ "JobId": "h3-1", "RequestId": "r1" })).await;
        // First poll: running (JobStatusCode 2) — must keep polling.
        Mock::given(method("POST"))
            .and(header("x-tc-action", "QueryTextToImageJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Response": { "JobStatusCode": "2", "JobStatusMsg": "处理中", "ResultImage": [] }
            })))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        mount_image3_query(&server, json!({
            "JobStatusCode": "5", "JobStatusMsg": "处理完成", "JobErrorCode": "", "JobErrorMsg": "",
            "ResultImage": [image_url.clone()], "ResultDetails": ["Success"]
        }))
        .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"HUNYUAN3PNG".to_vec()))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema(IMAGE3);
        let mut base = make_base("雨中, 竹林, 小路", IMAGE3);
        base.seed = Some(4_294_967_295);
        let mut extras = make_extras();
        extras.size = Some("1280x720".to_string());
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let out = provider.generate(&schema, &base, &extras, &materialized).await.expect("generate");
        assert_eq!(out.data, b"HUNYUAN3PNG");
        assert_eq!(out.metadata.get("job_id"), Some(&json!("h3-1")));

        let received = server.received_requests().await.unwrap();
        assert!(requests_for(&received, "SubmitHunyuanImageJob").is_empty(), "Image 3.0 must not call Image 2.0's action");
        assert_eq!(requests_for(&received, "QueryTextToImageJob").len(), 2, "a running job is polled again");
        let submit = requests_for(&received, "SubmitTextToImageJob")[0];
        let auth = submit.headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("TC3-HMAC-SHA256 Credential=AKIDtest/"), "auth: {auth}");
        assert!(auth.contains("/aiart/tc3_request"), "signed for the aiart service: {auth}");
        let body: Value = serde_json::from_slice(&submit.body).unwrap();
        assert_eq!(
            body,
            json!({ "Prompt": "雨中, 竹林, 小路", "Resolution": "1280:720", "Seed": 4_294_967_295u64 }),
            "text-only request carries no Images, NegativePrompt or ContentImage"
        );
        let query: Value = serde_json::from_slice(&requests_for(&received, "QueryTextToImageJob")[0].body).unwrap();
        assert_eq!(query, json!({ "JobId": "h3-1" }));
    }

    #[tokio::test]
    async fn image3_sends_reference_images_as_the_images_list() {
        let server = MockServer::start().await;
        // Fail the submit so generate() returns without polling.
        mount_image3_submit(&server, json!({ "Error": { "Code": "InvalidParameterValue", "Message": "stop" } })).await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema(IMAGE3);
        let base = make_base("a fox in the style of the references", IMAGE3);
        let mut extras = make_extras();
        extras.size = None;
        let materialized = MaterializedRequest {
            refs: vec![
                MaterializedRef { role: "init".to_string(), form: MaterializedRefForm::Base64("aW1n".to_string()) },
                MaterializedRef { role: "init".to_string(), form: MaterializedRefForm::Url("https://x/ref.png".to_string()) },
            ],
            cleanup: Cleanup::empty(),
        };

        let err = provider.generate(&schema, &base, &extras, &materialized).await.unwrap_err();
        assert!(err.to_string().contains("stop"), "vendor error surfaces: {err}");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests_for(&received, "SubmitTextToImageJob")[0].body).unwrap();
        assert_eq!(body["Images"], json!(["aW1n", "https://x/ref.png"]));
        assert!(body.get("ContentImage").is_none(), "ContentImage is Image 2.0's field: {body}");
        assert!(body.get("Resolution").is_none(), "no size → Tencent's default 1024:1024: {body}");
    }

    #[tokio::test]
    async fn image3_failed_job_reports_the_job_error_message() {
        let server = MockServer::start().await;
        mount_image3_submit(&server, json!({ "JobId": "h3-2" })).await;
        mount_image3_query(&server, json!({
            "JobStatusCode": "4", "JobStatusMsg": "处理失败",
            "JobErrorCode": "OperationDenied.ImageIllegalDetected", "JobErrorMsg": "图片包含违规内容", "ResultImage": []
        }))
        .await;

        let provider = make_provider(&server.uri());
        let err = provider
            .generate(&ref_schema(IMAGE3), &make_base("x", IMAGE3), &make_extras(), &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("图片包含违规内容"), "JobErrorMsg surfaces: {err}");
    }

    #[tokio::test]
    async fn image3_done_without_a_result_image_is_an_error() {
        let server = MockServer::start().await;
        mount_image3_submit(&server, json!({ "JobId": "h3-3" })).await;
        mount_image3_query(&server, json!({ "JobStatusCode": "5", "JobStatusMsg": "处理完成", "ResultImage": [""] })).await;

        let provider = make_provider(&server.uri());
        let err = provider
            .generate(&ref_schema(IMAGE3), &make_base("x", IMAGE3), &make_extras(), &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no ResultImage"), "{err}");
    }

    #[tokio::test]
    async fn image3_submit_without_a_job_id_is_an_error() {
        let server = MockServer::start().await;
        mount_image3_submit(&server, json!({ "RequestId": "r1" })).await;

        let provider = make_provider(&server.uri());
        let err = provider
            .generate(&ref_schema(IMAGE3), &make_base("x", IMAGE3), &make_extras(), &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("JobId"), "{err}");
        let received = server.received_requests().await.unwrap();
        assert!(requests_for(&received, "QueryTextToImageJob").is_empty(), "nothing to poll");
    }
}
