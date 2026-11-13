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

/// Tencent TokenHub's production API base. Every TokenHub endpoint litegen
/// calls is under it: `/wand/hunyuan-image/…`, `/wand/hunyuan-video/…`,
/// `/api/3d/…`.
///
/// @see <https://cloud.tencent.com/document/product/1823/135745>
///   Verbatim: "接口： POST https://tokenhub.tencentmaas.com/v1/wand/hunyuan-image/v35-generation"
pub(crate) const TOKENHUB_API_BASE: &str = "https://tokenhub.tencentmaas.com/v1";

/// The endpoint and wire `model` of a TokenHub image row; `None` for any id
/// the catalog does not carry (never a default model: a typo would bill as
/// another model).
///
/// @see <https://cloud.tencent.com/document/product/1823/135745> — Hy-Image-3.5-preview (2026-11-09)
///   Verbatim: "接口： POST https://tokenhub.tencentmaas.com/v1/wand/hunyuan-image/v35-generation"
///   Verbatim: "model | 是 | string | 模型 ID。取值：hy-image-v3.5-preview"
/// @see <https://cloud.tencent.com/document/product/1823/130055> — "0.15 元/张" (1K, 2K), "0.2 元/张" (4K) (2026-11-09)
fn route(model_id: &str) -> Option<(&'static str, &'static str, Shape)> {
    match model_id {
        "tokenhub/hy-image-v3.5-preview" => Some(("/wand/hunyuan-image/v35-generation", "hy-image-v3.5-preview", Shape::HyImage35)),
        // Seedream resold: "POST https://tokenhub.tencentmaas.com/v1/wand/si-image/generation",
        // "model … 取值：seedream-image-v5.0-pro、seedream-image-v5.0-lite、seedream-image-v5.0-flash".
        // @see <https://cloud.tencent.com/document/product/1823/136609> (2026-11-09)
        "tokenhub/seedream-image-v5.0-pro" => Some(("/wand/si-image/generation", "seedream-image-v5.0-pro", Shape::Seedream)),
        "tokenhub/seedream-image-v5.0-flash" => Some(("/wand/si-image/generation", "seedream-image-v5.0-flash", Shape::Seedream)),
        // Kling resold: "POST https://tokenhub.tencentmaas.com/v1/wand/kling-image/generation",
        // "model … 取值：kling-image-v3" / "kling-image-o1、kling-image-v3-omni".
        // @see <https://cloud.tencent.com/document/product/1823/138803> (2026-11-09)
        "tokenhub/kling-image-v3" => Some(("/wand/kling-image", "kling-image-v3", Shape::KlingImage)),
        "tokenhub/kling-image-o1" => Some(("/wand/kling-image", "kling-image-o1", Shape::KlingImage)),
        "tokenhub/kling-image-v3-omni" => Some(("/wand/kling-image", "kling-image-v3-omni", Shape::KlingImage)),
        // Vidu resold: "POST https://tokenhub.tencentmaas.com/v1/wand/vidu-image/generation", "取值：vidu-image-q2".
        // @see <https://cloud.tencent.com/document/product/1823/135746> (2026-11-09)
        "tokenhub/vidu-image-q2" => Some(("/wand/vidu-image", "vidu-image-q2", Shape::ViduImage)),
        _ => None,
    }
}

/// The vendor's spelling of a litegen tier on an asynchronous route: Kling's
/// "1k（1K 标清）、2k（2K 高清）、4k（4K 超高清）" ("4K 仅 kling-image-v3-omni 支持"), Vidu's
/// "1080p / 2K / 4K" (litegen's 1K is its lowest, 1080p).
/// @see <https://cloud.tencent.com/document/product/1823/138803> (2026-11-09)
/// @see <https://cloud.tencent.com/document/product/1823/135746> (2026-11-09)
fn async_tier(model_id: &str, tier: &str) -> Option<&'static str> {
    match (model_id, tier) {
        ("tokenhub/kling-image-v3" | "tokenhub/kling-image-o1" | "tokenhub/kling-image-v3-omni", "1K") => Some("1k"),
        ("tokenhub/kling-image-v3" | "tokenhub/kling-image-o1" | "tokenhub/kling-image-v3-omni", "2K") => Some("2k"),
        ("tokenhub/kling-image-v3-omni", "4K") => Some("4k"),
        ("tokenhub/vidu-image-q2", "1K") => Some("1080p"),
        ("tokenhub/vidu-image-q2", "2K") => Some("2K"),
        ("tokenhub/vidu-image-q2", "4K") => Some("4K"),
        _ => None,
    }
}

/// The ratio a Kling omni text-to-image request goes with when the caller
/// names none. TokenHub's own default is refused there: "默认值 auto。注：文生图
/// 场景（未传 image_list）不支持 auto". 16:9 is the default the same guide gives
/// kling-image-v3 ("默认值 16:9"). A request with images still sends no ratio,
/// so `auto` follows them. Not a row default: the dashboard fills a row's
/// default in on every request, images or not.
/// @see <https://cloud.tencent.com/document/product/1823/138803> (2026-11-09)
const KLING_OMNI_TEXT_TO_IMAGE_RATIO: &str = "16:9";

/// How a TokenHub image API is spoken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Hy-Image-3.5: OpenAI Chat-style `messages`; the image at
    /// `choices[0].delta.image.url`.
    HyImage35,
    /// Seedream resold: Ark's image body (`images`, `size` tier or WxH,
    /// `response_format`); the image at `data[0].b64_json` (or `url`).
    Seedream,
    /// Kling's image API resold: submit, then poll `task_status`
    /// (`video::kling::read_kling` envelope); the image at
    /// `task_result.images[0].url`.
    KlingImage,
    /// Vidu's image API resold: submit, then poll `state`; the image at
    /// `creations[0].url`.
    ViduImage,
}

/// `generate_max_pixels` for a litegen tier: "支持 3 个枚举档位：1048576（1K，1024×1024）、
/// 2359296（1.5K，默认）、4194304（2K）" (Hy-Image-3.5-preview). 4K is reached only
/// with an exact `size` ("generate_max_pixels 最高档位为 2K（4194304），要出 4K 必须走
/// size 参数").
/// @see <https://cloud.tencent.com/document/product/1823/135745> (2026-11-09)
fn generate_max_pixels(tier: &str) -> Option<u64> {
    match tier {
        "1K" => Some(1_048_576),
        "1.5K" => Some(2_359_296),
        "2K" => Some(4_194_304),
        _ => None,
    }
}

/// The tier sent beside a ratio-shaped size when no `resolution` is asked
/// for: the vendor's own default.
const DEFAULT_TIER: &str = "1.5K";

