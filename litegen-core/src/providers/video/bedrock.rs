use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{sigv4, AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, CredentialPool, HealthCheckResult, ProviderError, ProviderInstanceConfig, VideoExtras,
    VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Amazon Bedrock video provider: Luma Ray 2 (`luma.ray-v2:0`).
///
/// Asynchronous: `POST /async-invoke` {modelId, modelInput, outputDataConfig}
/// answers an `invocationArn`; `GET /async-invoke/{arn}` says InProgress |
/// Completed | Failed. The MP4 is written to the caller's S3 bucket (the
/// `s3_output_uri` option or credential field), so a finished job is read
/// back from there with the same AWS credential (SigV4 for `s3`) and kept as
/// bytes: there is no link to hand out. SigV4 for `bedrock`.
///
/// @see <https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_StartAsyncInvoke.html> (2026-11-10)
///   Verbatim: "POST /async-invoke HTTP/1.1 ... {\"clientRequestToken\",\"modelId\",\"modelInput\",\"outputDataConfig\",\"tags\"}"
/// @see <https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_GetAsyncInvoke.html> (2026-11-10)
///   Verbatim: "status … Valid Values: InProgress | Completed | Failed"
/// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-luma.md> (2026-11-10)
///   Verbatim: "The model output will be placed in the specified output Amazon S3 bucket"
/// @see <https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html> (2026-11-10)
///   Verbatim: "The x-amz-content-sha256 header is required for all AWS Signature Version 4 requests."
///
/// Nova Reel, which this adapter built before, reached end of life on
/// 2026-09-30 ("Models that have reached EOL",
/// <https://docs.aws.amazon.com/bedrock/latest/userguide/model-lifecycle-legacy.md>).
pub struct BedrockVideoProvider {
    config: Option<ProviderInstanceConfig>,
    cred_pool: Option<CredentialPool>,
    auth: AuthSpec,
    client: Client,
}

/// Ray 2's one Region: "bedrock-runtime | luma.ray-v2:0 |
/// https://bedrock-runtime.us-west-2.amazonaws.com | Not supported | Not
/// supported" (no Geo or Global inference id). The output bucket must be
/// there too ("the bucket exists in the same region as your Bedrock service").
/// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-luma-ai-ray2.md> (2026-11-10)
const RAY_REGION: &str = "us-west-2";

/// The most a finished video may weigh when read back ("Generated videos
/// range from 5-50 MB").
const MAX_VIDEO_BYTES: usize = 1024 * 1024 * 1024;

/// The sha256 of an empty body: an S3 GET's `x-amz-content-sha256`.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// An `s3://bucket[/prefix]` location: the bucket a DNS-safe S3 name ("3 to
/// 63 characters … lowercase letters, numbers, dots, and hyphens"), the
/// prefix plain characters only, so it can go into a host and a signed path
/// as it is.
/// @see <https://docs.aws.amazon.com/AmazonS3/latest/userguide/bucketnamingrules.html> (2026-11-10)
#[derive(Debug, Clone, PartialEq)]
struct S3Location {
    bucket: String,
    /// The key prefix, without a leading or trailing slash ("" for the bucket's root).
    prefix: String,
}

impl S3Location {
    fn parse(uri: &str) -> Option<Self> {
        let rest = uri.strip_prefix("s3://")?;
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        let bucket_ok = (3..=63).contains(&bucket.len())
            && bucket.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
            && bucket.starts_with(|c: char| c.is_ascii_alphanumeric())
            && bucket.ends_with(|c: char| c.is_ascii_alphanumeric());
        let prefix = prefix.trim_matches('/');
        let prefix_ok = prefix.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.~/".contains(&b))
            && !prefix.split('/').any(|seg| seg.is_empty() && !prefix.is_empty() || seg == "." || seg == "..");
        (bucket_ok && prefix_ok).then(|| Self { bucket: bucket.to_string(), prefix: prefix.to_string() })
    }
}

impl std::fmt::Display for S3Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.prefix.is_empty() { write!(f, "s3://{}", self.bucket) } else { write!(f, "s3://{}/{}", self.bucket, self.prefix) }
    }
}

