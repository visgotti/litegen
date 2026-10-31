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

/// A Tencent Cloud product the adapter calls: its TC3 service name (also the
/// `<service>.tencentcloudapi.com` host prefix) and its API version.
#[derive(Clone, Copy)]
struct Product {
    service: &'static str,
    version: &'static str,
}

/// Hunyuan Image 2.0 (`SubmitHunyuanImageJob`). Image 3.0's aiart actions
/// (`SubmitTextToImageJob`, `QueryTextToImageJob`) were deleted 2026-09-22
/// and its row retired; its successor is TokenHub's `hy-image-v3`.
/// @see <https://cloud.tencent.com/document/product/1668/88078> — aiart release 31: "删除接口：
///   … QueryTextToImageJob SubmitTextToImageJob …"
const HUNYUAN: Product = Product { service: "hunyuan", version: IMAGE_VERSION };

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
        _model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let region = self.region(&creds);
        let mut submit = json!({ "Prompt": base.prompt });
        if let Some(np) = base.negative_prompt.as_deref() {
            submit["NegativePrompt"] = Value::String(np.to_string());
        }
        if let Some(size) = extras.size.as_deref() {
            // Resolution takes `W:H`. The validator accepts a size spelled
            // `WxH` or `WXH` (`parse_size`), so convert either separator.
            submit["Resolution"] = Value::String(size.replace(['x', 'X'], ":"));
        }
        // `Seed`: unset is random and a positive seed is fixed, so 0 is left out.
        if let Some(seed) = base.seed.filter(|s| *s >= 1) {
            submit["Seed"] = Value::Number(seed.into());
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
        // Accepted: from here on a failure must not re-send the job (L1).
        extras.vendor_job.record(job_id);

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
                    return Ok(GenerationOutput { data: bytes, content_type: "image/png".to_string(), metadata, more: Vec::new() });
                }
                Some("4") => {
                    return Err(ProviderError::RequestFailed {
                        // Why, in Tencent's words: `JobErrorMsg` ("任务处理失败错误信息"),
                        // then `JobErrorCode`; `JobStatusMsg` is only "处理失败".
                        message: format!(
                            "Hunyuan job failed: {}",
                            ["JobErrorMsg", "JobErrorCode", "JobStatusMsg"]
                                .iter()
                                .find_map(|k| resp[*k].as_str().filter(|s| !s.is_empty()))
                                .unwrap_or("")
                        ),
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
    use crate::proxy::materializer::Cleanup;
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
            vendor_job: Default::default(),
            size: Some("1024x1024".to_string()), aspect_ratio: None, quality: None, style: None,
            steps: None, guidance_scale: None, strength: None, response_format: "url".to_string(), extra: None,
            edit: None, views: Vec::new(),
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
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
        // The vendor accepted the job before the poll: the router must never re-send it.
        assert_eq!(extras.vendor_job.get().as_deref(), Some("hy-1"));
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

    // ─── Hunyuan audit, 2026-10-30: H3, H6 ──────────────────────────────────

    /// H3: `Seed`: "不传：随机种子生成。 正数：固定种子生成。" (unset is random, a
    /// positive seed is fixed), so a seed of 0 is left out instead of sent.
    /// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/hunyuan/v20230901/models.go> (2026-10-30)
    #[tokio::test]
    async fn only_a_positive_seed_is_sent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-tc-action", "SubmitHunyuanImageJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Response": { "Error": { "Code": "InvalidParameterValue", "Message": "stop" }, "RequestId": "r1" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("hunyuan/hunyuan-image");
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
        for seed in [0, 7] {
            let mut base = make_base("a paper crane", "hunyuan/hunyuan-image");
            base.seed = Some(seed);
            let _ = provider.generate(&schema, &base, &make_extras(), &materialized).await;
        }

        let seeds: Vec<Value> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap()["Seed"].clone())
            .collect();
        assert_eq!(seeds, vec![Value::Null, json!(7)]);
    }

    /// H6: a failed job says why — `JobErrorMsg`, then `JobErrorCode` — not
    /// the generic `JobStatusMsg` ("处理失败").
    #[tokio::test]
    async fn a_failed_job_says_why() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-tc-action", "SubmitHunyuanImageJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Response": { "JobId": "hy-2" } })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(header("x-tc-action", "QueryHunyuanImageJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Response": { "JobStatusCode": "4", "JobStatusMsg": "处理失败", "JobErrorCode": "FailedOperation.ImageRiskCheck", "JobErrorMsg": "输入图片审核不通过" }
            })))
            .mount(&server)
            .await;
        let err = make_provider(&server.uri())
            .generate(&ref_schema("hunyuan/hunyuan-image"), &make_base("x", "hunyuan/hunyuan-image"), &make_extras(), &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() })
            .await
            .unwrap_err();
        assert!(matches!(&err, ProviderError::RequestFailed { message, .. } if message.contains("输入图片审核不通过")), "{err:?}");
    }
}