/// A `size` of exactly `ratio` ("W:H"), about 1 MP, with both sides in the
/// vendor's [256, 8192] ("宽、高均为正整数，取值范围 [256, 8192]") and at most
/// 16,777,216 px. Sent beside `generate_max_pixels`, it says only the shape:
/// "size + generate_max_pixels：最终宽高由内部逻辑综合计算，尽量贴近 size 的宽高比例与
/// generate_max_pixels 的面积档位".
/// @see <https://cloud.tencent.com/document/product/1823/135745> (2026-11-09)
fn ratio_size(ratio: &str) -> Option<String> {
    let (w, h) = ratio.split_once(':')?;
    let (w, h) = (w.trim().parse::<u64>().ok()?, h.trim().parse::<u64>().ok()?);
    if w == 0 || h == 0 {
        return None;
    }
    let k = ((1_048_576.0 / (w * h) as f64).sqrt().round() as u64).max(1);
    let (sw, sh) = (w * k, h * k);
    let side = 256..=8192;
    (side.contains(&sw) && side.contains(&sh) && sw * sh <= 16_777_216).then(|| format!("{sw}x{sh}"))
}

/// The vendor's reason from a failure frame or an error answer:
/// `{error: {type, code, message}}` ("仅失败终态帧才带，OpenAI 风格 {type, code,
/// message, request_id}"), or the video API's `{error: {code, message}}`.
fn vendor_reason(body: &Value) -> Option<String> {
    let e = body.get("error").filter(|e| e.is_object())?;
    let code = e["code"].as_str().map(str::to_string).or_else(|| e["code"].as_i64().map(|c| c.to_string()));
    match (code, e["message"].as_str()) {
        (Some(c), Some(m)) => Some(format!("{c}: {m}")),
        (Some(c), None) => Some(c),
        (None, Some(m)) => Some(m.to_string()),
        (None, None) => None,
    }
}

/// Tencent TokenHub image models: Hunyuan's new home ("腾讯混元大模型相关功能将逐步
/// 迁移至 TokenHub。迁移后，原平台将不再新增模型能力，并停止支持新购模型服务"; the old
/// Tencent Cloud API stays as the `hunyuan` provider for its existing buyers).
///
/// Synchronous: one POST answers with the image's URL (a signed COS link,
/// valid 12 h), which is downloaded without credentials.
///
/// @see <https://cloud.tencent.com/document/product/1823/135745> — auth, endpoint, fields
///   Verbatim: "下文所有示例中的 YOUR_API_KEY 均需替换为您自己的 API Key，鉴权方式为请求头 Authorization: Bearer YOUR_API_KEY。"
///   Verbatim: "两个模型均为同步调用：一次请求直接返回生成结果，无需提交任务与轮询。"
///   Verbatim: "choices[0].delta.image.url 即为本次生图的最终交付 URL，客户端应优先读取此字段。"
///   Verbatim: "HTTP 状态码 400 请求格式有误 … 401 鉴权不通过 … 422 输入、输出审核不通过（内容安全拦截）… 429 请求并发数超过限额 … 500 内部错误"
/// @see <https://cloud.tencent.com/document/product/1729/105925> — the migration notice (2026-11-09)
pub struct TokenHubImageProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
    asset_client: Client,
    pub(crate) public_assets_only: bool,
}

impl TokenHubImageProvider {
    fn api_client(extra_headers: &HashMap<String, String>) -> Client {
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
        self.config.as_ref().and_then(|c| c.api_base.as_deref()).unwrap_or(TOKENHUB_API_BASE)
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("tokenhub".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }
}

impl TokenHubImageProvider {
    /// Seedream resold on TokenHub, synchronous, in Ark's image shape.
    ///
    /// @see <https://cloud.tencent.com/document/product/1823/136609> (2026-11-09)
    ///   Verbatim: "size … 方式 1（推荐）：指定分辨率档位 … 可选值：1K、1.5K、2K 方式 2：指定宽高像素值（宽 x 高）"; "不可混用"
    ///   Verbatim: "response_format … url：返回图片下载链接 … b64_json：以 Base64 编码字符串的 JSON 格式返回图像数据。"
    async fn seedream(
        &self,
        model: &ModelSchema,
        endpoint: &str,
        native: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        if extras.size.is_some() && extras.resolution.is_some() {
            return Err(ProviderError::InvalidRequest("tokenhub: Seedream takes a resolution tier or an exact size, not both (\"不可混用\")".into()));
        }
        let creds = self.creds()?;
        let mut body = json!({});
        // `extra` (watermark, optimize_prompt_options) first; none replaces an owned field (rule 11).
        if let Some(obj) = body.as_object_mut() {
            let owned = |k: &str| matches!(k, "model" | "prompt" | "images" | "size" | "response_format" | "output_format" | "background" | "sequential_image_generation" | "layer_decomposition");
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
                obj.insert(k.clone(), v.clone());
            }
        }
        body["model"] = json!(crate::providers::wire_model(self.config.as_ref(), &model.id, native));
        body["prompt"] = json!(base.prompt);
        // Inline, so nothing is fetched server-side.
        body["response_format"] = json!("b64_json");
        if let Some(size) = extras.size.as_deref().or(extras.resolution.as_deref()) {
            body["size"] = json!(size);
        }
        if let Some(format) = extras.output_format.as_deref() {
            body["output_format"] = json!(format);
        }
        if let Some(bg) = extras.background.as_deref() {
            body["background"] = json!(bg);
        }
        let images: Vec<Value> = materialized
            .refs
            .iter()
            .filter_map(|r| match &r.form {
                MaterializedRefForm::Base64(b64) => Some(json!(crate::proxy::materializer::base64_data_uri(b64, Some("image/png")))),
                MaterializedRefForm::Url(u) => Some(json!(u)),
                MaterializedRefForm::MultipartField { .. } => None,
            })
            .collect();
        if !images.is_empty() {
            body["images"] = Value::Array(images);
        }
        let url = format!("{}{endpoint}", self.api_base());
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.post(&url).header("Content-Type", "application/json").json(&body))?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("TokenHub request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_connect(),
        })?;
        let data = crate::providers::read_vendor_json(resp, "TokenHub").await?;
        let first = &data["data"][0];
        let bytes = if let Some(b64) = first["b64_json"].as_str() {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| ProviderError::RequestFailed {
                message: format!("TokenHub {native}: undecodable b64_json: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
        } else if let Some(u) = first["url"].as_str() {
            super::fetch_result(&self.asset_client, u, self.public_assets_only, "tokenhub image").await?
        } else {
            let why = vendor_reason(&data).unwrap_or_else(|| "the answer names no image (data[0])".into());
            return Err(ProviderError::RequestFailed { message: format!("TokenHub {native}: {why}"), status_code: None, provider_error: Some(data), retryable: false });
        };
        let mut metadata = HashMap::new();
        metadata.insert("model".to_string(), Value::String(native.to_string()));
        if let Some(tokens) = data["tokenhub_usage"]["total_tokens"].as_u64() {
            metadata.insert("tokenhub_tokens".to_string(), json!(tokens));
        }
        // The bytes say what they are; else the answer's output_format.
        let content_type = crate::util::content::image_content_type(&bytes)
            .map(str::to_string)
            .unwrap_or_else(|| if first["output_format"] == "png" { "image/png".into() } else { "image/jpeg".into() });
        Ok(GenerationOutput { data: bytes, content_type, metadata, more: Vec::new() })
    }
}