/// A key's `<Key>` values in a ListObjectsV2 answer, XML-unescaped.
/// @see <https://docs.aws.amazon.com/AmazonS3/latest/API/API_ListObjectsV2.html> (2026-11-10)
fn listed_keys(xml: &str) -> Vec<String> {
    xml.split("<Key>")
        .skip(1)
        .filter_map(|part| part.split_once("</Key>").map(|(k, _)| k))
        .map(|k| k.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&"))
        .collect()
}

impl BedrockVideoProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (S3 reads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(120), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            cred_pool: None,
            auth: AuthSpec::AwsSigV4 { service: "bedrock".into(), default_region: RAY_REGION.into() },
            client: Self::api_client(&Default::default()),
        }
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("bedrock".into()))?;
        if let Some(pool) = &self.cred_pool {
            return Ok(base.with_signing(pool.next()));
        }
        Ok(base)
    }

    /// `api_base` (a proxy, or a test's mock), else Bedrock's runtime host in
    /// `region`.
    fn host_base(&self, region: &str) -> String {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| format!("https://bedrock-runtime.{region}.amazonaws.com"))
    }

    fn option(&self, key: &str) -> Option<String> {
        self.config.as_ref().and_then(|c| c.options.as_ref()).and_then(|o| o.get(key)).and_then(|v| v.as_str()).map(str::to_string)
    }

    /// Where the output goes: the operator's `s3_output_uri` option, else the
    /// credential's (a BYO credential's `s3_output_uri` field).
    fn s3_output_uri(&self, creds: &ProviderCredentials) -> Option<String> {
        self.option("s3_output_uri").or_else(|| creds.extra.get("s3_output_uri").cloned())
    }

    /// The URL of an object (or, with no key, the bucket) in `region`:
    /// virtual-hosted, or path-style for a bucket whose dots would break the
    /// TLS name, or under the `s3_api_base` option (an S3-compatible proxy, or
    /// a test's mock).
    fn s3_url(&self, region: &str, bucket: &str, key: &str) -> Result<reqwest::Url, ProviderError> {
        let path = if key.is_empty() { String::new() } else { format!("/{key}") };
        let url = match self.option("s3_api_base") {
            Some(base) => format!("{}/{bucket}{path}", base.trim_end_matches('/')),
            None if bucket.contains('.') => format!("https://s3.{region}.amazonaws.com/{bucket}{path}"),
            None => format!("https://{bucket}.s3.{region}.amazonaws.com{}", if path.is_empty() { "/" } else { &path }),
        };
        reqwest::Url::parse(&url).map_err(|e| ProviderError::InvalidRequest(format!("bad S3 url: {e}")))
    }

    /// The Bedrock model for a litegen id; `None` for an id with no arm, which
    /// is refused rather than run as another model (L13).
    fn resolve_model(model_id: &str) -> Option<&'static str> {
        match model_id {
            "bedrock/luma.ray-v2:0" => Some("luma.ray-v2:0"),
            _ => None,
        }
    }

    /// A keyframe as Ray 2 takes it: {"type": "image", "source": {"type":
    /// "base64", "media_type": …, "data": …}}, JPEG or PNG ("Images must be in
    /// supported formats (JPEG, PNG)").
    fn keyframe(form: &MaterializedRefForm) -> Result<Value, ProviderError> {
        use base64::Engine;
        let MaterializedRefForm::Base64(data) = form else {
            return Err(ProviderError::InvalidRequest("bedrock Ray 2 takes a keyframe as base64 image data".into()));
        };
        let head: String = data.chars().take(32).collect();
        let bytes = base64::engine::general_purpose::STANDARD.decode(&head[..head.len() / 4 * 4]).unwrap_or_default();
        match crate::util::content::image_content_type(&bytes) {
            Some(media_type @ ("image/jpeg" | "image/png")) => Ok(json!({ "type": "image", "source": { "type": "base64", "media_type": media_type, "data": data } })),
            other => Err(ProviderError::InvalidRequest(format!(
                "bedrock Ray 2 takes a JPEG or PNG keyframe, not {}",
                other.unwrap_or("an unrecognised image")
            ))),
        }
    }

    /// A signed S3 GET (service `s3`, `region`): its answer, unread.
    async fn s3_get(&self, creds: &ProviderCredentials, region: &str, url: reqwest::Url) -> Result<reqwest::Response, ProviderError> {
        let signed = sigv4::sign(creds, "s3", region, "GET", &url, &[("x-amz-content-sha256".to_string(), EMPTY_SHA256.to_string())], b"")?;
        // Its own client: the operator's extra_headers are for Bedrock's API.
        // No redirects: the signature and any session token travel in
        // `x-amz-*` headers, which a redirect to another host would carry.
        let mut req = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ProviderError::RequestFailed { message: e.to_string(), status_code: None, provider_error: None, retryable: true })?
            .get(url)
            .header("x-amz-content-sha256", EMPTY_SHA256);
        for (k, v) in &signed {
            req = req.header(k, v);
        }
        req.send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("S3 read failed: {}", e.without_url()),
            status_code: None,
            provider_error: None,
            retryable: true,
        })
    }

    /// A finished job's MP4, read back from the caller's bucket: at
    /// `<the job's s3Uri>/output.mp4`, else the first `.mp4` listed under that
    /// prefix. `Ok(Err(reason))` when the job is done but litegen may not read
    /// its output (a verdict: asking again won't help); a passing failure (a
    /// 5xx, a dropped connection) is a retryable error.
    async fn read_output(&self, creds: &ProviderCredentials, region: &str, out: &S3Location) -> Result<Result<Vec<u8>, String>, ProviderError> {
        let unreadable = |status: u16| {
            format!("Bedrock finished, but litegen may not read its output under {out} (S3 answered HTTP {status}): grant the credential s3:GetObject and s3:ListBucket on that bucket")
        };
        let passing = |status: reqwest::StatusCode| status.is_server_error() || matches!(status.as_u16(), 408 | 429);
        let key = if out.prefix.is_empty() { "output.mp4".to_string() } else { format!("{}/output.mp4", out.prefix) };
        let mut resp = self.s3_get(creds, region, self.s3_url(region, &out.bucket, &key)?).await?;
        if matches!(resp.status().as_u16(), 403 | 404) {
            // Not there (or, without s3:ListBucket, "403" for a missing key): list the prefix.
            let mut list = self.s3_url(region, &out.bucket, "")?;
            list.query_pairs_mut().append_pair("list-type", "2").append_pair("prefix", &if out.prefix.is_empty() { String::new() } else { format!("{}/", out.prefix) });
            let listed = self.s3_get(creds, region, list).await?;
            let status = listed.status();
            if passing(status) {
                return Err(ProviderError::RequestFailed { message: format!("S3 list answered HTTP {}", status.as_u16()), status_code: Some(status.as_u16()), provider_error: None, retryable: true });
            }
            if !status.is_success() {
                return Ok(Err(unreadable(status.as_u16())));
            }
            let xml = listed.text().await.unwrap_or_default();
            let Some(found) = listed_keys(&xml).into_iter().find(|k| k.ends_with(".mp4")) else {
                return Ok(Err(format!("Bedrock finished, but no .mp4 is under {out}")));
            };
            if !found.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.~/".contains(&b)) {
                return Ok(Err(format!("Bedrock's output key {found:?} under {out} is not one litegen can read")));
            }
            resp = self.s3_get(creds, region, self.s3_url(region, &out.bucket, &found)?).await?;
        }
        let status = resp.status();
        if passing(status) {
            return Err(ProviderError::RequestFailed { message: format!("S3 read answered HTTP {}", status.as_u16()), status_code: Some(status.as_u16()), provider_error: None, retryable: true });
        }
        if !status.is_success() {
            return Ok(Err(unreadable(status.as_u16())));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("S3 read broke off: {}", e.without_url()),
            status_code: None,
            provider_error: None,
            retryable: true,
        })? {
            if bytes.len() + chunk.len() > MAX_VIDEO_BYTES {
                return Ok(Err(format!("Bedrock's output under {out} is over {MAX_VIDEO_BYTES} bytes")));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Ok(bytes))
    }
}