impl TokenHubImageProvider {
    /// Kling's and Vidu's image APIs resold: submit, record the accepted task
    /// (a timeout from here is never sent again), poll within litegen's
    /// deadline, download the image.
    #[allow(clippy::too_many_arguments)]
    async fn submit_async(
        &self,
        model: &ModelSchema,
        prefix: &str,
        native: &str,
        shape: Shape,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let tier = match extras.resolution.as_deref() {
            Some(t) => Some(async_tier(&model.id, t).ok_or_else(|| ProviderError::InvalidRequest(format!("{}: no resolution tier {t}", model.id)))?),
            None => None,
        };
        let raw = |form: &MaterializedRefForm| match form {
            MaterializedRefForm::Base64(b64) => Some(b64.clone()),
            MaterializedRefForm::Url(u) => Some(u.clone()),
            MaterializedRefForm::MultipartField { .. } => None,
        };
        let mut body = json!({ "model": crate::providers::wire_model(self.config.as_ref(), &model.id, native), "prompt": base.prompt });
        if shape == Shape::KlingImage {
            let images: Vec<String> = materialized.refs.iter().filter_map(|r| raw(&r.form)).collect();
            // One image: litegen returns and bills one, whatever `n` was.
            body["n"] = json!(1);
            if native == "kling-image-v3" {
                // "仅文生图支持；图生图（image 不为空）场景下不支持负向提示词。"
                if base.negative_prompt.is_some() && !images.is_empty() {
                    return Err(ProviderError::InvalidRequest(format!("{}: a negative_prompt cannot go with a reference image", model.id)));
                }
                if let Some(np) = base.negative_prompt.as_deref() {
                    body["negative_prompt"] = json!(np);
                }
                // "image … Base64 方式无须添加 data:image/png;base64, 前缀，直接传 Base64 字符串本身。"
                if let Some(first) = images.into_iter().next() {
                    body["image"] = json!(first);
                }
            } else {
                if !images.is_empty() {
                    body["image_list"] = json!(images);
                } else if extras.aspect_ratio.is_none() {
                    body["aspect_ratio"] = json!(KLING_OMNI_TEXT_TO_IMAGE_RATIO);
                }
                // "result_type … single（单图，默认）、series（组图）": litegen delivers one.
                body["result_type"] = json!("single");
            }
        } else {
            // "格式要求：data:image/<图片格式>;base64,<Base64 编码>"
            let images: Vec<String> = materialized
                .refs
                .iter()
                .filter_map(|r| match &r.form {
                    MaterializedRefForm::Base64(b64) => Some(crate::proxy::materializer::base64_data_uri(b64, Some("image/png"))),
                    MaterializedRefForm::Url(u) => Some(u.clone()),
                    MaterializedRefForm::MultipartField { .. } => None,
                })
                .collect();
            if !images.is_empty() {
                body["images"] = json!(images);
            }
            if let Some(seed) = base.seed {
                body["seed"] = json!(seed);
            }
        }
        if let Some(ratio) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = json!(ratio);
        }
        if let Some(t) = tier {
            body["resolution"] = json!(t);
        }
        let creds = self.creds()?;
        let url = format!("{}{prefix}/generation", self.api_base());
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.post(&url).header("Content-Type", "application/json").json(&body))?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("TokenHub request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_connect(),
        })?;
        let (task_id, answer) = if shape == Shape::KlingImage {
            let data = crate::providers::video::kling::read_kling(resp, "via TokenHub image submit").await?;
            (data["data"]["task_id"].as_str().map(str::to_string), data)
        } else {
            let data = crate::providers::read_vendor_json(resp, "TokenHub").await?;
            (data["task_id"].as_str().map(str::to_string), data)
        };
        let Some(task_id) = task_id.filter(|t| !t.is_empty()) else {
            return Err(ProviderError::RequestFailed {
                message: format!("TokenHub {native} answer names no task_id"),
                status_code: None,
                provider_error: Some(answer),
                retryable: false,
            });
        };
        // Accepted: from here on a failure must not re-send the job (L1).
        extras.vendor_job.record(task_id.clone());
        let url = self.poll_async(shape, prefix, &task_id).await?;
        self.finish_async(native, &task_id, &url).await
    }

    /// Poll an accepted task until its image URL is known, within litegen's
    /// deadline (180 s when called directly). Kling: "submitted / processing /
    /// succeed / failed"; Vidu: "created / queueing / processing / success /
    /// failed". A dropped or passing failure is asked again; a verdict is final.
    async fn poll_async(&self, shape: Shape, prefix: &str, task_id: &str) -> Result<String, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}{prefix}/tasks/{task_id}", self.api_base());
        let started = std::time::Instant::now();
        let mut budget = crate::providers::PollBudget::new(std::time::Duration::from_secs(180), std::time::Duration::from_secs(2));
        while budget.next().await {
            let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
            let Ok(resp) = crate::providers::inject_trace_headers(builder).send().await else { continue };
            let read = if shape == Shape::KlingImage {
                crate::providers::video::kling::read_kling(resp, "via TokenHub image poll").await
            } else {
                crate::providers::read_vendor_json(resp, "TokenHub").await
            };
            let data = match read {
                Ok(data) => data,
                Err(ProviderError::RequestFailed { retryable: true, .. } | ProviderError::RateLimited { .. }) => continue,
                Err(e) => return Err(e),
            };
            let failed = |why: String| ProviderError::RequestFailed { message: format!("TokenHub task {task_id} failed: {why}"), status_code: None, provider_error: Some(data.clone()), retryable: false };
            if shape == Shape::KlingImage {
                match data["data"]["task_status"].as_str() {
                    Some("succeed") => {
                        return data["data"]["task_result"]["images"][0]["url"].as_str().map(str::to_string).ok_or_else(|| failed("finished with no image (task_result.images[0].url)".into()))
                    }
                    Some("failed") => return Err(failed(data["data"]["task_status_msg"].as_str().unwrap_or("no reason given").to_string())),
                    Some("submitted") | Some("processing") => continue,
                    other => return Err(super::unknown_state("tokenhub", other)),
                }
            } else {
                match data["state"].as_str() {
                    Some("success") => return data["creations"][0]["url"].as_str().map(str::to_string).ok_or_else(|| failed("finished with no image (creations[0].url)".into())),
                    Some("failed") => return Err(failed(data["err_code"].as_str().or(data["message"].as_str()).unwrap_or("no reason given").to_string())),
                    Some("created") | Some("queueing") | Some("processing") => continue,
                    other => return Err(super::unknown_state("tokenhub", other)),
                }
            }
        }
        // Still running at litegen's deadline: the task id is recorded, so the
        // router delivers it later instead of sending it again (late delivery).
        Err(ProviderError::Timeout { timeout_ms: budget.total_ms(started) })
    }

    /// A finished task's image, downloaded without credentials.
    async fn finish_async(&self, native: &str, task_id: &str, url: &str) -> Result<GenerationOutput, ProviderError> {
        let bytes = super::fetch_result(&self.asset_client, url, self.public_assets_only, "tokenhub image").await?;
        let mut metadata = HashMap::new();
        metadata.insert("model".to_string(), Value::String(native.to_string()));
        metadata.insert("task_id".to_string(), Value::String(task_id.to_string()));
        let content_type = crate::util::content::image_content_type(&bytes).unwrap_or("image/png").to_string();
        Ok(GenerationOutput { data: bytes, content_type, metadata, more: Vec::new() })
    }
}