impl Default for BedrockVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for BedrockVideoProvider {
    fn name(&self) -> &str {
        "bedrock"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.credentials.credential_sets.is_empty() {
            self.cred_pool = Some(CredentialPool::shared(config.credentials.credential_sets.clone()));
        }
        self.client = Self::api_client(&config.extra_headers);
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.cred_pool.is_some())
    }

    /// Submit `POST /async-invoke`, signed for us-west-2 (Ray 2's one Region)
    /// whatever region the credential names.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let native = Self::resolve_model(&model.id)
            .ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "bedrock".into() })?;
        let creds = self.creds()?;
        let s3_uri = self.s3_output_uri(&creds).ok_or_else(|| {
            ProviderError::InvalidRequest(
                "bedrock video writes its output to your S3 bucket: set `s3_output_uri` (s3://bucket/prefix in us-west-2) on the credential or in the provider's options".into(),
            )
        })?;
        // Checked here, before Bedrock runs a job litegen could not read back.
        if S3Location::parse(&s3_uri).is_none() {
            return Err(ProviderError::InvalidRequest(format!(
                "bedrock `s3_output_uri` must be s3://<bucket>[/<prefix>], a lowercase bucket name and a prefix of letters, digits and - _ . ~ / only; got {s3_uri:?}"
            )));
        }

        // "keyframes – (object) Define start (frame0) and/or end (frame1) keyframes".
        let mut keyframes = serde_json::Map::new();
        for r in &materialized.refs {
            match r.role.as_str() {
                "first_frame" | "init" => keyframes.insert("frame0".into(), Self::keyframe(&r.form)?),
                "last_frame" => keyframes.insert("frame1".into(), Self::keyframe(&r.form)?),
                _ => None,
            };
        }
        let looping = extras.r#loop;
        if looping == Some(true) && keyframes.contains_key("frame1") {
            return Err(ProviderError::InvalidRequest("bedrock Ray 2: a loop can't end on a last frame (\"Loop is not supported for keyframes\")".into()));
        }
        // "duration – (enum) … ("5s", "9s")".
        let duration = match extras.duration_seconds.round() as i64 {
            5 => "5s",
            9 => "9s",
            other => return Err(ProviderError::InvalidRequest(format!("bedrock Ray 2 makes 5 s or 9 s videos, not {other} s"))),
        };
        let resolution = extras.resolution.clone().or_else(|| match model.params.get("resolution") {
            Some(crate::capabilities::schema::ParamSpec::String(s)) => s.default.clone(),
            _ => None,
        });
        let mut input = json!({ "prompt": base.prompt, "duration": duration });
        if let Some(aspect) = &extras.aspect_ratio {
            input["aspect_ratio"] = json!(aspect);
        }
        if let Some(resolution) = resolution {
            input["resolution"] = json!(resolution);
        }
        if let Some(looping) = looping {
            input["loop"] = json!(looping);
        }
        if !keyframes.is_empty() {
            input["keyframes"] = Value::Object(keyframes);
        }
        let body = json!({
            "modelId": crate::providers::wire_model(self.config.as_ref(), &model.id, native),
            "modelInput": input,
            "outputDataConfig": { "s3OutputDataConfig": { "s3Uri": s3_uri } },
        });
        let body_bytes = serde_json::to_vec(&body).map_err(|e| ProviderError::InvalidRequest(e.to_string()))?;

        let url = reqwest::Url::parse(&format!("{}/async-invoke", self.host_base(RAY_REGION)))
            .map_err(|e| ProviderError::InvalidRequest(format!("bad bedrock url: {e}")))?;
        let signed = sigv4::sign(&creds, "bedrock", RAY_REGION, "POST", &url, &[("content-type".to_string(), "application/json".to_string())], &body_bytes)?;
        let mut req = self.client.post(url).header("content-type", "application/json");
        for (k, v) in &signed {
            req = req.header(k, v);
        }
        let resp = crate::providers::inject_trace_headers(req.body(body_bytes))
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Bedrock async-invoke failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: crate::providers::may_resend_after(&e, true),
            })?;
        // The status first (a gateway's 5xx page is retried), and a 429's
        // `Retry-After` reaches the retry policy.
        let data = crate::providers::read_vendor_json(resp, "Bedrock").await?;
        let arn = data["invocationArn"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Bedrock response missing invocationArn".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;
        Ok(VideoGenerationHandle { provider_job_id: arn.to_string(), provider: "bedrock".to_string(), model: model.id.clone() })
    }

    /// `GET /async-invoke/{arn}` in the job's Region; a Completed job's MP4
    /// read back from its `outputDataConfig.s3OutputDataConfig.s3Uri`.
    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        self.poll_status_gated(handle, &crate::providers::AlwaysFetch).await
    }

    /// The clip is read from S3 only once `gate` says this observer keeps it.
    async fn poll_status_gated(
        &self,
        handle: &VideoGenerationHandle,
        gate: &dyn crate::providers::FetchGate,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        // The job's Region from its ARN ("arn:aws:bedrock:<region>:…"),
        // checked like any region that goes into a host.
        let region = crate::providers::auth::normalize_aws_region(handle.provider_job_id.split(':').nth(3), RAY_REGION);
        // Raw arn in the path: reqwest sends ':' literally and SigV4 encodes it
        // to %3A, matching how AWS canonicalizes the received path.
        let url = reqwest::Url::parse(&format!("{}/async-invoke/{}", self.host_base(&region), handle.provider_job_id))
            .map_err(|e| ProviderError::InvalidRequest(format!("bad bedrock url: {e}")))?;
        let signed = sigv4::sign(&creds, "bedrock", &region, "GET", &url, &[], b"")?;
        let mut req = self.client.get(url);
        for (k, v) in &signed {
            req = req.header(k, v);
        }
        let resp = crate::providers::inject_trace_headers(req).send().await.map_err(|e| ProviderError::RequestFailed {
            message: e.to_string(),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        // A 4xx is the answer (a 404 is a job that can't be found), a 5xx is asked again.
        let data = super::read_poll_json(resp, "bedrock").await?;

        let done = |status, error: Option<String>| VideoGenerationPollResult {
            status,
            progress: 100,
            video_url: None,
            video_data: None,
            content_type: None,
            error,
            metadata: HashMap::new(),
        };
        // "Valid Values: InProgress | Completed | Failed".
        // @see <https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_GetAsyncInvoke.html> (2026-11-10)
        match data["status"].as_str() {
            Some("InProgress") => Ok(VideoGenerationPollResult { progress: 50, ..done(GenerationStatus::Processing, None) }),
            Some("Failed") => Ok(done(
                GenerationStatus::Failed,
                Some(data["failureMessage"].as_str().unwrap_or("Bedrock failed the job without a message").to_string()),
            )),
            Some("Completed") => {
                let Some(out) = data["outputDataConfig"]["s3OutputDataConfig"]["s3Uri"].as_str().and_then(S3Location::parse) else {
                    return Ok(done(GenerationStatus::Failed, Some(format!("Bedrock finished without an S3 location litegen can read: {}", data["outputDataConfig"]))));
                };
                if gate.ask().await == crate::providers::Fetch::StandAside {
                    return Ok(super::standing_aside());
                }
                // The bucket "exists in the same region as your Bedrock service".
                match self.read_output(&creds, &region, &out).await? {
                    Ok(bytes) => Ok(VideoGenerationPollResult {
                        video_data: Some(bytes),
                        content_type: Some("video/mp4".into()),
                        ..done(GenerationStatus::Completed, None)
                    }),
                    Err(reason) => Ok(done(GenerationStatus::Failed, Some(reason))),
                }
            }
            other => Ok(super::unknown_state("bedrock", other)),
        }
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

    /// The video lands in the caller's private bucket: no link to hand out.
    fn hands_out_links(&self, _model: &ModelSchema) -> bool {
        false
    }

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Bedrock video provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Bedrock video provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ARN: &str = "arn:aws:bedrock:us-west-2:123456789012:async-invoke/abc123";
    /// sha256 of an empty body: what an S3 GET sends as x-amz-content-sha256.
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load").get(id).expect("model").clone()
    }

    /// A credential naming us-east-1, to show Ray 2 is called in us-west-2 anyway.
    fn make_provider(api_base: &str, s3_api_base: &str, s3_output_uri: Option<&str>) -> BedrockVideoProvider {
        let mut p = BedrockVideoProvider::new();
        let mut options = json!({ "s3_api_base": s3_api_base });
        if let Some(uri) = s3_output_uri {
            options["s3_output_uri"] = json!(uri);
        }
        let mut cfg = ProviderInstanceConfig { api_base: Some(api_base.to_string()), options: Some(options), ..Default::default() };
        cfg.credentials.key_id = Some("AKIDEXAMPLE".to_string());
        cfg.credentials.key_secret = Some("secretkey".to_string());
        cfg.credentials.region = Some("us-east-1".to_string());
        p.configure(cfg);
        p
    }

    fn make_base(prompt: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(), model: RAY.to_string(), n: 1, negative_prompt: None,
            seed: None, reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    const RAY: &str = "bedrock/luma.ray-v2:0";

    fn extras(aspect: Option<&str>, resolution: Option<&str>, duration: f64, looping: Option<bool>) -> VideoExtras {
        VideoExtras {
            duration_seconds: duration, aspect_ratio: aspect.map(str::to_string), resolution: resolution.map(str::to_string),
            fps: None, extra: None, camera: None, audio: None, r#loop: looping, source: None, direction: None, time_range: None, replace: None,
        }
    }

    fn frames(frames: &[(&str, &[u8])]) -> MaterializedRequest {
        use base64::Engine;
        MaterializedRequest {
            refs: frames
                .iter()
                .map(|(role, bytes)| MaterializedRef {
                    role: role.to_string(),
                    form: MaterializedRefForm::Base64(base64::engine::general_purpose::STANDARD.encode(bytes)),
                    time_s: None,
                })
                .collect(),
            cleanup: Cleanup::empty(),
        }
    }

    fn none() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    async fn accepting_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/async-invoke"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "invocationArn": ARN })))
            .mount(&server)
            .await;
        server
    }

    fn sent_body(r: &wiremock::Request) -> Value {
        serde_json::from_slice(&r.body).unwrap()
    }

    /// L13: every catalog row has its own arm, and an id with none is refused
    /// rather than run as another model.
    #[test]
    fn every_catalog_row_resolves_and_an_unknown_id_does_not() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        let rows: Vec<_> = catalog.for_provider("bedrock").filter(|m| m.media_type == crate::capabilities::MediaType::Video).collect();
        assert!(!rows.is_empty());
        for m in rows {
            assert!(BedrockVideoProvider::resolve_model(&m.id).is_some(), "{} has no arm", m.id);
        }
        for id in ["bedrock/amazon.nova-reel-v1:1", "bedrock/luma.ray-v9:0", "luma.ray-v2:0x"] {
            assert!(BedrockVideoProvider::resolve_model(id).is_none(), "{id}");
        }
    }

    /// `POST /async-invoke` {modelId, modelInput, outputDataConfig}, signed for
    /// us-west-2 (Ray 2's one Region) whatever the credential names: duration
    /// "5s" | "9s" and resolution always, aspect_ratio and loop when asked.
    /// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-luma.md> (2026-11-10)
    #[tokio::test]
    async fn ray_2_is_submitted_signed_for_us_west_2_with_its_own_fields() {
        let server = accepting_server().await;
        let provider = make_provider(&server.uri(), &server.uri(), Some("s3://my-bucket/litegen/"));
        let schema = ref_schema(RAY);
        let handle = provider.generate(&schema, &make_base("a whale in space"), &extras(Some("21:9"), Some("540p"), 9.0, Some(true)), &none()).await.unwrap();
        assert_eq!((handle.provider_job_id.as_str(), handle.model.as_str()), (ARN, RAY));
        let _ = provider.generate(&schema, &make_base("a whale in space"), &extras(None, None, 5.0, None), &none()).await.unwrap();

        let sent = server.received_requests().await.unwrap();
        let auth = sent[0].headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/") && auth.contains("/us-west-2/bedrock/aws4_request"), "{auth}");
        assert_eq!(sent_body(&sent[0]), json!({
            "modelId": "luma.ray-v2:0",
            "modelInput": { "prompt": "a whale in space", "aspect_ratio": "21:9", "duration": "9s", "resolution": "540p", "loop": true },
            "outputDataConfig": { "s3OutputDataConfig": { "s3Uri": "s3://my-bucket/litegen/" } }
        }));
        assert_eq!(sent_body(&sent[1])["modelInput"], json!({ "prompt": "a whale in space", "duration": "5s", "resolution": "720p" }));
    }

    /// keyframes "frame0" and "frame1", each {"type": "image", "source":
    /// {"type": "base64", "media_type": …, "data": …}}, JPEG or PNG ("Images
    /// must be in supported formats (JPEG, PNG)"); a last frame alone is
    /// "end frames". Anything else is refused before any call, as is a loop
    /// with a last frame ("Loop is not supported for keyframes").
    /// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-luma.md> (2026-11-10)
    #[tokio::test]
    async fn keyframes_go_as_base64_with_their_own_type() {
        let server = accepting_server().await;
        let provider = make_provider(&server.uri(), &server.uri(), Some("s3://my-bucket/litegen/"));
        let schema = ref_schema(RAY);
        let png = b"\x89PNG\r\n\x1a\nrest";
        let jpeg = b"\xff\xd8\xff\xe0rest";
        provider.generate(&schema, &make_base("a tiger"), &extras(None, None, 5.0, None), &frames(&[("last_frame", jpeg), ("first_frame", png)])).await.unwrap();
        provider.generate(&schema, &make_base("a tiger"), &extras(None, None, 5.0, None), &frames(&[("last_frame", jpeg)])).await.unwrap();
        let sent = server.received_requests().await.unwrap();
        use base64::Engine;
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        assert_eq!(sent_body(&sent[0])["modelInput"]["keyframes"], json!({
            "frame0": { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": b64(png) } },
            "frame1": { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": b64(jpeg) } }
        }));
        assert_eq!(sent_body(&sent[1])["modelInput"]["keyframes"].as_object().map(|k| k.keys().cloned().collect::<Vec<_>>()), Some(vec!["frame1".to_string()]));

        for (refs, looping) in [
            (frames(&[("first_frame", b"RIFF\x00\x00\x00\x00WEBPVP8 ")]), None),
            (frames(&[("first_frame", png), ("last_frame", jpeg)]), Some(true)),
        ] {
            let err = provider.generate(&schema, &make_base("a tiger"), &extras(None, None, 5.0, looping), &refs).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 2, "a refused request reached Bedrock");
    }

    /// The output goes to the caller's bucket: without one, or with one that
    /// isn't `s3://<bucket>[/<prefix>]` in plain characters, the request is
    /// refused before any call. So is an id with no arm.
    #[tokio::test]
    async fn a_request_without_a_usable_bucket_is_refused_before_any_call() {
        let server = accepting_server().await;
        let schema = ref_schema(RAY);
        for uri in [None, Some("https://my-bucket.s3.amazonaws.com/x"), Some("s3://Bad_Bucket/x"), Some("s3://ok-bucket/a b/"), Some("s3://"), Some("s3://ok-bucket/../x")] {
            let err = make_provider(&server.uri(), &server.uri(), uri)
                .generate(&schema, &make_base("x"), &extras(None, None, 5.0, None), &none())
                .await
                .unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{uri:?}: {err:?}");
        }
        let mut other = schema.clone();
        other.id = "bedrock/amazon.nova-reel-v1:1".into();
        let err = make_provider(&server.uri(), &server.uri(), Some("s3://my-bucket/litegen/"))
            .generate(&other, &make_base("x"), &extras(None, None, 5.0, None), &none())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::ModelNotSupported { .. }), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    fn completed(s3_uri: &str) -> Value {
        json!({ "invocationArn": ARN, "modelArn": "arn:aws:bedrock:us-west-2::foundation-model/luma.ray-v2:0", "status": "Completed",
            "outputDataConfig": { "s3OutputDataConfig": { "s3Uri": s3_uri } } })
    }

    fn handle() -> VideoGenerationHandle {
        VideoGenerationHandle { provider_job_id: ARN.into(), provider: "bedrock".into(), model: RAY.into() }
    }

    /// A gate that always says another observer keeps the video.
    struct Refuse;

    #[async_trait::async_trait]
    impl crate::providers::FetchGate for Refuse {
        async fn ask(&self) -> crate::providers::Fetch {
            crate::providers::Fetch::StandAside
        }
    }

    /// Review 2026-11-11 P4: an observer that does not keep the clip reads
    /// nothing from the bucket.
    #[tokio::test]
    async fn a_gate_that_says_stand_aside_downloads_nothing() {
        let bedrock = MockServer::start().await;
        let s3 = MockServer::start().await;
        Mock::given(method("GET")).and(path(format!("/async-invoke/{ARN}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(completed("s3://my-bucket/litegen/abc123")))
            .mount(&bedrock).await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"MP4BYTES".to_vec())).mount(&s3).await;
        let poll = make_provider(&bedrock.uri(), &s3.uri(), Some("s3://my-bucket/litegen/")).poll_status_gated(&handle(), &Refuse).await.unwrap();
        assert_eq!((poll.status, poll.video_data.is_none(), poll.error.is_none()), (GenerationStatus::Processing, true, true));
        assert!(s3.received_requests().await.unwrap().is_empty(), "nothing read from the bucket");
    }

    /// A finished job's MP4 is read back from the caller's bucket with the same
    /// credential (SigV4 for `s3`, us-west-2, x-amz-content-sha256) at
    /// `<the job's s3Uri>/output.mp4`, and kept as bytes: the `s3://` location
    /// is no link a client could fetch.
    #[tokio::test]
    async fn a_finished_job_is_read_back_from_the_bucket() {
        let bedrock = MockServer::start().await;
        let s3 = MockServer::start().await;
        Mock::given(method("GET")).and(path(format!("/async-invoke/{ARN}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(completed("s3://my-bucket/litegen/abc123")))
            .mount(&bedrock).await;
        Mock::given(method("GET")).and(path("/my-bucket/litegen/abc123/output.mp4"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"MP4BYTES".to_vec()).insert_header("content-type", "video/mp4"))
            .mount(&s3).await;
        let poll = make_provider(&bedrock.uri(), &s3.uri(), Some("s3://my-bucket/litegen/")).poll_status(&handle()).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_data.as_deref(), Some(b"MP4BYTES".as_slice()));
        assert_eq!((poll.video_url, poll.content_type.as_deref(), poll.progress), (None, Some("video/mp4"), 100));

        let asked = bedrock.received_requests().await.unwrap();
        let auth = asked[0].headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.contains("/us-west-2/bedrock/aws4_request"), "{auth}");
        let read = s3.received_requests().await.unwrap();
        let auth = read[0].headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.contains("/us-west-2/s3/aws4_request") && auth.contains("x-amz-content-sha256"), "{auth}");
        assert_eq!(read[0].headers.get("x-amz-content-sha256").unwrap(), EMPTY_SHA256);
    }

    /// The read-back carries an AWS signature and, with a session, its token
    /// in `x-amz-*` headers, which a client keeps across a redirect to
    /// another host. S3 answers a GET with the object, or an error, and never
    /// needs a redirect followed: one is refused, and nothing reaches the
    /// host it names.
    #[tokio::test]
    async fn a_redirect_from_the_bucket_is_not_followed() {
        let bedrock = MockServer::start().await;
        let s3 = MockServer::start().await;
        let elsewhere = MockServer::start().await;
        Mock::given(method("GET")).and(path(format!("/async-invoke/{ARN}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(completed("s3://my-bucket/litegen/abc123")))
            .mount(&bedrock).await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", format!("{}/collect", elsewhere.uri()).as_str()))
            .mount(&s3).await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"MP4BYTES".to_vec())).mount(&elsewhere).await;

        let poll = make_provider(&bedrock.uri(), &s3.uri(), Some("s3://my-bucket/litegen/")).poll_status(&handle()).await;
        assert!(elsewhere.received_requests().await.unwrap().is_empty(), "the signed read followed the redirect");
        assert!(poll.is_err() || poll.as_ref().is_ok_and(|p| p.status != GenerationStatus::Completed), "{poll:?}");
    }

    /// Where `output.mp4` isn't, the job's prefix is listed (ListObjectsV2)
    /// and its MP4 read; one litegen may not read fails the job, naming the
    /// permission (the job has finished: asking again won't help).
    #[tokio::test]
    async fn a_finished_job_elsewhere_under_its_prefix_is_found_and_an_unreadable_one_fails() {
        let bedrock = MockServer::start().await;
        let s3 = MockServer::start().await;
        Mock::given(method("GET")).and(path(format!("/async-invoke/{ARN}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(completed("s3://my-bucket/litegen/abc123")))
            .mount(&bedrock).await;
        Mock::given(method("GET")).and(path("/my-bucket/litegen/abc123/output.mp4")).respond_with(ResponseTemplate::new(404)).mount(&s3).await;
        Mock::given(method("GET")).and(path("/my-bucket")).and(query_param("list-type", "2")).and(query_param("prefix", "litegen/abc123/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "<?xml version=\"1.0\"?><ListBucketResult><Contents><Key>litegen/abc123/manifest.json</Key></Contents><Contents><Key>litegen/abc123/clip-1.mp4</Key></Contents></ListBucketResult>",
            ))
            .mount(&s3).await;
        Mock::given(method("GET")).and(path("/my-bucket/litegen/abc123/clip-1.mp4"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"CLIP".to_vec()))
            .mount(&s3).await;
        let poll = make_provider(&bedrock.uri(), &s3.uri(), Some("s3://my-bucket/litegen/")).poll_status(&handle()).await.unwrap();
        assert_eq!((poll.status, poll.video_data.as_deref()), (GenerationStatus::Completed, Some(b"CLIP".as_slice())));

        let locked = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(403).set_body_string("<Error><Code>AccessDenied</Code></Error>")).mount(&locked).await;
        let poll = make_provider(&bedrock.uri(), &locked.uri(), Some("s3://my-bucket/litegen/")).poll_status(&handle()).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Failed);
        assert!(poll.error.as_deref().is_some_and(|e| e.contains("s3:GetObject") && e.contains("s3://my-bucket/litegen/abc123")), "{:?}", poll.error);
    }

    /// Every state Bedrock documents ("InProgress | Completed | Failed"), an
    /// undocumented one, and the poll's own failures.
    /// @see <https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_GetAsyncInvoke.html> (2026-11-10)
    #[tokio::test]
    async fn a_poll_says_what_bedrock_said() {
        let answer = |status: &str| json!({ "invocationArn": ARN, "status": status, "failureMessage": "Content filtered" });
        for (reply, want) in [
            (ResponseTemplate::new(200).set_body_json(answer("InProgress")), Ok(GenerationStatus::Processing)),
            (ResponseTemplate::new(200).set_body_json(answer("Failed")), Ok(GenerationStatus::Failed)),
            (ResponseTemplate::new(200).set_body_json(answer("Paused")), Ok(GenerationStatus::Failed)),
            (ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })), Err(false)),
            (ResponseTemplate::new(503).set_body_string("busy"), Err(true)),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET")).respond_with(reply).mount(&server).await;
            let got = make_provider(&server.uri(), &server.uri(), Some("s3://my-bucket/litegen/")).poll_status(&handle()).await;
            match (got, want) {
                (Ok(p), Ok(status)) => {
                    assert_eq!(p.status, status);
                    if p.status == GenerationStatus::Failed {
                        assert!(p.error.is_some());
                    }
                }
                (Err(ProviderError::RequestFailed { retryable, .. }), Err(want)) => assert_eq!(retryable, want),
                (got, want) => panic!("{got:?} for {want:?}"),
            }
        }
        // A bucket that answers 5xx is asked again next tick.
        let bedrock = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(completed("s3://my-bucket/litegen/abc123"))).mount(&bedrock).await;
        let s3 = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&s3).await;
        let err = make_provider(&bedrock.uri(), &s3.uri(), Some("s3://my-bucket/litegen/")).poll_status(&handle()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: true, .. }), "{err:?}");
    }
}