impl Default for TokenHubImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for TokenHubImageProvider {
    fn name(&self) -> &str {
        "tokenhub"
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
        let (endpoint, native, shape) = route(&model.id).ok_or_else(|| ProviderError::ModelNotSupported {
            model: model.id.clone(),
            provider: "tokenhub".into(),
        })?;
        if shape == Shape::Seedream {
            return self.seedream(model, endpoint, native, base, extras, materialized).await;
        }
        if matches!(shape, Shape::KlingImage | Shape::ViduImage) {
            return self.submit_async(model, endpoint, native, shape, base, extras, materialized).await;
        }
        // An exact size beside a tier or a ratio would be read as a ratio
        // ("size + generate_max_pixels：最终宽高由内部逻辑综合计算"), so it is not exact.
        if extras.size.is_some() && (extras.resolution.is_some() || extras.aspect_ratio.is_some()) {
            return Err(ProviderError::InvalidRequest(
                "tokenhub: send an exact size, or resolution and/or aspect_ratio — not size with either".into(),
            ));
        }
        let max_pixels = match extras.resolution.as_deref() {
            Some(t) => Some(generate_max_pixels(t).ok_or_else(|| ProviderError::InvalidRequest(format!("tokenhub: no resolution tier '{t}'")))?),
            None => None,
        };
        let creds = self.creds()?;

        // One user message: the prompt, then each image as an image_url part
        // ("type | 是 | string | 内容类型，text 或 image_url"; "image_url … 结构为 {"url": "..."}").
        let mut content = vec![json!({ "type": "text", "text": base.prompt })];
        for r in &materialized.refs {
            let url = match &r.form {
                MaterializedRefForm::Url(u) => u.clone(),
                // "data:image/...;base64,...": the bytes' own type.
                MaterializedRefForm::Base64(b64) => crate::proxy::materializer::base64_data_uri(b64, Some("image/png")),
                MaterializedRefForm::MultipartField { .. } => continue,
            };
            content.push(json!({ "type": "image_url", "image_url": { "url": url } }));
        }
        let mut body = json!({});
        // `extra` adds the endpoint's own fields (footnote, use_search_tool); it
        // is merged first, so none can replace a field litegen sets (rule 11).
        if let Some(obj) = body.as_object_mut() {
            let owned = |k: &str| matches!(k, "model" | "messages" | "size" | "seed" | "generate_max_pixels");
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
                obj.insert(k.clone(), v.clone());
            }
        }
        body["model"] = json!(crate::providers::wire_model(self.config.as_ref(), &model.id, native));
        body["messages"] = json!([{ "role": "user", "content": content }]);
        match (extras.size.as_deref(), extras.aspect_ratio.as_deref()) {
            // "传入具体值时强制按指定尺寸出图": an exact size.
            (Some(size), _) => body["size"] = json!(size),
            (None, Some(ratio)) => {
                let size = ratio_size(ratio).ok_or_else(|| ProviderError::InvalidRequest(format!("tokenhub: no size for aspect ratio '{ratio}'")))?;
                body["size"] = json!(size);
                body["generate_max_pixels"] = json!(max_pixels.or_else(|| generate_max_pixels(DEFAULT_TIER)));
            }
            (None, None) => {
                if let Some(px) = max_pixels {
                    body["generate_max_pixels"] = json!(px);
                }
            }
        }
        // "生成种子，int64 类型，范围 [0, 2^63-1]"; the row starts it at 1 (0 is random).
        if let Some(seed) = base.seed {
            body["seed"] = json!(seed);
        }

        let url = format!("{}{endpoint}", self.api_base());
        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("TokenHub request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            // A call that timed out may still run, and bill: only a
            // connection that never opened is sent again.
            retryable: e.is_connect(),
        })?;
        let data = crate::providers::read_vendor_json(resp, "TokenHub").await?;

        let Some(image_url) = data["choices"][0]["delta"]["image"]["url"].as_str().filter(|u| !u.is_empty()) else {
            // "仅在内容安全拦截 / 业务错误时会为 error 等非空值": the vendor's verdict.
            let why = vendor_reason(&data).unwrap_or_else(|| "the answer names no image (choices[0].delta.image.url)".into());
            return Err(ProviderError::RequestFailed {
                message: format!("TokenHub {native}: {why}"),
                status_code: None,
                provider_error: Some(data),
                retryable: false,
            });
        };
        let bytes = super::fetch_result(&self.asset_client, image_url, self.public_assets_only, "tokenhub image").await?;

        let mut metadata = HashMap::new();
        metadata.insert("model".to_string(), Value::String(native.to_string()));
        if let Some(tokens) = data["tokenhub_usage"]["total_tokens"].as_u64() {
            metadata.insert("tokenhub_tokens".to_string(), json!(tokens));
        }
        let content_type = crate::util::content::image_content_type(&bytes).unwrap_or("image/png").to_string();
        Ok(GenerationOutput { data: bytes, content_type, metadata, more: Vec::new() })
    }

    fn resumable(&self, model: &ModelSchema) -> bool {
        route(&model.id).is_some_and(|(_, _, shape)| matches!(shape, Shape::KlingImage | Shape::ViduImage))
    }

    /// One look at an accepted task, read as `generate`'s own poll loop reads it.
    async fn poll_job(&self, model: &str, job: &str, _resume: Option<&Value>) -> Result<crate::providers::ImageJobPoll, ProviderError> {
        let Some((prefix, native, shape)) = route(model).filter(|(_, _, s)| matches!(s, Shape::KlingImage | Shape::ViduImage)) else {
            return Err(ProviderError::ModelNotSupported { model: model.to_string(), provider: "tokenhub".into() });
        };
        match super::poll_once(self.poll_async(shape, prefix, job)).await {
            Ok(Some(url)) => self.finish_async(native, job, &url).await.map(crate::providers::ImageJobPoll::Done),
            Ok(None) => Ok(crate::providers::ImageJobPoll::Running),
            Err(e) => super::job_failed(e),
        }
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, Some(json!({ "model": model.id }))))
    }

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult { healthy: false, message: "TokenHub provider not configured".into(), latency_ms: None };
        }
        HealthCheckResult { healthy: true, message: "TokenHub provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ID: &str = "tokenhub/hy-image-v3.5-preview";
    const PATH: &str = "/wand/hunyuan-image/v35-generation";
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nTOKENHUB";

    fn ref_schema(id: &str) -> ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load").get(id).expect("model").clone()
    }

    /// A provider whose key comes from a dashboard pool, as a BYO credential does.
    fn pooled(api_base: &str) -> TokenHubImageProvider {
        let mut p = TokenHubImageProvider::new();
        p.configure(ProviderInstanceConfig {
            api_base: Some(api_base.to_string()),
            api_keys: vec![ApiKeyEntry { key: "th-pooled".into(), weight: 1, label: None }],
            ..Default::default()
        });
        p.public_assets_only = false;
        p
    }

    fn base(prompt: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(), model: ID.to_string(), n: 1, negative_prompt: None,
            seed: None, reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: None, resolution: None, aspect_ratio: None, quality: None, style: None,
            steps: None, guidance_scale: None, strength: None, response_format: "url".to_string(), extra: None,
            edit: None, views: Vec::new(), output_format: None, background: None, expand: None,
            layout: None, mask_prompt: None, preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        }
    }

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    /// The vendor's success answer, naming the image at `url`.
    fn answer(url: &str) -> Value {
        json!({
            "id": "1374200352-WandImage-085e", "object": "image.chat.completion.chunk", "created": 1789720599,
            "model": "HY-Image-3.5-preview-4090-Tob-v1.2", "round": 0,
            "choices": [{ "index": 0, "delta": { "type": "image", "image": { "url": url, "width": 1024, "height": 1024, "source": "generate" } }, "finish_reason": null }],
            "usage": { "total_tokens": 15000 }, "tokenhub_usage": { "total_tokens": 15000 }, "request_id": "a1c0"
        })
    }

    async fn vendor() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/result/main.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG.to_vec()))
            .mount(&server).await;
        Mock::given(method("POST")).and(path(PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer(&format!("{}/result/main.png", server.uri()))))
            .mount(&server).await;
        server
    }

    async fn sent_body(server: &MockServer) -> Value {
        let received = server.received_requests().await.unwrap();
        let submit = received.iter().find(|r| r.url.path() == PATH).expect("a submit");
        serde_json::from_slice(&submit.body).unwrap()
    }

    /// "以 OpenAI Chat 风格的 messages 协议提交 … 服务端会取数组中最后一个 role=user 的消息作为本轮
    /// 生图指令"; "鉴权方式为请求头 Authorization: Bearer YOUR_API_KEY"; the answer's
    /// choices[0].delta.image.url is the image, downloaded.
    /// @see <https://cloud.tencent.com/document/product/1823/135745> (2026-11-09)
    #[tokio::test]
    async fn hy_image_3_5_sends_its_prompt_as_one_user_message_with_the_pooled_key() {
        let server = vendor().await;
        let out = pooled(&server.uri())
            .generate(&ref_schema(ID), &base("a golden retriever on a beach at dusk"), &extras(), &no_refs())
            .await
            .expect("generated");
        assert_eq!(out.data, PNG);
        assert_eq!(out.content_type, "image/png");

        let received = server.received_requests().await.unwrap();
        let submit = received.iter().find(|r| r.url.path() == PATH).expect("a submit");
        assert_eq!(submit.headers.get("authorization").unwrap(), "Bearer th-pooled");
        let body: Value = serde_json::from_slice(&submit.body).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "hy-image-v3.5-preview",
                "messages": [{ "role": "user", "content": [{ "type": "text", "text": "a golden retriever on a beach at dusk" }] }]
            }),
            "nothing else: no size, tier or seed was asked for"
        );
    }

    /// Images go after the text as `image_url` parts: "支持 http(s) 公网 URL 或
    /// data:image/...;base64,... 形态".
    #[tokio::test]
    async fn references_go_as_image_url_parts_after_the_text() {
        let server = vendor().await;
        let refs = MaterializedRequest {
            refs: vec![
                MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64("iVBORw0KGgoAAAANSUhEUg==".into()), time_s: None },
                MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64("/9j/4AAQSkZJRgABAQ==".into()), time_s: None },
            ],
            cleanup: Cleanup::empty(),
        };
        pooled(&server.uri()).generate(&ref_schema(ID), &base("paint the cat in this style"), &extras(), &refs).await.expect("generated");
        let content = &sent_body(&server).await["messages"][0]["content"];
        assert_eq!(content[0], json!({ "type": "text", "text": "paint the cat in this style" }));
        assert_eq!(content[1], json!({ "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==" } }));
        assert_eq!(content[2], json!({ "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQ==" } }));
        assert_eq!(content.as_array().map(Vec::len), Some(3));
    }

    /// `resolution` → generate_max_pixels ("1048576（1K）、2359296（1.5K，默认）、
    /// 4194304（2K）"); `aspect_ratio` → a size of that ratio beside it ("size +
    /// generate_max_pixels：… 尽量贴近 size 的宽高比例与 generate_max_pixels 的面积档位"), the
    /// tier asked for or the default 1.5K; an exact `size` alone; `seed` as is.
    #[tokio::test]
    async fn the_tier_goes_as_generate_max_pixels_and_a_ratio_as_a_size_beside_it() {
        let server = vendor().await;
        let p = pooled(&server.uri());
        let schema = ref_schema(ID);
        // (size, resolution, aspect_ratio) → what goes out.
        type Case<'a> = (Option<&'a str>, Option<&'a str>, Option<&'a str>, Value);
        let cases: [Case; 4] = [
            (None, Some("2K"), None, json!({ "generate_max_pixels": 4194304 })),
            (None, None, Some("16:9"), json!({ "size": "1360x765", "generate_max_pixels": 2359296 })),
            (None, Some("1K"), Some("9:21"), json!({ "size": "666x1554", "generate_max_pixels": 1048576 })),
            (Some("4096x4096"), None, None, json!({ "size": "4096x4096" })),
        ];
        for (size, tier, ratio, want) in cases {
            server.reset().await;
            let server_uri = server.uri();
            Mock::given(method("GET")).and(path("/result/main.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(PNG.to_vec())).mount(&server).await;
            Mock::given(method("POST")).and(path(PATH)).respond_with(ResponseTemplate::new(200).set_body_json(answer(&format!("{server_uri}/result/main.png")))).mount(&server).await;
            let mut e = extras();
            e.size = size.map(str::to_string);
            e.resolution = tier.map(str::to_string);
            e.aspect_ratio = ratio.map(str::to_string);
            let mut b = base("a lighthouse");
            b.seed = Some(42);
            p.generate(&schema, &b, &e, &no_refs()).await.expect("generated");
            let body = sent_body(&server).await;
            for k in ["size", "generate_max_pixels"] {
                assert_eq!(body.get(k), want.get(k), "{size:?} {tier:?} {ratio:?}: {k}");
            }
            assert_eq!(body["seed"], 42);
        }
    }

    /// An exact size beside a tier or a ratio would be read as a ratio by the
    /// vendor, so the pair is refused before any call.
    #[tokio::test]
    async fn a_size_with_a_tier_or_a_ratio_is_refused_before_any_call() {
        let server = MockServer::start().await;
        let p = pooled(&server.uri());
        for (tier, ratio) in [(Some("2K"), None), (None, Some("1:1"))] {
            let mut e = extras();
            e.size = Some("1024x1024".into());
            e.resolution = tier.map(str::to_string);
            e.aspect_ratio = ratio.map(str::to_string);
            let err = p.generate(&ref_schema(ID), &base("x"), &e, &no_refs()).await.expect_err("refused");
            assert!(matches!(err, ProviderError::InvalidRequest(ref m) if m.contains("size")), "{err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// Every tier and ratio the row declares maps to a value inside the
    /// vendor's bounds: a tier of generate_max_pixels' three, a size with both
    /// sides in [256, 8192] and at most 16,777,216 px, of exactly that ratio.
    #[test]
    fn every_declared_tier_and_ratio_maps_inside_the_vendors_bounds() {
        use crate::capabilities::schema::ParamSpec;
        let schema = ref_schema(ID);
        let Some(ParamSpec::String(tiers)) = schema.params.get("resolution") else { panic!("no resolution") };
        for t in &tiers.enum_values {
            assert!(matches!(generate_max_pixels(t), Some(1048576 | 2359296 | 4194304)), "{t}");
        }
        let Some(ParamSpec::AspectRatio(ratios)) = schema.params.get("aspect_ratio") else { panic!("no aspect_ratio") };
        for r in &ratios.allowed {
            let size = ratio_size(r).unwrap_or_else(|| panic!("{r} maps to no size"));
            let (w, h) = size.split_once('x').map(|(w, h)| (w.parse::<u64>().unwrap(), h.parse::<u64>().unwrap())).unwrap();
            assert!((256..=8192).contains(&w) && (256..=8192).contains(&h) && w * h <= 16_777_216, "{r} → {size}");
            let (rw, rh) = r.split_once(':').map(|(a, b)| (a.parse::<u64>().unwrap(), b.parse::<u64>().unwrap())).unwrap();
            assert_eq!(w * rh, h * rw, "{r} → {size} keeps the ratio exactly");
        }
        assert_eq!(generate_max_pixels("4K"), None, "4K is a size, not a tier");
        assert_eq!(ratio_size("0:1"), None);
    }

    const SEEDREAM: &str = "tokenhub/seedream-image-v5.0-pro";
    const SI: &str = "/wand/si-image/generation";

    async fn seedream_vendor(answer: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path(SI)).respond_with(ResponseTemplate::new(200).set_body_json(answer)).mount(&server).await;
        server
    }

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    async fn si_body(server: &MockServer) -> Value {
        let received = server.received_requests().await.unwrap();
        serde_json::from_slice(&received.iter().find(|r| r.url.path() == SI).expect("a call").body).unwrap()
    }

    /// Seedream resold on TokenHub: Ark's image shape at /wand/si-image/generation;
    /// a tier goes as `size` "1K" | "1.5K" | "2K", an exact size as "WxH" (never
    /// both); images as data URIs; the image comes back inline (b64_json).
    /// @see <https://cloud.tencent.com/document/product/1823/136609> (2026-11-09)
    #[tokio::test]
    async fn seedream_sends_arks_shape_with_a_tier_or_a_size() {
        let server = seedream_vendor(json!({ "created": 1, "data": [{ "b64_json": b64(PNG), "output_format": "png" }], "request_id": "r" })).await;
        let p = pooled(&server.uri());
        let mut e = extras();
        e.resolution = Some("1.5K".into());
        e.output_format = Some("png".into());
        e.background = Some("transparent".into());
        let refs = MaterializedRequest {
            refs: vec![MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64("iVBORw0KGgoAAAANSUhEUg==".into()), time_s: None }],
            cleanup: Cleanup::empty(),
        };
        let mut b = base("a red bicycle");
        b.model = SEEDREAM.into();
        let out = p.generate(&ref_schema(SEEDREAM), &b, &e, &refs).await.expect("generated");
        assert_eq!((out.data.as_slice(), out.content_type.as_str()), (PNG, "image/png"));
        assert_eq!(
            si_body(&server).await,
            json!({
                "model": "seedream-image-v5.0-pro", "prompt": "a red bicycle", "response_format": "b64_json",
                "size": "1.5K", "output_format": "png", "background": "transparent",
                "images": ["data:image/png;base64,iVBORw0KGgoAAAANSUhEUg=="]
            })
        );
        server.reset().await;
        Mock::given(method("POST")).and(path(SI)).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "b64_json": b64(PNG) }] }))).mount(&server).await;
        let mut e = extras();
        e.size = Some("2560x1440".into());
        p.generate(&ref_schema(SEEDREAM), &b, &e, &no_refs()).await.expect("generated");
        let body = si_body(&server).await;
        assert_eq!(body["size"], "2560x1440");
        assert!(body.get("images").is_none(), "{body}");
    }

    /// "不可混用": a tier and an exact size together are refused before any call;
    /// `extra` cannot replace an owned field; an answer with no image is final.
    #[tokio::test]
    async fn seedream_refuses_a_tier_with_a_size_and_keeps_its_owned_fields() {
        let server = seedream_vendor(json!({ "data": [{ "b64_json": b64(PNG) }] })).await;
        let p = pooled(&server.uri());
        let mut e = extras();
        e.resolution = Some("2K".into());
        e.size = Some("2048x2048".into());
        assert!(matches!(p.generate(&ref_schema(SEEDREAM), &base("x"), &e, &no_refs()).await, Err(ProviderError::InvalidRequest(_))));
        assert!(server.received_requests().await.unwrap().is_empty());
        let mut e = extras();
        e.extra = Some(json!({ "model": "other", "response_format": "url", "size": "1K", "prompt": "y", "watermark": false }));
        p.generate(&ref_schema(SEEDREAM), &base("x"), &e, &no_refs()).await.expect("generated");
        let body = si_body(&server).await;
        assert_eq!((body["model"].as_str(), body["response_format"].as_str(), body["prompt"].as_str()), (Some("seedream-image-v5.0-pro"), Some("b64_json"), Some("x")));
        assert!(body.get("size").is_none() && body["watermark"] == false, "{body}");
        server.reset().await;
        Mock::given(method("POST")).and(path(SI)).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] }))).mount(&server).await;
        let err = p.generate(&ref_schema(SEEDREAM), &base("x"), &extras(), &no_refs()).await.expect_err("no image");
        assert!(!err.is_retryable(), "{err:?}");
    }

    /// Kling's image API resold on TokenHub: submit at /wand/kling-image/generation
    /// (`model`, one image asked for, v3's single `image` as raw base64 — "无须添加
    /// data:image/png;base64, 前缀"), record the task, then poll
    /// /wand/kling-image/tasks/{task_id} ("submitted / processing / succeed / failed")
    /// and download `task_result.images[0].url`.
    /// @see <https://cloud.tencent.com/document/product/1823/138803> (2026-11-09)
    #[tokio::test]
    async fn kling_image_submits_polls_and_downloads() {
        let server = MockServer::start().await;
        let uri = server.uri();
        Mock::given(method("POST")).and(path("/wand/kling-image/generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "message": "SUCCEED", "data": { "task_id": "ki1", "task_status": "submitted" } })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/wand/kling-image/tasks/ki1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": { "task_id": "ki1", "task_status": "succeed", "task_result": { "images": [{ "index": 0, "url": format!("{uri}/result/main.png") }] } } })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/result/main.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(PNG.to_vec())).mount(&server).await;
        let id = "tokenhub/kling-image-v3";
        let mut e = extras();
        e.resolution = Some("2K".into());
        e.aspect_ratio = Some("21:9".into());
        let refs = MaterializedRequest {
            refs: vec![MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64("iVBORw0KGgoAAAANSUhEUg==".into()), time_s: None }],
            cleanup: Cleanup::empty(),
        };
        let mut b = base("a robot in the rain");
        b.n = 4;
        let out = pooled(&server.uri()).generate(&ref_schema(id), &b, &e, &refs).await.expect("generated");
        assert_eq!(out.data, PNG);
        assert_eq!(e.vendor_job.get().as_deref(), Some("ki1"), "the accepted task is recorded before polling");
        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received.iter().find(|r| r.url.path() == "/wand/kling-image/generation").unwrap().body).unwrap();
        assert_eq!(
            body,
            json!({ "model": "kling-image-v3", "prompt": "a robot in the rain", "n": 1, "resolution": "2k", "aspect_ratio": "21:9", "image": "iVBORw0KGgoAAAANSUhEUg==" })
        );
    }

    /// The omni models take `image_list` (raw base64, ≤ 10) and a single result
    /// ("result_type … single（单图，默认）"); a failed task ends with its message.
    #[tokio::test]
    async fn kling_omni_images_go_as_a_list_and_a_failure_names_its_reason() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/wand/kling-image/generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": { "task_id": "ko1", "task_status": "submitted" } })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/wand/kling-image/tasks/ko1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": { "task_id": "ko1", "task_status": "failed", "task_status_msg": "risk control" } })))
            .mount(&server).await;
        let refs = MaterializedRequest {
            refs: vec![
                MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64("AAAA".into()), time_s: None },
                MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64("BBBB".into()), time_s: None },
            ],
            cleanup: Cleanup::empty(),
        };
        let mut e = extras();
        e.resolution = Some("4K".into());
        match pooled(&server.uri()).generate(&ref_schema("tokenhub/kling-image-v3-omni"), &base("merge <<<image_1>>> into <<<image_2>>>"), &e, &refs).await {
            Err(ProviderError::RequestFailed { message, retryable: false, .. }) => assert!(message.contains("risk control"), "{message}"),
            other => panic!("{other:?}"),
        }
        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received.iter().find(|r| r.url.path() == "/wand/kling-image/generation").unwrap().body).unwrap();
        assert_eq!((body["image_list"].clone(), body["resolution"].as_str(), body["result_type"].as_str()), (json!(["AAAA", "BBBB"]), Some("4k"), Some("single")));
        assert!(body.get("image").is_none() && body.get("negative_prompt").is_none(), "{body}");
    }

    /// The omni models' `aspect_ratio`: "默认值 auto。注：文生图场景（未传 image_list）
    /// 不支持 auto" (the default is auto, which text-to-image without
    /// `image_list` does not take). A text-only request that names no ratio goes
    /// with the row's default; one with images still sends none, so `auto`
    /// follows them.
    /// @see <https://cloud.tencent.com/document/product/1823/138803> (2026-11-09)
    #[tokio::test]
    async fn kling_omni_text_to_image_never_rides_on_the_auto_default() {
        for id in ["tokenhub/kling-image-o1", "tokenhub/kling-image-v3-omni"] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).and(path("/wand/kling-image/generation"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": { "task_id": "ko2", "task_status": "submitted" } })))
                .mount(&server).await;
            Mock::given(method("GET")).and(path("/wand/kling-image/tasks/ko2"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": { "task_id": "ko2", "task_status": "failed", "task_status_msg": "stop" } })))
                .mount(&server).await;
            let p = pooled(&server.uri());
            let _ = p.generate(&ref_schema(id), &base("a lighthouse"), &extras(), &no_refs()).await;
            let one = MaterializedRequest {
                refs: vec![MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64("AAAA".into()), time_s: None }],
                cleanup: Cleanup::empty(),
            };
            let _ = p.generate(&ref_schema(id), &base("the same, at dusk"), &extras(), &one).await;
            let bodies: Vec<Value> = server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|r| r.url.path() == "/wand/kling-image/generation")
                .map(|r| serde_json::from_slice(&r.body).unwrap())
                .collect();
            assert_eq!(bodies[0]["aspect_ratio"], "16:9", "{id} text-to-image: {}", bodies[0]);
            assert!(bodies[1].get("aspect_ratio").is_none(), "{id} with images: {}", bodies[1]);
            match ref_schema(id).params.get("aspect_ratio") {
                Some(crate::capabilities::ParamSpec::AspectRatio(a)) => {
                    assert!(a.allowed.iter().any(|r| r == KLING_OMNI_TEXT_TO_IMAGE_RATIO), "{id}: {:?}", a.allowed)
                }
                other => panic!("{id} aspect_ratio is {other:?}"),
            }
        }
    }

    /// Vidu's image API resold: `images` as data URIs (≤ 7), litegen's 1K as
    /// Vidu's "1080p"; polled at /wand/vidu-image/tasks/{task_id} ("created /
    /// queueing / processing / success / failed"), `creations[0].url`.
    /// @see <https://cloud.tencent.com/document/product/1823/135746> (2026-11-09)
    #[tokio::test]
    async fn vidu_image_submits_polls_and_downloads() {
        let server = MockServer::start().await;
        let uri = server.uri();
        Mock::given(method("POST")).and(path("/wand/vidu-image/generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "vi1", "state": "created" })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/wand/vidu-image/tasks/vi1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "state": "success", "creations": [{ "url": format!("{uri}/result/main.png") }] })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/result/main.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(PNG.to_vec())).mount(&server).await;
        let mut e = extras();
        e.resolution = Some("1K".into());
        let mut b = base("a lantern festival");
        b.seed = Some(9);
        pooled(&server.uri()).generate(&ref_schema("tokenhub/vidu-image-q2"), &b, &e, &no_refs()).await.expect("generated");
        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received.iter().find(|r| r.url.path() == "/wand/vidu-image/generation").unwrap().body).unwrap();
        assert_eq!(body, json!({ "model": "vidu-image-q2", "prompt": "a lantern festival", "resolution": "1080p", "seed": 9 }));
    }

    /// Each async route's tier table maps every declared tier to the vendor's spelling.
    #[test]
    fn every_async_tier_is_one_of_the_vendors() {
        use crate::capabilities::schema::ParamSpec;
        for id in ["tokenhub/kling-image-v3", "tokenhub/kling-image-o1", "tokenhub/kling-image-v3-omni", "tokenhub/vidu-image-q2"] {
            let Some(ParamSpec::String(tiers)) = ref_schema(id).params.get("resolution").cloned() else { panic!("{id}") };
            for t in &tiers.enum_values {
                let wire = async_tier(id, t).unwrap_or_else(|| panic!("{id}: no wire tier for {t}"));
                assert!(["1k", "2k", "4k", "1080p", "2K", "4K"].contains(&wire), "{id}: {t} → {wire}");
            }
        }
        assert_eq!(async_tier("tokenhub/kling-image-o1", "4K"), None, "4K is v3-omni's only");
    }

    /// Every TokenHub image row resolves to an endpoint; nothing else does.
    #[test]
    fn every_tokenhub_image_row_has_an_endpoint() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let reg = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        let rows: Vec<_> = reg.for_provider("tokenhub").filter(|m| m.media_type == crate::capabilities::MediaType::Image).collect();
        assert!(!rows.is_empty());
        for m in rows {
            assert!(route(&m.id).is_some(), "{} has no endpoint", m.id);
        }
        assert!(route("tokenhub/hy-image-v3").is_none(), "retired 2026-11-12, not carried");
        assert!(route("tokenhub/seedream-image-v5.0-lite").is_none(), "retires 2026-11-24, not carried");
    }

    /// A failure frame ("仅失败终态帧才带，OpenAI 风格 {type, code, message, request_id}")
    /// and the documented refusals (422 moderation) end the call with the
    /// vendor's reason, not sent again; a 429 or 500 may be sent again.
    #[tokio::test]
    async fn a_failure_frame_or_a_refusal_names_the_vendors_reason() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path(PATH)).and(header("authorization", "Bearer th-pooled"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "abc123", "object": "image.chat.completion.chunk", "round": 0,
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "error" }],
                "error": { "type": "invalid_request_error", "code": "content_filter", "message": "input moderation rejected", "request_id": "x" }
            })))
            .mount(&server).await;
        let p = pooled(&server.uri());
        match p.generate(&ref_schema(ID), &base("x"), &extras(), &no_refs()).await {
            Err(ProviderError::RequestFailed { message, retryable: false, .. }) => {
                assert!(message.contains("content_filter") && message.contains("input moderation rejected"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        for (status, retryable) in [(422, false), (401, false), (429, true), (500, true)] {
            server.reset().await;
            Mock::given(method("POST")).and(path(PATH))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({ "error": { "code": "1300", "message": "Trigger the platform strategy" } })))
                .mount(&server).await;
            let err = p.generate(&ref_schema(ID), &base("x"), &extras(), &no_refs()).await.expect_err("failed");
            assert_eq!(err.is_retryable(), retryable, "HTTP {status}: {err:?}");
            assert!(err.to_string().contains("Trigger the platform strategy"), "HTTP {status}: {err}");
        }
    }

    /// `extra` adds the endpoint's own fields (footnote, use_search_tool) and
    /// cannot replace one litegen sets.
    #[tokio::test]
    async fn extra_adds_fields_but_cannot_replace_what_litegen_sets() {
        let server = vendor().await;
        let mut e = extras();
        e.extra = Some(json!({ "model": "hy-image-v3", "messages": [], "size": "512x512", "footnote": "litegen", "use_search_tool": { "value": true } }));
        pooled(&server.uri()).generate(&ref_schema(ID), &base("a fox"), &e, &no_refs()).await.expect("generated");
        let body = sent_body(&server).await;
        assert_eq!(body["model"], "hy-image-v3.5-preview");
        assert_eq!(body["messages"][0]["content"][0]["text"], "a fox");
        assert!(body.get("size").is_none(), "{body}");
        assert_eq!(body["footnote"], "litegen");
        assert_eq!(body["use_search_tool"], json!({ "value": true }));
    }
}
