use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::image::tokenhub::TOKENHUB_API_BASE;
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// How a TokenHub video API is spoken: its request and answer shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Hy 生视频: flat fields (prompt, image, aspect_ratio…); the task is
    /// `task_id`, the video `videos[0].url`.
    HyVideo,
    /// WAND-Vega: a `content` list of text and roled image parts, `ratio`;
    /// the task is `id` (HTTP 201), the video `content.video_url`.
    WandVega,
    /// Kling, resold: Kling's own `settings` / `contents` with `model` in the
    /// body, `{code, data: {id}}`, polled as Kling's `data[]`
    /// (`video::kling`'s shared code).
    Kling,
    /// MiniMax v2.3, resold: flat fields (first_frame_image…), `{task_id,
    /// base_resp}`, polled as "Processing / Success / Fail" with content.url.
    MiniMaxFlat,
    /// MiniMax H3, resold: a `content` list with roled image_url parts,
    /// polled as task.status "queued / running / succeeded / failed / cancelled".
    MiniMaxContent,
    /// PixVerse, resold: PixVerse's own fields on four endpoints, `{ErrCode,
    /// Resp: {video_id}}`, polled as `Resp.status` 1 / 5 / 6 / 7 / 8.
    PixVerse,
    /// Vidu, resold: Vidu's own fields on four endpoints, `{task_id, state}`,
    /// polled as `state` created / queueing / processing / success / failed.
    Vidu,
    /// YouTu on TokenHub's OpenAI-style video API: `/api/video/submit` and a
    /// POSTed `/api/video/query {model, id}`; `{status, progress, data: {url}}`.
    YtVideo,
}

/// A TokenHub video row's submit endpoint, poll path prefix, wire `model`
/// and shape.
#[derive(Debug, Clone, Copy)]
struct Route {
    submit: &'static str,
    tasks: &'static str,
    native: &'static str,
    shape: Shape,
}

/// The route of a TokenHub video row; `None` for any id the catalog does not
/// carry (never a default model: a typo would bill as another model).
///
/// @see <https://cloud.tencent.com/document/product/1823/137202> — Hy-Video 1.5 (2026-11-09)
///   Verbatim: "接口： POST https://tokenhub.tencentmaas.com/v1/wand/hunyuan-video/generation"
///   Verbatim: "接口： GET https://tokenhub.tencentmaas.com/v1/wand/hunyuan-video/tasks/{task_id}"
///   Verbatim: "model | 是 | string | 模型版本。取值：hy-video-v1.5"
/// @see <https://cloud.tencent.com/document/product/1823/138195> — WAND-Vega-Video-1.0-Lite (2026-11-09)
///   Verbatim: "接口：POST https://tokenhub.tencentmaas.com/v1/wand/vega-videos/tasks"
///   Verbatim: "接口：GET https://tokenhub.tencentmaas.com/v1/wand/vega-videos/tasks/{id}"
///   Verbatim: "model | 是 | string | 模型名称。取值： wand-vega-video-1.0-lite"
/// @see <https://cloud.tencent.com/document/product/1823/130055> — "Hy-Video-1.5 … 0.3 元/秒"; "WAND-Vega-Video-1.0-Lite … 768P … 输出视频0.375元/秒" (2026-11-09)
fn route(model_id: &str) -> Option<Route> {
    let (submit, tasks, native, shape) = match model_id {
        "tokenhub/hy-video-v1.5" => ("/wand/hunyuan-video/generation", "/wand/hunyuan-video/tasks", "hy-video-v1.5", Shape::HyVideo),
        "tokenhub/wand-vega-video-1.0-lite" => ("/wand/vega-videos/tasks", "/wand/vega-videos/tasks", "wand-vega-video-1.0-lite", Shape::WandVega),
        // Kling resold: "POST https://tokenhub.tencentmaas.com/v1/wand/kling/text-to-video" (and
        // /image-to-video), "model … 取值：kling-video-v4、kling-video-v2.5-turbo、kling-video-v2.6、
        // kling-video-v3-turbo、kling-video-v3"; "GET …/v1/wand/kling/tasks/{task_id}".
        // @see <https://cloud.tencent.com/document/product/1823/135742> (2026-11-09)
        "tokenhub/kling-video-v2.5-turbo" => ("/wand/kling", "/wand/kling/tasks", "kling-video-v2.5-turbo", Shape::Kling),
        "tokenhub/kling-video-v2.6" => ("/wand/kling", "/wand/kling/tasks", "kling-video-v2.6", Shape::Kling),
        "tokenhub/kling-video-v3" => ("/wand/kling", "/wand/kling/tasks", "kling-video-v3", Shape::Kling),
        "tokenhub/kling-video-v3-turbo" => ("/wand/kling", "/wand/kling/tasks", "kling-video-v3-turbo", Shape::Kling),
        "tokenhub/kling-video-v4" => ("/wand/kling", "/wand/kling/tasks", "kling-video-v4", Shape::Kling),
        // "POST https://tokenhub.tencentmaas.com/v1/wand/kling/omni-video", "model … 取值：kling-video-v4、
        // kling-video-v3-omni、kling-video-o1" (v4 stays on text/image-to-video).
        "tokenhub/kling-video-v3-omni" => ("/wand/kling", "/wand/kling/tasks", "kling-video-v3-omni", Shape::Kling),
        "tokenhub/kling-video-o1" => ("/wand/kling", "/wand/kling/tasks", "kling-video-o1", Shape::Kling),
        // MiniMax resold: "POST https://tokenhub.tencentmaas.com/v1/wand/minimax-video/generation"
        // (v2.3, "扁平参数") and ".../minimax-video-v2/generation" (h3 / h3-max, "content 数组").
        // @see <https://cloud.tencent.com/document/product/1823/135737> (2026-11-09)
        "tokenhub/minimax-video-v2.3" => ("/wand/minimax-video/generation", "/wand/minimax-video/tasks", "minimax-video-v2.3", Shape::MiniMaxFlat),
        "tokenhub/minimax-video-v2.3-fast" => ("/wand/minimax-video/generation", "/wand/minimax-video/tasks", "minimax-video-v2.3-fast", Shape::MiniMaxFlat),
        "tokenhub/minimax-video-h3" => ("/wand/minimax-video-v2/generation", "/wand/minimax-video-v2/tasks", "minimax-video-h3", Shape::MiniMaxContent),
        "tokenhub/minimax-video-h3-max" => ("/wand/minimax-video-v2/generation", "/wand/minimax-video-v2/tasks", "minimax-video-h3-max", Shape::MiniMaxContent),
        // PixVerse resold: "POST https://tokenhub.tencentmaas.com/v1/wand/pixverse/text-to-video"
        // (and image-to-video, start-end-to-video, reference-to-video), "model … 取值范围：
        // pixverse-video-v5.6、pixverse-video-v6.0、pixverse-video-c1".
        // @see <https://cloud.tencent.com/document/product/1823/135325> (2026-11-09)
        "tokenhub/pixverse-video-v6.0" => ("/wand/pixverse", "/wand/pixverse/tasks", "pixverse-video-v6.0", Shape::PixVerse),
        "tokenhub/pixverse-video-c1" => ("/wand/pixverse", "/wand/pixverse/tasks", "pixverse-video-c1", Shape::PixVerse),
        "tokenhub/pixverse-video-v5.6" => ("/wand/pixverse", "/wand/pixverse/tasks", "pixverse-video-v5.6", Shape::PixVerse),
        // Vidu resold: "POST https://tokenhub.tencentmaas.com/v1/wand/vidu/text-to-video" (and
        // image-to-video, start-end-to-video, reference-to-video); "GET …/v1/wand/vidu/tasks/{task_id}".
        // @see <https://cloud.tencent.com/document/product/1823/135756> (2026-11-09)
        "tokenhub/vidu-video-q4-preview" => ("/wand/vidu", "/wand/vidu/tasks", "vidu-video-q4-preview", Shape::Vidu),
        "tokenhub/vidu-video-q3-pro" => ("/wand/vidu", "/wand/vidu/tasks", "vidu-video-q3-pro", Shape::Vidu),
        "tokenhub/vidu-video-q3-turbo" => ("/wand/vidu", "/wand/vidu/tasks", "vidu-video-q3-turbo", Shape::Vidu),
        "tokenhub/vidu-video-q3" => ("/wand/vidu", "/wand/vidu/tasks", "vidu-video-q3", Shape::Vidu),
        "tokenhub/vidu-video-q2" => ("/wand/vidu", "/wand/vidu/tasks", "vidu-video-q2", Shape::Vidu),
        "tokenhub/vidu-video-q2-pro" => ("/wand/vidu", "/wand/vidu/tasks", "vidu-video-q2-pro", Shape::Vidu),
        "tokenhub/vidu-video-q2-turbo" => ("/wand/vidu", "/wand/vidu/tasks", "vidu-video-q2-turbo", Shape::Vidu),
        // YouTu: "base_url：https://tokenhub.tencentmaas.com/v1/api/video/submit", ".../v1/api/video/query".
        // @see <https://cloud.tencent.com/document/product/1823/135716> (2026-11-09)
        "tokenhub/yt-video-2.0" => ("/api/video/submit", "/api/video/query", "yt-video-2.0", Shape::YtVideo),
        _ => return None,
    };
    Some(Route { submit, tasks, native, shape })
}

/// WAND-Vega's `role` for a litegen role: "首帧取 first_frame，尾帧取 last_frame，
/// 参考图片取 reference_image".
fn wand_role(role: &str) -> Option<&'static str> {
    match role {
        "first_frame" => Some("first_frame"),
        "last_frame" => Some("last_frame"),
        "reference" => Some("reference_image"),
        _ => None,
    }
}

/// Fields litegen sets on a submit, which `extra` cannot replace (rule 11),
/// and `n` and `image_url`, which never go out: one video is billed, and the
/// frame goes as `image`.
const OWNED: &[&str] = &[
    "model", "prompt", "negative_prompt", "image", "image_url", "n", "duration", "resolution", "aspect_ratio", "content", "ratio", "options",
    "first_frame_image", "last_frame_image", "extra", "prompt_expansion_mode",
    "quality", "img_id", "img_ids", "first_frame_img", "last_frame_img", "image_references", "seed", "generate_audio_switch",
];

/// The vendor's reason from an answer's `error: {code, message}` ("统一错误信封").
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

/// The frame of a materialized ref, as the data URI (or URL) TokenHub takes.
fn image_of(form: &MaterializedRefForm) -> Option<String> {
    match form {
        // "Base64：格式要求：data:image/<图片格式>;base64,<Base64 编码>": the bytes' own type.
        MaterializedRefForm::Base64(b64) => Some(crate::proxy::materializer::base64_data_uri(b64, Some("image/png"))),
        MaterializedRefForm::Url(u) => Some(u.clone()),
        MaterializedRefForm::MultipartField { .. } => None,
    }
}

/// Hy 生视频's body: prompt, negative_prompt, image (data URI), duration,
/// resolution, and aspect_ratio on text-to-video only.
fn hy_video_body(native: &str, base: &BaseGenerationRequest, extras: &VideoExtras, materialized: &MaterializedRequest) -> Result<Value, ProviderError> {
    // "携带 image（Base64）或 image_url（图片 URL）即为图生视频".
    let image = materialized.refs.iter().find(|r| r.role == "first_frame").and_then(|r| image_of(&r.form));
    let prompt = base.prompt.trim();
    // "prompt | 条件必选 | … 文生视频必选；图生视频可选".
    if prompt.is_empty() && image.is_none() {
        return Err(ProviderError::InvalidRequest("tokenhub: text-to-video needs a prompt; attach a first_frame to animate an image".into()));
    }
    let mut body = json!({ "model": native });
    if !prompt.is_empty() {
        body["prompt"] = json!(prompt);
    }
    if let Some(neg) = base.negative_prompt.as_deref().filter(|n| !n.trim().is_empty()) {
        body["negative_prompt"] = json!(neg);
    }
    // "视频时长（秒），小数将截断为整数。当前版本固定生成 5 秒。"
    body["duration"] = json!(extras.duration_seconds.trunc() as i64);
    // "清晰度。当前版本固定输出 720p。"
    body["resolution"] = json!(extras.resolution.as_deref().unwrap_or("720p"));
    match image {
        Some(image) => body["image"] = json!(image),
        // "画面宽高比，仅文生视频有效" — an image sets its own ratio.
        None => {
            if let Some(ratio) = extras.aspect_ratio.as_deref() {
                body["aspect_ratio"] = json!(ratio);
            }
        }
    }
    Ok(body)
}

/// WAND-Vega's body: `content` = the prompt's text part, then the image
/// parts with their role (first frame before last frame); `resolution`
/// (768P when none is asked: the guide gives no default), `ratio`, an
/// integer `duration` ("4 ~ 15 的整数").
fn wand_body(native: &str, base: &BaseGenerationRequest, extras: &VideoExtras, materialized: &MaterializedRequest) -> Result<Value, ProviderError> {
    let has = |role: &str| materialized.refs.iter().any(|r| r.role == role);
    let frames = has("first_frame") || has("last_frame");
    // "三种生成形态（文生、首尾帧生、参考生）": one at a time.
    if frames && has("reference") {
        return Err(ProviderError::InvalidRequest("tokenhub: WAND-Vega takes first/last frames or reference images, not both".into()));
    }
    if has("last_frame") && !has("first_frame") {
        return Err(ProviderError::InvalidRequest("tokenhub: a last_frame needs a first_frame".into()));
    }
    let prompt = base.prompt.trim();
    // "type=text 时必填且不可为空串".
    if prompt.is_empty() {
        return Err(ProviderError::InvalidRequest("tokenhub: WAND-Vega needs a prompt".into()));
    }
    let mut content = vec![json!({ "type": "text", "text": prompt })];
    for role in ["first_frame", "last_frame", "reference"] {
        for r in materialized.refs.iter().filter(|r| r.role == role) {
            if let (Some(url), Some(wrole)) = (image_of(&r.form), wand_role(role)) {
                content.push(json!({ "type": "image_url", "image_url": { "url": url }, "role": wrole }));
            }
        }
    }
    let mut body = json!({
        "model": native,
        "content": content,
        "resolution": extras.resolution.as_deref().unwrap_or("768P"),
        "duration": extras.duration_seconds.round() as i64,
    });
    if let Some(ratio) = extras.aspect_ratio.as_deref() {
        body["ratio"] = json!(ratio);
    }
    Ok(body)
}

/// MiniMax v2.3's flat body: prompt, an integer duration ("768P：6 或 10；1080P：仅
/// 6"), resolution, first_frame_image / last_frame_image as data URIs.
fn minimax_flat_body(native: &str, base: &BaseGenerationRequest, extras: &VideoExtras, materialized: &MaterializedRequest) -> Result<Value, ProviderError> {
    let frame = |role: &str| materialized.refs.iter().find(|r| r.role == role).and_then(|r| image_of(&r.form));
    let (first, last) = (frame("first_frame"), frame("last_frame"));
    if last.is_some() && first.is_none() {
        return Err(ProviderError::InvalidRequest("tokenhub: a last_frame needs a first_frame".into()));
    }
    let prompt = base.prompt.trim();
    if prompt.is_empty() && first.is_none() {
        return Err(ProviderError::InvalidRequest("tokenhub: text-to-video needs a prompt; attach a first_frame to animate an image".into()));
    }
    let resolution = extras.resolution.as_deref().unwrap_or("768P");
    let duration = extras.duration_seconds.round() as i64;
    if resolution == "1080P" && duration != 6 {
        return Err(ProviderError::InvalidRequest(format!("tokenhub: {native} makes 1080P only at 6 s (\"1080P：仅 6\")")));
    }
    let mut body = json!({ "model": native, "duration": duration, "resolution": resolution });
    if !prompt.is_empty() {
        body["prompt"] = json!(prompt);
    }
    if let Some(f) = first {
        body["first_frame_image"] = json!(f);
    }
    if let Some(l) = last {
        body["last_frame_image"] = json!(l);
    }
    Ok(body)
}

/// MiniMax H3's `content` body: a text part, then the frames (first before
/// last) or the references as image_url parts with their role; resolution
/// and duration always ("必选"); `ratio` on text-to-video only ("文生视频必填且不能为
/// adaptive"; 16:9 when none is asked); H3-Max's prompt_expansion_mode as
/// `extra.prompt_expansion_mode`.
fn minimax_content_body(native: &str, base: &BaseGenerationRequest, extras: &VideoExtras, materialized: &MaterializedRequest) -> Result<Value, ProviderError> {
    let has = |role: &str| materialized.refs.iter().any(|r| r.role == role);
    if (has("first_frame") || has("last_frame")) && has("reference") {
        return Err(ProviderError::InvalidRequest("tokenhub: MiniMax H3 takes first/last frames or reference images, not both".into()));
    }
    if has("last_frame") && !has("first_frame") {
        return Err(ProviderError::InvalidRequest("tokenhub: a last_frame needs a first_frame".into()));
    }
    let mut content = vec![json!({ "type": "text", "text": base.prompt })];
    for (role, wire) in [("first_frame", "first_frame"), ("last_frame", "last_frame"), ("reference", "reference_image")] {
        for r in materialized.refs.iter().filter(|r| r.role == role) {
            if let Some(url) = image_of(&r.form) {
                content.push(json!({ "type": "image_url", "image_url": { "url": url }, "role": wire }));
            }
        }
    }
    let mut body = json!({
        "model": native,
        "content": content,
        "resolution": extras.resolution.as_deref().unwrap_or("768P"),
        "duration": extras.duration_seconds.round() as i64,
    });
    if !has("first_frame") && !has("reference") {
        body["ratio"] = json!(extras.aspect_ratio.as_deref().unwrap_or("16:9"));
    } else if has("reference") {
        if let Some(ratio) = extras.aspect_ratio.as_deref() {
            body["ratio"] = json!(ratio);
        }
    }
    if let Some(mode) = extras.extra.as_ref().and_then(|e| e.get("prompt_expansion_mode")) {
        body["extra"] = json!({ "prompt_expansion_mode": mode });
    }
    Ok(body)
}

/// PixVerse's poll answer: "Resp.status | integer | 视频状态：1 成功 / 5 生成中 / 6 已删除 /
/// 7 审核失败 / 8 生成失败", "Resp.url"; a non-zero ErrCode is an error.
fn pixverse_poll_result(data: &Value) -> Result<VideoGenerationPollResult, ProviderError> {
    if let Some(code) = data["ErrCode"].as_i64().filter(|c| *c != 0) {
        return Err(ProviderError::RequestFailed {
            message: format!("TokenHub PixVerse poll answered ErrCode {code}: {}", data["ErrMsg"].as_str().unwrap_or("")),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        });
    }
    let url = data["Resp"]["url"].as_str().filter(|u| !u.is_empty()).map(String::from);
    let (status, error) = match data["Resp"]["status"].as_i64() {
        Some(1) if url.is_some() => (GenerationStatus::Completed, None),
        Some(1) => (GenerationStatus::Failed, Some("TokenHub reported success without a video URL".to_string())),
        Some(5) => (GenerationStatus::Processing, None),
        Some(6) => (GenerationStatus::Failed, Some("PixVerse deleted the result (status 6)".to_string())),
        Some(7) => (GenerationStatus::Failed, Some("PixVerse content moderation failed (status 7)".to_string())),
        Some(8) => (GenerationStatus::Failed, Some("PixVerse generation failed (status 8)".to_string())),
        other => return Ok(super::unknown_state("tokenhub", other.map(|n| n.to_string()).as_deref())),
    };
    Ok(VideoGenerationPollResult {
        status,
        progress: if status.is_terminal() { 100 } else { 50 },
        video_url: url.filter(|_| status == GenerationStatus::Completed),
        video_data: None,
        content_type: Some("video/mp4".into()),
        error,
        metadata: HashMap::new(),
    })
}

/// YouTu's query answer: `status` (queued / in_progress / completed / failed,
/// the /v1/api family's states), `progress`, `data.url`.
fn yt_poll_result(data: &Value) -> VideoGenerationPollResult {
    let url = data["data"]["url"].as_str().filter(|u| !u.is_empty()).map(String::from);
    let (status, error) = match data["status"].as_str() {
        Some("queued") => (GenerationStatus::Pending, None),
        Some("in_progress") => (GenerationStatus::Processing, None),
        Some("completed") if url.is_some() => (GenerationStatus::Completed, None),
        Some("completed") => (GenerationStatus::Failed, Some("TokenHub reported success without a video URL".to_string())),
        Some("failed") => (GenerationStatus::Failed, Some(vendor_reason(data).unwrap_or_else(|| "TokenHub task failed".into()))),
        other => return super::unknown_state("tokenhub", other),
    };
    let progress = match status {
        GenerationStatus::Completed | GenerationStatus::Failed => 100,
        _ => data["progress"].as_u64().map(|p| p.min(99) as u8).unwrap_or(50),
    };
    VideoGenerationPollResult {
        status,
        progress,
        video_url: url.filter(|_| status == GenerationStatus::Completed),
        video_data: None,
        content_type: Some("video/mp4".into()),
        error,
        metadata: HashMap::new(),
    }
}

/// Tencent TokenHub video models: submit a task, then poll it by its
/// `task_id` (the handle holds only that: the poll path depends on nothing
/// else). The finished video is a signed COS link, fetched without
/// credentials.
///
/// @see <https://cloud.tencent.com/document/product/1823/137202> — auth, submit, poll, errors
///   Verbatim: "鉴权方式为请求头 Authorization: Bearer YOUR_API_KEY。"
///   Verbatim: "本系列接口响应不使用 code / message / data 统一信封：提交成功返回 {"task_id": "...", "created": ..., "request_id": "..."}，查询返回任务对象（含 task_id / status / videos / usage 等字段）。任务状态统一为：queued（等待调度）/ running（生成中）/ succeeded（成功）/ failed（失败）。"
///   Verbatim: "提交失败返回统一错误信封： task_id … （未能创建时为空字符串） status … 固定 failed。"
///   Verbatim: "401 1000 Authentication failed … 429 1302 Too frequent API calls … 500 5000 Internal server error … 503 5001 … 504 5002"
pub struct TokenHubVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl TokenHubVideoProvider {
    fn api_client(extra_headers: &HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(60), extra_headers)
    }

    pub fn new() -> Self {
        Self { config: None, key_pool: None, auth: AuthSpec::bearer(), client: Self::api_client(&Default::default()) }
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

impl TokenHubVideoProvider {
    /// Kling resold on TokenHub: the frames, TokenHub's rules, Kling's
    /// `settings` and `contents` (`video::kling`), `model` in the body.
    async fn kling_submit(
        &self,
        model: &ModelSchema,
        route: Route,
        native: &str,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let (first, tail) = super::kling::kling_frames(materialized, true);
        // TokenHub's tables, refused before any call: "注意：V2.6 使用首尾帧生成时仅支持
        // 720p、audio=native 时仅支持 1080p；V2.5 Turbo 使用首尾帧生成时仅支持 1080p".
        // @see <https://cloud.tencent.com/document/product/1823/135742> (2026-11-09)
        let refuse = |why: &str| Err(ProviderError::InvalidRequest(format!("{}: {why}", model.id)));
        let resolution = extras.resolution.as_deref().or(match route.native {
            "kling-video-v4" => Some("1080p"),
            _ => Some("720p"),
        });
        if tail.is_some() && first.is_none() {
            return refuse("a last_frame needs a first_frame");
        }
        match route.native {
            "kling-video-v2.5-turbo" if tail.is_some() && resolution != Some("1080p") => return refuse("first and last frames need resolution 1080p"),
            "kling-video-v2.6" if tail.is_some() && resolution != Some("720p") => return refuse("first and last frames need resolution 720p"),
            "kling-video-v2.6" if extras.audio == Some(true) && (resolution != Some("1080p") || tail.is_some()) => {
                return refuse("native audio needs resolution 1080p, and no last frame");
            }
            _ => {}
        }
        // "O1 仅用首帧且无其他参考时仅支持 5 / 10"
        if route.native == "kling-video-o1" && first.is_some() && tail.is_none() && ![5.0, 10.0].contains(&extras.duration_seconds) {
            return refuse("a first frame alone makes 5 or 10 s on Kling O1");
        }
        let creds = self.creds()?;
        let image = first.is_some();
        let settings = super::kling::kling_settings(model, extras, image);
        let omni = matches!(route.native, "kling-video-v3-omni" | "kling-video-o1");
        let (kind, body) = if omni {
            // The omni endpoint takes `contents` for text and frames alike.
            ("omni-video", json!({ "model": native, "contents": super::kling::kling_contents(&base.prompt, first, tail), "settings": settings }))
        } else if image {
            ("image-to-video", json!({ "model": native, "contents": super::kling::kling_contents(&base.prompt, first, tail), "settings": settings }))
        } else {
            ("text-to-video", json!({ "model": native, "prompt": base.prompt, "settings": settings }))
        };
        let url = format!("{}{}/{kind}", self.api_base(), route.submit);
        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("TokenHub request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_connect(),
        })?;
        let data = super::kling::read_kling(resp, "via TokenHub submit").await?;
        // "data.id | string | 系统生成的任务 ID，用于后续任务查询。"
        let Some(task_id) = data["data"]["id"].as_str().filter(|t| !t.is_empty()) else {
            return Err(ProviderError::RequestFailed {
                message: format!("TokenHub {native} answer names no data.id"),
                status_code: None,
                provider_error: Some(data),
                retryable: false,
            });
        };
        Ok(VideoGenerationHandle { provider_job_id: task_id.to_string(), provider: "tokenhub".into(), model: model.id.clone() })
    }
}

impl TokenHubVideoProvider {
    /// PixVerse resold on TokenHub: the endpoint follows the images, with
    /// PixVerse's own fields.
    async fn pixverse_submit(
        &self,
        model: &ModelSchema,
        route: Route,
        native: &str,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let refuse = |why: &str| Err(ProviderError::InvalidRequest(format!("{}: {why}", model.id)));
        let of = |role: &str| materialized.refs.iter().filter(|r| r.role == role).filter_map(|r| image_of(&r.form)).collect::<Vec<_>>();
        let (first, last, references) = (of("first_frame"), of("last_frame"), of("reference"));
        if (!first.is_empty() || !last.is_empty()) && !references.is_empty() {
            return refuse("first/last frames or reference images, not both");
        }
        if !last.is_empty() && first.is_empty() {
            return refuse("a last_frame needs a first_frame");
        }
        let quality = extras.resolution.as_deref().unwrap_or("540p");
        let duration = extras.duration_seconds.round() as i64;
        // "v5.6：5/8/10（1080p 不支持 10）"
        if route.native == "pixverse-video-v5.6" && quality == "1080p" && duration == 10 {
            return refuse("10 s is not made at 1080p (\"1080p 不支持 10\")");
        }
        let creds = self.creds()?;
        let mut body = json!({ "model": native, "prompt": base.prompt, "duration": duration, "quality": quality });
        let kind = match (first.first(), last.first(), references.is_empty()) {
            (_, _, false) => {
                body["image_references"] = Value::Array(references.into_iter().map(|u| json!({ "img_id": u })).collect());
                "reference-to-video"
            }
            (Some(f), Some(l), true) => {
                body["first_frame_img"] = json!(f);
                body["last_frame_img"] = json!(l);
                "start-end-to-video"
            }
            (Some(f), None, true) => {
                body["img_id"] = json!(f);
                "image-to-video"
            }
            (None, _, true) => "text-to-video",
        };
        // "aspect_ratio" on text-to-video and references; an image sets the shape.
        if matches!(kind, "text-to-video" | "reference-to-video") {
            if let Some(ratio) = extras.aspect_ratio.as_deref() {
                body["aspect_ratio"] = json!(ratio);
            }
        }
        // "随机种子。取值范围：0 ~ 2147483647"
        if let Some(seed) = base.seed {
            body["seed"] = json!(seed);
        }
        // "generate_audio_switch … true=有声 … false=静音"
        if let Some(audio) = extras.audio {
            body["generate_audio_switch"] = json!(audio);
        }
        if let Some(obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED.contains(&k)) {
                obj.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        let url = format!("{}{}/{kind}", self.api_base(), route.submit);
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.post(&url).header("Content-Type", "application/json").json(&body))?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("TokenHub request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_connect(),
        })?;
        let data = crate::providers::read_vendor_json(resp, "TokenHub").await?;
        // "ErrCode | integer | 错误码；0 表示成功。"
        if let Some(code) = data["ErrCode"].as_i64().filter(|c| *c != 0) {
            return Err(ProviderError::RequestFailed {
                message: format!("TokenHub {native} was not accepted: ErrCode {code}: {}", data["ErrMsg"].as_str().unwrap_or("")),
                status_code: None,
                provider_error: Some(data),
                retryable: false,
            });
        }
        // "Resp.video_id | string | 任务 ID" (a number in practice: both are read).
        let id = match &data["Resp"]["video_id"] {
            Value::String(v) if !v.is_empty() => v.clone(),
            Value::Number(n) => n.to_string(),
            _ => {
                return Err(ProviderError::RequestFailed {
                    message: format!("TokenHub {native} answer names no Resp.video_id"),
                    status_code: None,
                    provider_error: Some(data),
                    retryable: false,
                })
            }
        };
        Ok(VideoGenerationHandle { provider_job_id: id, provider: "tokenhub".into(), model: model.id.clone() })
    }
}

impl TokenHubVideoProvider {
    /// Vidu resold on TokenHub: the endpoint follows the images (references →
    /// reference-to-video, first + last → start-end-to-video, a first frame →
    /// image-to-video, else text-to-video), with Vidu's own fields.
    async fn vidu_submit(
        &self,
        model: &ModelSchema,
        route: Route,
        native: &str,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let refuse = |why: String| Err(ProviderError::InvalidRequest(format!("{}: {why}", model.id)));
        let of = |role: &str| materialized.refs.iter().filter(|r| r.role == role).filter_map(|r| image_of(&r.form)).collect::<Vec<_>>();
        let (first, last, references) = (of("first_frame"), of("last_frame"), of("reference"));
        if (!first.is_empty() || !last.is_empty()) && !references.is_empty() {
            return refuse("first/last frames or reference images, not both".into());
        }
        if !last.is_empty() && first.is_empty() {
            return refuse("a last_frame needs a first_frame".into());
        }
        let kind = if !references.is_empty() {
            "reference-to-video"
        } else if !last.is_empty() {
            "start-end-to-video"
        } else if !first.is_empty() {
            "image-to-video"
        } else if model.capabilities.text_to_video {
            "text-to-video"
        } else {
            return refuse("no text-to-video mode; attach a first_frame or reference images".into());
        };
        if kind == "start-end-to-video" {
            // "首尾帧生视频 | … q2 系列 … 1 ~ 8"; "audio … 仅 q3 系列支持".
            if route.native.starts_with("vidu-video-q2") && extras.duration_seconds > 8.0 {
                return refuse("a first/last-frame clip is at most 8 s on Vidu Q2".into());
            }
            if extras.audio == Some(true) && !route.native.starts_with("vidu-video-q3") {
                return refuse("no sound on first/last-frame clips on this model (only the Q3 series has it); ask without audio".into());
            }
        }
        if kind == "reference-to-video" {
            // "直接参考方式 ≤ 2000 字符"
            if base.prompt.chars().count() > 2000 {
                return refuse("a prompt with reference images is at most 2000 characters".into());
            }
            // "非主体调用时仅 q3 支持" (audio on references without `subjects`).
            if extras.audio == Some(true) && route.native != "vidu-video-q3" {
                return refuse("no sound with reference images on this model (only vidu-video-q3 has it); ask without audio".into());
            }
        }
        let row_default = match model.params.get("resolution") {
            Some(crate::capabilities::ParamSpec::String(s)) => s.default.clone(),
            _ => None,
        };
        let mut body = json!({
            "model": native,
            "duration": extras.duration_seconds.round() as i64,
            // The row's default when none is named: the clip runs at what it was priced at.
            "resolution": extras.resolution.clone().or(row_default).unwrap_or_else(|| "720p".into()),
        });
        if !base.prompt.trim().is_empty() {
            body["prompt"] = json!(base.prompt);
        }
        let images: Vec<String> = match kind {
            "reference-to-video" => references,
            // start-end2video reads images positionally: the start frame, then the end frame.
            "start-end-to-video" => first.into_iter().chain(last).collect(),
            "image-to-video" => first,
            _ => Vec::new(),
        };
        if !images.is_empty() {
            body["images"] = json!(images);
        }
        // Text and references take a ratio; images set their own.
        if matches!(kind, "text-to-video" | "reference-to-video") {
            if let Some(ratio) = extras.aspect_ratio.as_deref() {
                body["aspect_ratio"] = json!(ratio);
            }
        }
        if let Some(seed) = base.seed {
            body["seed"] = json!(seed);
        }
        if let Some(audio) = extras.audio {
            body["audio"] = json!(audio);
        }
        let creds = self.creds()?;
        let url = format!("{}{}/{kind}", self.api_base(), route.submit);
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.post(&url).header("Content-Type", "application/json").json(&body))?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("TokenHub request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_connect(),
        })?;
        let data = crate::providers::read_vendor_json(resp, "TokenHub").await?;
        let Some(task_id) = data["task_id"].as_str().filter(|t| !t.is_empty()).map(str::to_string) else {
            let why = vendor_reason(&data).unwrap_or_else(|| "the answer names no task_id".into());
            return Err(ProviderError::RequestFailed { message: format!("TokenHub {native} was not accepted: {why}"), status_code: None, provider_error: Some(data), retryable: false });
        };
        Ok(VideoGenerationHandle { provider_job_id: task_id, provider: "tokenhub".into(), model: model.id.clone() })
    }
}

impl TokenHubVideoProvider {
    /// YouTu on TokenHub: the old action's fields in snake_case ("参数统一以小写下划线
    /// 形式呈现"): `image` {base64 | url}, `prompt`, `resolution`, `fps`.
    async fn yt_submit(
        &self,
        model: &ModelSchema,
        route: Route,
        native: &str,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let image = materialized.refs.iter().find(|r| r.role == "first_frame").and_then(|r| match &r.form {
            // The action's Image: {Base64} or {Url}.
            MaterializedRefForm::Base64(b64) => Some(json!({ "base64": b64 })),
            MaterializedRefForm::Url(u) => Some(json!({ "url": u })),
            MaterializedRefForm::MultipartField { .. } => None,
        });
        let Some(image) = image else {
            return Err(ProviderError::InvalidRequest(format!("{}: attach a first_frame (image-to-video only)", model.id)));
        };
        let mut body = json!({ "model": native, "image": image, "resolution": extras.resolution.as_deref().unwrap_or("720p") });
        if !base.prompt.trim().is_empty() {
            body["prompt"] = json!(base.prompt);
        }
        if let Some(fps) = extras.fps {
            body["fps"] = json!(fps);
        }
        if let Some(obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED.contains(&k) || k == "fps") {
                obj.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        let creds = self.creds()?;
        let url = format!("{}{}", self.api_base(), route.submit);
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.post(&url).header("Content-Type", "application/json").json(&body))?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("TokenHub request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_connect(),
        })?;
        let data = crate::providers::read_vendor_json(resp, "TokenHub").await?;
        let Some(id) = data["id"].as_str().filter(|t| !t.is_empty()).map(str::to_string) else {
            let why = vendor_reason(&data).unwrap_or_else(|| "the answer names no id".into());
            return Err(ProviderError::RequestFailed { message: format!("TokenHub {native} was not accepted: {why}"), status_code: None, provider_error: Some(data), retryable: false });
        };
        Ok(VideoGenerationHandle { provider_job_id: id, provider: "tokenhub".into(), model: model.id.clone() })
    }
}

impl Default for TokenHubVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for TokenHubVideoProvider {
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
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let route = route(&model.id).ok_or_else(|| ProviderError::ModelNotSupported {
            model: model.id.clone(),
            provider: "tokenhub".into(),
        })?;
        let native = crate::providers::wire_model(self.config.as_ref(), &model.id, route.native).to_string();
        if route.shape == Shape::Kling {
            return self.kling_submit(model, route, &native, base, extras, materialized).await;
        }
        if route.shape == Shape::PixVerse {
            return self.pixverse_submit(model, route, &native, base, extras, materialized).await;
        }
        if route.shape == Shape::Vidu {
            return self.vidu_submit(model, route, &native, base, extras, materialized).await;
        }
        if route.shape == Shape::YtVideo {
            return self.yt_submit(model, route, &native, base, extras, materialized).await;
        }
        let mut body = match route.shape {
            Shape::HyVideo => hy_video_body(&native, base, extras, materialized)?,
            Shape::WandVega => wand_body(&native, base, extras, materialized)?,
            Shape::MiniMaxFlat => minimax_flat_body(&native, base, extras, materialized)?,
            Shape::MiniMaxContent => minimax_content_body(&native, base, extras, materialized)?,
            Shape::Kling | Shape::PixVerse | Shape::Vidu | Shape::YtVideo => unreachable!("sent above"),
        };
        // `extra` (revise, footnote) adds the endpoint's own fields; none can
        // replace an owned one (rule 11).
        if let Some(obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED.contains(&k)) {
                obj.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        let creds = self.creds()?;

        let url = format!("{}{}", self.api_base(), route.submit);
        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("TokenHub request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            // A submit that timed out may still have made a billed task
            // ("提交接口不做请求级幂等：每次提交均生成新的任务 ID").
            retryable: e.is_connect(),
        })?;
        let data = crate::providers::read_vendor_json(resp, "TokenHub").await?;
        // MiniMax's flat answer carries its verdict in base_resp ("status_code …
        // 0 表示提交成功").
        if let Some(code) = data["base_resp"]["status_code"].as_i64().filter(|c| *c != 0) {
            return Err(ProviderError::RequestFailed {
                message: format!("TokenHub {native} was not accepted: {code}: {}", data["base_resp"]["status_msg"].as_str().unwrap_or("")),
                status_code: None,
                provider_error: Some(data),
                retryable: false,
            });
        }
        let id_field = match route.shape {
            Shape::HyVideo | Shape::Kling | Shape::MiniMaxFlat | Shape::MiniMaxContent | Shape::PixVerse | Shape::Vidu => "task_id",
            Shape::YtVideo => "id",
            Shape::WandVega => "id",
        };
        let Some(task_id) = data[id_field].as_str().filter(|t| !t.is_empty()) else {
            let why = vendor_reason(&data).unwrap_or_else(|| format!("the answer names no {id_field}"));
            return Err(ProviderError::RequestFailed {
                message: format!("TokenHub {native} was not accepted: {why}"),
                status_code: None,
                provider_error: Some(data),
                retryable: false,
            });
        };
        Ok(VideoGenerationHandle { provider_job_id: task_id.to_string(), provider: "tokenhub".into(), model: model.id.clone() })
    }

    async fn poll_status(&self, handle: &VideoGenerationHandle) -> Result<VideoGenerationPollResult, ProviderError> {
        let route = route(&handle.model).ok_or_else(|| ProviderError::ModelNotSupported {
            model: handle.model.clone(),
            provider: "tokenhub".into(),
        })?;
        let creds = self.creds()?;
        let url = format!("{}{}/{}", self.api_base(), route.tasks, handle.provider_job_id);
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
        if route.shape == Shape::YtVideo {
            // "POST …/v1/api/video/query" with {model, id}.
            let query = format!("{}{}", self.api_base(), route.tasks);
            let builder = crate::providers::auth::apply(
                &self.auth,
                &creds,
                self.client.post(&query).header("Content-Type", "application/json").json(&json!({ "model": route.native, "id": handle.provider_job_id })),
            )?;
            let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
                message: format!("TokenHub poll failed: {e}"),
                status_code: None,
                provider_error: None,
                retryable: true,
            })?;
            let data = super::read_poll_json(resp, "tokenhub").await?;
            return Ok(yt_poll_result(&data));
        }
        if route.shape == Shape::Kling {
            // Kling's envelope: `code` 0, else an error (`read_kling`: 1302-1304
            // and 5000-5002 may pass; TokenHub's unified codes share the ranges).
            let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
                message: format!("TokenHub poll failed: {e}"),
                status_code: None,
                provider_error: None,
                retryable: true,
            })?;
            let data = super::kling::read_kling(resp, "via TokenHub poll").await?;
            return super::kling::kling_task_result(&data, &handle.provider_job_id, "tokenhub");
        }
        if route.shape == Shape::PixVerse {
            let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
                message: format!("TokenHub poll failed: {e}"),
                status_code: None,
                provider_error: None,
                retryable: true,
            })?;
            let data = super::read_poll_json(resp, "tokenhub").await?;
            return pixverse_poll_result(&data);
        }
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("TokenHub poll failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        let data = super::read_poll_json(resp, "tokenhub").await?;

        // Both shapes: "queued（等待调度）/ running（生成中）/ succeeded（成功）/ failed（失败）"
        // (Hy 生视频), "queued（排队中）、running（生成中）、succeeded（生成成功）、failed（生成失败）" (WAND).
        let status = match route.shape {
            // "status | string | 任务状态：Processing / Success / Fail。"
            Shape::MiniMaxFlat => match data["status"].as_str() {
                Some("Processing") => GenerationStatus::Processing,
                Some("Success") => GenerationStatus::Completed,
                Some("Fail") => GenerationStatus::Failed,
                other => return Ok(super::unknown_state("tokenhub", other)),
            },
            // "state | string | 处理状态：created / queueing / processing / success / failed"
            Shape::Vidu => match data["state"].as_str() {
                Some("created") | Some("queueing") | Some("processing") => GenerationStatus::Processing,
                Some("success") => GenerationStatus::Completed,
                Some("failed") => GenerationStatus::Failed,
                other => return Ok(super::unknown_state("tokenhub", other)),
            },
            // "task.status | string | 任务状态：queued / running / succeeded / failed / cancelled。"
            Shape::MiniMaxContent => match data["task"]["status"].as_str() {
                Some("queued") => GenerationStatus::Pending,
                Some("running") => GenerationStatus::Processing,
                Some("succeeded") => GenerationStatus::Completed,
                Some("failed") | Some("cancelled") => GenerationStatus::Failed,
                other => return Ok(super::unknown_state("tokenhub", other)),
            },
            _ => match data["status"].as_str() {
                Some("queued") => GenerationStatus::Pending,
                Some("running") => GenerationStatus::Processing,
                Some("succeeded") => GenerationStatus::Completed,
                Some("failed") => GenerationStatus::Failed,
                other => return Ok(super::unknown_state("tokenhub", other)),
            },
        };
        // "仅 succeeded 时 videos[].url 有值；failed 时查看 error 字段获取失败原因。"
        let video_url = match route.shape {
            Shape::HyVideo => data["videos"][0]["url"].as_str(),
            // "content.video_url | string | 生成视频的下载地址。临时地址，有效期 12 小时"
            Shape::WandVega => data["content"]["video_url"].as_str(),
            // "content.url | string | 生成视频的下载地址，为临时地址，有效期 12 小时"
            Shape::MiniMaxFlat | Shape::MiniMaxContent => data["content"]["url"].as_str(),
            // "creations[].url | string | 生成视频的下载地址"
            Shape::Vidu => data["creations"][0]["url"].as_str(),
            Shape::Kling | Shape::PixVerse | Shape::YtVideo => None,
        }
        .filter(|u| !u.is_empty())
        .map(String::from);
        let (status, error) = match (status, &video_url) {
            (GenerationStatus::Completed, None) => (GenerationStatus::Failed, Some("TokenHub reported success without a video URL".to_string())),
            (GenerationStatus::Failed, _) => (
                status,
                Some(
                    vendor_reason(&data)
                        .or_else(|| data["err_code"].as_str().filter(|m| !m.is_empty()).map(str::to_string))
                        // "base_resp.status_msg … status=Fail 时该字段承载任务失败原因"
                        .or_else(|| data["base_resp"]["status_msg"].as_str().filter(|m| !m.is_empty()).map(str::to_string))
                        .or_else(|| data["task"]["status"].as_str().map(|s| format!("task {s}")))
                        .unwrap_or_else(|| "TokenHub task failed".into()),
                ),
            ),
            _ => (status, None),
        };
        let mut metadata = HashMap::new();
        if let Some(tokens) = data["tokenhub_usage"]["total_tokens"].as_u64() {
            metadata.insert("tokenhub_tokens".to_string(), json!(tokens));
        }
        Ok(VideoGenerationPollResult {
            status,
            progress: if status.is_terminal() { 100 } else { 50 },
            video_url: video_url.filter(|_| status == GenerationStatus::Completed),
            video_data: None,
            content_type: Some("video/mp4".into()),
            error,
            metadata,
        })
    }

    async fn estimate_cost(&self, model: &ModelSchema, _request: &VideoGenerationRequest) -> Result<CostEstimate, ProviderError> {
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ID: &str = "tokenhub/hy-video-v1.5";
    const SUBMIT: &str = "/wand/hunyuan-video/generation";

    fn ref_schema(id: &str) -> ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load").get(id).expect("model").clone()
    }

    fn pooled(api_base: &str) -> TokenHubVideoProvider {
        let mut p = TokenHubVideoProvider::new();
        p.configure(ProviderInstanceConfig {
            api_base: Some(api_base.to_string()),
            api_keys: vec![ApiKeyEntry { key: "th-pooled".into(), weight: 1, label: None }],
            ..Default::default()
        });
        p
    }

    fn base(prompt: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(), model: ID.to_string(), n: 1, negative_prompt: None,
            seed: None, reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn extras() -> VideoExtras {
        VideoExtras {
            duration_seconds: 5.0, aspect_ratio: None, resolution: None, fps: None, extra: None, camera: None,
            audio: None, r#loop: None, source: None, direction: None, time_range: None, replace: None,
        }
    }

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    fn frame() -> MaterializedRequest {
        MaterializedRequest {
            refs: vec![MaterializedRef { role: "first_frame".into(), form: MaterializedRefForm::Base64("iVBORw0KGgoAAAANSUhEUg==".into()), time_s: None }],
            cleanup: Cleanup::empty(),
        }
    }

    async fn accepting() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path(SUBMIT))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "task_id": "251435731-WandVideo-05c4", "created": 1787108411, "request_id": "835a"
            })))
            .mount(&server).await;
        server
    }

    async fn sent(server: &MockServer) -> (Value, String) {
        let received = server.received_requests().await.unwrap();
        let r = received.iter().find(|r| r.url.path() == SUBMIT).expect("a submit");
        (serde_json::from_slice(&r.body).unwrap(), r.headers.get("authorization").unwrap().to_str().unwrap().to_string())
    }

    /// Text-to-video: "不携带图片即为文生视频"; model, prompt, negative_prompt,
    /// duration (5, "小数将截断为整数"), resolution (720p) and aspect_ratio
    /// ("仅文生视频有效"); Bearer auth; the handle is the task_id alone.
    /// @see <https://cloud.tencent.com/document/product/1823/137202> (2026-11-09)
    #[tokio::test]
    async fn text_to_video_sends_its_fields_with_the_pooled_key() {
        let server = accepting().await;
        let mut b = base("the camera pushes in slowly");
        b.negative_prompt = Some("blur".into());
        let mut e = extras();
        e.aspect_ratio = Some("9:16".into());
        e.resolution = Some("720p".into());
        let h = pooled(&server.uri()).generate(&ref_schema(ID), &b, &e, &no_refs()).await.expect("submitted");
        assert_eq!((h.provider_job_id.as_str(), h.provider.as_str(), h.model.as_str()), ("251435731-WandVideo-05c4", "tokenhub", ID));
        let (body, auth) = sent(&server).await;
        assert_eq!(auth, "Bearer th-pooled");
        assert_eq!(
            body,
            json!({
                "model": "hy-video-v1.5", "prompt": "the camera pushes in slowly", "negative_prompt": "blur",
                "duration": 5, "resolution": "720p", "aspect_ratio": "9:16"
            })
        );
    }

    /// Image-to-video: the frame as a data URI in `image`; no aspect_ratio
    /// ("图生视频的画面比例由输入图片决定"); the prompt may be left out.
    #[tokio::test]
    async fn image_to_video_sends_the_frame_as_a_data_uri_and_no_ratio() {
        let server = accepting().await;
        let mut e = extras();
        e.aspect_ratio = Some("16:9".into());
        pooled(&server.uri()).generate(&ref_schema(ID), &base(""), &e, &frame()).await.expect("submitted");
        let (body, _) = sent(&server).await;
        assert_eq!(body["image"], "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==");
        assert!(body.get("aspect_ratio").is_none(), "{body}");
        assert!(body.get("prompt").is_none(), "{body}");
        assert!(body.get("image_url").is_none() && body.get("n").is_none(), "{body}");
    }

    /// "文生视频必选": with neither a prompt nor an image nothing is sent.
    #[tokio::test]
    async fn a_request_with_neither_prompt_nor_image_is_refused_before_any_call() {
        let server = MockServer::start().await;
        let err = pooled(&server.uri()).generate(&ref_schema(ID), &base("  "), &extras(), &no_refs()).await.expect_err("refused");
        assert!(matches!(err, ProviderError::InvalidRequest(ref m) if m.contains("prompt") && m.contains("first_frame")), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// `extra` adds revise / footnote and cannot replace an owned field; `n`
    /// never goes out (one video is billed).
    #[tokio::test]
    async fn extra_adds_fields_but_cannot_replace_what_litegen_sets() {
        let server = accepting().await;
        let mut e = extras();
        e.extra = Some(json!({ "model": "kling-video-v3", "prompt": "other", "n": 4, "duration": 10, "image_url": "https://x/y.png", "revise": false, "footnote": "litegen" }));
        pooled(&server.uri()).generate(&ref_schema(ID), &base("waves"), &e, &no_refs()).await.expect("submitted");
        let (body, _) = sent(&server).await;
        assert_eq!((body["model"].as_str(), body["prompt"].as_str(), body["duration"].as_i64()), (Some("hy-video-v1.5"), Some("waves"), Some(5)));
        assert!(body.get("n").is_none() && body.get("image_url").is_none(), "{body}");
        assert_eq!((body["revise"].as_bool(), body["footnote"].as_str()), (Some(false), Some("litegen")));
    }

    /// A refused submit — "提交失败返回统一错误信封 {"task_id":"","status":"failed",
    /// "error":{…}}" — names the vendor's reason and is not sent again; a 503
    /// ("5001 服务暂不可用") may be; an answer with no task id is final.
    #[tokio::test]
    async fn a_refused_submit_names_the_vendors_reason() {
        let server = MockServer::start().await;
        let p = pooled(&server.uri());
        let envelope = json!({ "task_id": "", "status": "failed", "error": { "code": "1300", "message": "Trigger the platform strategy" } });
        for (status, body, retryable) in [(200, envelope.clone(), false), (400, envelope.clone(), false), (503, json!({ "error": { "code": "5001", "message": "Server is temporarily unavailable" } }), true), (200, json!({ "request_id": "r" }), false)] {
            server.reset().await;
            Mock::given(method("POST")).and(path(SUBMIT)).respond_with(ResponseTemplate::new(status).set_body_json(body.clone())).mount(&server).await;
            let err = p.generate(&ref_schema(ID), &base("x"), &extras(), &no_refs()).await.expect_err("refused");
            assert_eq!(err.is_retryable(), retryable, "HTTP {status} {body}: {err:?}");
            if body["error"]["message"].is_string() {
                assert!(err.to_string().contains(body["error"]["message"].as_str().unwrap()), "{err}");
            }
        }
    }

    async fn poll(server: &MockServer, answer: Value) -> Result<VideoGenerationPollResult, ProviderError> {
        server.reset().await;
        Mock::given(method("GET")).and(path("/wand/hunyuan-video/tasks/251435731-WandVideo-05c4"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .mount(server).await;
        // Rebuilt by hand, as the poller does from the row: the id alone.
        let h = VideoGenerationHandle { provider_job_id: "251435731-WandVideo-05c4".into(), provider: "tokenhub".into(), model: ID.into() };
        pooled(&server.uri()).poll_status(&h).await
    }

    /// "queued（等待调度）/ running（生成中）/ succeeded（成功）/ failed（失败）"; "仅
    /// succeeded 时 videos[].url 有值；failed 时查看 error 字段获取失败原因".
    #[tokio::test]
    async fn every_documented_status_maps() {
        let server = MockServer::start().await;
        let url = "https://aigc-output-video-1326893053.cos.ap-guangzhou.myqcloud.com/v_0.mp4?q-sign-algorithm=sha1";
        let r = poll(&server, json!({ "task_id": "t", "status": "queued", "error": null })).await.unwrap();
        assert_eq!(r.status, GenerationStatus::Pending);
        let r = poll(&server, json!({ "task_id": "t", "status": "running", "error": null })).await.unwrap();
        assert_eq!((r.status, r.progress), (GenerationStatus::Processing, 50));
        let r = poll(&server, json!({ "task_id": "t", "status": "succeeded", "videos": [{ "url": url, "audio_url": "" }], "tokenhub_usage": { "total_tokens": 150000 } })).await.unwrap();
        assert_eq!((r.status, r.progress, r.video_url.as_deref(), r.content_type.as_deref()), (GenerationStatus::Completed, 100, Some(url), Some("video/mp4")));
        let r = poll(&server, json!({ "task_id": "t", "status": "succeeded", "videos": [] })).await.unwrap();
        assert_eq!(r.status, GenerationStatus::Failed, "a success with nothing to deliver");
        let r = poll(&server, json!({ "task_id": "t", "status": "failed", "error": { "code": "InternalServiceError", "message": "generate video failed" } })).await.unwrap();
        assert_eq!(r.status, GenerationStatus::Failed);
        assert!(r.error.as_deref().is_some_and(|e| e.contains("generate video failed")), "{:?}", r.error);
        let r = poll(&server, json!({ "task_id": "t", "status": "paused" })).await.unwrap();
        assert_eq!(r.status, GenerationStatus::Failed);
        assert!(r.error.as_deref().is_some_and(|e| e.contains("paused")), "{:?}", r.error);
    }

    /// A revoked key or a purged task (401, 404) can never be learned: final.
    /// A 5xx may pass. An HTML page is an error, never "still running".
    #[tokio::test]
    async fn a_poll_401_or_404_is_final_a_5xx_is_asked_again() {
        let server = MockServer::start().await;
        let h = VideoGenerationHandle { provider_job_id: "t1".into(), provider: "tokenhub".into(), model: ID.into() };
        for (status, retryable) in [(401, false), (404, false), (500, true), (504, true)] {
            server.reset().await;
            Mock::given(method("GET")).and(path("/wand/hunyuan-video/tasks/t1"))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({ "error": { "code": "1002", "message": "Authorization is invalid" } })))
                .mount(&server).await;
            let err = pooled(&server.uri()).poll_status(&h).await.expect_err("an error");
            assert_eq!(err.is_retryable(), retryable, "HTTP {status}");
        }
        server.reset().await;
        Mock::given(method("GET")).and(path("/wand/hunyuan-video/tasks/t1"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>gateway</html>"))
            .mount(&server).await;
        assert!(pooled(&server.uri()).poll_status(&h).await.is_err());
    }

    const WAND: &str = "tokenhub/wand-vega-video-1.0-lite";
    const WAND_TASKS: &str = "/wand/vega-videos/tasks";

    async fn wand_accepting() -> MockServer {
        let server = MockServer::start().await;
        // "提交成功以 HTTP 状态码 201 + 返回 id 为准"
        Mock::given(method("POST")).and(path(WAND_TASKS))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "1300000000-video-68b7", "model": "wand-vega-video-1.0-lite", "status": "queued" })))
            .mount(&server).await;
        server
    }

    async fn wand_body(server: &MockServer) -> Value {
        let received = server.received_requests().await.unwrap();
        serde_json::from_slice(&received.iter().find(|r| r.url.path() == WAND_TASKS).expect("a submit").body).unwrap()
    }

    fn image(role: &str, b64: &str) -> MaterializedRef {
        MaterializedRef { role: role.into(), form: MaterializedRefForm::Base64(b64.into()), time_s: None }
    }

    /// WAND-Vega text-to-video: `content` holds the prompt as a text part;
    /// resolution (768P when none is asked: no documented default), ratio and
    /// an integer duration; the handle is the task `id`.
    /// @see <https://cloud.tencent.com/document/product/1823/138195> (2026-11-09)
    #[tokio::test]
    async fn wand_text_to_video_sends_a_content_list_and_its_settings() {
        let server = wand_accepting().await;
        let mut e = extras();
        e.duration_seconds = 10.0;
        e.aspect_ratio = Some("21:9".into());
        let mut b = base("a drone shot over a harbour");
        b.model = WAND.into();
        let h = pooled(&server.uri()).generate(&ref_schema(WAND), &b, &e, &no_refs()).await.expect("submitted");
        assert_eq!((h.provider_job_id.as_str(), h.model.as_str()), ("1300000000-video-68b7", WAND));
        assert_eq!(
            wand_body(&server).await,
            json!({
                "model": "wand-vega-video-1.0-lite",
                "content": [{ "type": "text", "text": "a drone shot over a harbour" }],
                "resolution": "768P", "ratio": "21:9", "duration": 10
            })
        );
    }

    /// Frames and references are image_url parts with their role ("首帧取
    /// first_frame，尾帧取 last_frame，参考图片取 reference_image"), after the text.
    #[tokio::test]
    async fn wand_frames_and_references_go_as_roled_image_parts() {
        let server = wand_accepting().await;
        let p = pooled(&server.uri());
        let frames = MaterializedRequest { refs: vec![image("last_frame", "/9j/4AAQSkZJRgABAQ=="), image("first_frame", "iVBORw0KGgoAAAANSUhEUg==")], cleanup: Cleanup::empty() };
        let mut e = extras();
        e.resolution = Some("1080P".into());
        p.generate(&ref_schema(WAND), &base("a door opens"), &e, &frames).await.expect("submitted");
        let body = wand_body(&server).await;
        assert_eq!(body["resolution"], "1080P");
        assert_eq!(body["content"][1], json!({ "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==" }, "role": "first_frame" }), "the first frame comes first");
        assert_eq!(body["content"][2]["role"], "last_frame");
        server.reset().await;
        Mock::given(method("POST")).and(path(WAND_TASKS)).respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "v2", "status": "queued" }))).mount(&server).await;
        let refs = MaterializedRequest { refs: vec![image("reference", "iVBORw0KGgoAAAANSUhEUg=="), image("reference", "iVBORw0KGgoAAAANSUhEUg==")], cleanup: Cleanup::empty() };
        p.generate(&ref_schema(WAND), &base("these two characters meet"), &extras(), &refs).await.expect("submitted");
        let content = wand_body(&server).await["content"].clone();
        assert_eq!(content.as_array().map(Vec::len), Some(3));
        assert!(content.as_array().unwrap()[1..].iter().all(|c| c["role"] == "reference_image"), "{content}");
    }

    /// "三种生成形态（文生、首尾帧生、参考生）": frames and references are not mixed,
    /// and a last frame needs a first one — refused before any call.
    #[tokio::test]
    async fn wand_refuses_frames_with_references_and_a_last_frame_alone() {
        let server = MockServer::start().await;
        let p = pooled(&server.uri());
        for refs in [vec![image("first_frame", "iVBORw0KGgo="), image("reference", "iVBORw0KGgo=")], vec![image("last_frame", "iVBORw0KGgo=")]] {
            let m = MaterializedRequest { refs, cleanup: Cleanup::empty() };
            let err = p.generate(&ref_schema(WAND), &base("x"), &extras(), &m).await.expect_err("refused");
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// "queued（排队中）、running（生成中）、succeeded（生成成功）、failed（生成失败）";
    /// the video is content.video_url, a failure's reason error.message.
    #[tokio::test]
    async fn wand_statuses_map() {
        let server = MockServer::start().await;
        let h = VideoGenerationHandle { provider_job_id: "v1".into(), provider: "tokenhub".into(), model: WAND.into() };
        let url = "https://aigc-video.example.com/xxx/result.mp4";
        for (answer, want, err) in [
            (json!({ "id": "v1", "status": "queued" }), GenerationStatus::Pending, None),
            (json!({ "id": "v1", "status": "running" }), GenerationStatus::Processing, None),
            (json!({ "id": "v1", "status": "succeeded", "content": { "video_url": url } }), GenerationStatus::Completed, None),
            (json!({ "id": "v1", "status": "succeeded", "content": {} }), GenerationStatus::Failed, Some("without a video URL")),
            (json!({ "id": "v1", "status": "failed", "error": { "code": "TaskTimeout", "message": "queued too long" } }), GenerationStatus::Failed, Some("queued too long")),
            (json!({ "id": "v1", "status": "expired" }), GenerationStatus::Failed, Some("expired")),
        ] {
            server.reset().await;
            Mock::given(method("GET")).and(path("/wand/vega-videos/tasks/v1")).respond_with(ResponseTemplate::new(200).set_body_json(answer.clone())).mount(&server).await;
            let r = pooled(&server.uri()).poll_status(&h).await.expect("polled");
            assert_eq!(r.status, want, "{answer}");
            if want == GenerationStatus::Completed {
                assert_eq!(r.video_url.as_deref(), Some(url));
            }
            if let Some(e) = err {
                assert!(r.error.as_deref().is_some_and(|m| m.contains(e)), "{answer}: {:?}", r.error);
            }
        }
    }

    async fn kling_accepting() -> MockServer {
        let server = MockServer::start().await;
        for kind in ["text-to-video", "image-to-video"] {
            Mock::given(method("POST")).and(path(format!("/wand/kling/{kind}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "message": "SUCCEED", "request_id": "r", "data": { "id": "k1", "status": "submitted" } })))
                .mount(&server).await;
        }
        server
    }

    async fn kling_sent(server: &MockServer) -> (String, Value) {
        let received = server.received_requests().await.unwrap();
        let r = received.iter().find(|r| r.method == wiremock::http::Method::POST).expect("a submit");
        (r.url.path().to_string(), serde_json::from_slice(&r.body).unwrap())
    }

    /// Kling resold on TokenHub: text-to-video goes to /wand/kling/text-to-video
    /// with `model` in the body and Kling's `settings` (duration, aspect_ratio,
    /// resolution, audio native|off, V3's multi_shot); the answer's `data.id`
    /// is the handle.
    /// @see <https://cloud.tencent.com/document/product/1823/135742> (2026-11-09)
    #[tokio::test]
    async fn kling_text_to_video_sends_klings_shape_to_tokenhub() {
        let server = kling_accepting().await;
        let id = "tokenhub/kling-video-v3";
        let mut e = extras();
        e.duration_seconds = 8.0;
        e.aspect_ratio = Some("9:16".into());
        e.resolution = Some("1080p".into());
        e.audio = Some(true);
        e.extra = Some(json!({ "multi_shot": false, "duration": 99 }));
        let mut b = base("a fox in the snow");
        b.model = id.into();
        let h = pooled(&server.uri()).generate(&ref_schema(id), &b, &e, &no_refs()).await.expect("submitted");
        assert_eq!((h.provider_job_id.as_str(), h.model.as_str()), ("k1", id));
        let (at, body) = kling_sent(&server).await;
        assert_eq!(at, "/wand/kling/text-to-video");
        assert_eq!(
            body,
            json!({
                "model": "kling-video-v3", "prompt": "a fox in the snow",
                "settings": { "duration": 8, "aspect_ratio": "9:16", "resolution": "1080p", "audio": "native", "multi_shot": false }
            })
        );
    }

    /// Image-to-video: `contents` = the prompt, then the first and last frames
    /// as data URIs ("格式要求：data:image/<图片格式>;base64"); no aspect_ratio ("图生视频无
    /// aspect_ratio 参数").
    #[tokio::test]
    async fn kling_image_to_video_sends_contents_with_data_uri_frames() {
        let server = kling_accepting().await;
        let id = "tokenhub/kling-video-v4";
        let mut e = extras();
        e.aspect_ratio = Some("21:9".into());
        e.resolution = Some("1080p".into());
        let frames = MaterializedRequest { refs: vec![image("last_frame", "/9j/4AAQSkZJRgABAQ=="), image("first_frame", "iVBORw0KGgoAAAANSUhEUg==")], cleanup: Cleanup::empty() };
        pooled(&server.uri()).generate(&ref_schema(id), &base("the door opens"), &e, &frames).await.expect("submitted");
        let (at, body) = kling_sent(&server).await;
        assert_eq!(at, "/wand/kling/image-to-video");
        assert_eq!(
            body,
            json!({
                "model": "kling-video-v4",
                "contents": [
                    { "type": "prompt", "text": "the door opens" },
                    { "type": "first_frame", "url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==" },
                    { "type": "last_frame", "url": "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQ==" }
                ],
                "settings": { "duration": 5, "resolution": "1080p" }
            })
        );
    }

    /// E10: a TokenHub Kling clip asked without a `resolution` is quoted and
    /// charged at the row's default (`params.resolution.default`: 1080p on
    /// V4, 720p on the rest), so `settings.resolution` carries that default
    /// instead of leaving TokenHub to pick one, on text and frames alike.
    #[tokio::test]
    async fn kling_without_a_resolution_is_sent_at_the_rows_default() {
        let server = kling_accepting().await;
        let p = pooled(&server.uri());
        for (id, frames) in [("tokenhub/kling-video-v3", no_refs()), ("tokenhub/kling-video-v3", frame()), ("tokenhub/kling-video-v4", no_refs())] {
            let mut b = base("x");
            b.model = id.into();
            p.generate(&ref_schema(id), &b, &extras(), &frames).await.expect("submitted");
        }
        let sent: Vec<Value> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method == wiremock::http::Method::POST)
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap()["settings"]["resolution"].clone())
            .collect();
        assert_eq!(sent, [json!("720p"), json!("720p"), json!("1080p")]);
    }

    /// TokenHub's own rules, refused before any call: "V2.6 使用首尾帧生成时仅支持
    /// 720p、audio=native 时仅支持 1080p；V2.5 Turbo 使用首尾帧生成时仅支持 1080p", and a last
    /// frame needs a first.
    #[tokio::test]
    async fn tokenhubs_kling_rules_are_refused_before_any_call() {
        let server = MockServer::start().await;
        let p = pooled(&server.uri());
        let both = || MaterializedRequest { refs: vec![image("first_frame", "iVBORw0KGgo="), image("last_frame", "iVBORw0KGgo=")], cleanup: Cleanup::empty() };
        let cases: Vec<(&str, Option<&str>, Option<bool>, MaterializedRequest)> = vec![
            ("tokenhub/kling-video-v2.5-turbo", Some("720p"), None, both()),
            ("tokenhub/kling-video-v2.6", Some("1080p"), None, both()),
            ("tokenhub/kling-video-v2.6", Some("720p"), Some(true), no_refs()),
            ("tokenhub/kling-video-v3", Some("720p"), None, MaterializedRequest { refs: vec![image("last_frame", "iVBORw0KGgo=")], cleanup: Cleanup::empty() }),
        ];
        for (id, res, audio, refs) in cases {
            let mut e = extras();
            e.resolution = res.map(str::to_string);
            e.audio = audio;
            let err = p.generate(&ref_schema(id), &base("x"), &e, &refs).await.expect_err("refused");
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{id}: {err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// The poll: GET /wand/kling/tasks/{task_id} answers Kling's `data[]`
    /// ("processing / succeeded / failed", `outputs[].url`); a non-zero `code`
    /// is an error, never "still running".
    #[tokio::test]
    async fn kling_polls_map_tokenhubs_answers() {
        let server = MockServer::start().await;
        let h = VideoGenerationHandle { provider_job_id: "k1".into(), provider: "tokenhub".into(), model: "tokenhub/kling-video-v3".into() };
        let url = "https://kling-output.example/k1.mp4";
        for (answer, want) in [
            (json!({ "code": 0, "data": [{ "id": "k1", "status": "processing" }] }), Some(GenerationStatus::Processing)),
            (json!({ "code": 0, "data": [{ "id": "k1", "status": "succeeded", "outputs": [{ "type": "video", "url": url }] }] }), Some(GenerationStatus::Completed)),
            (json!({ "code": 0, "data": [{ "id": "k1", "status": "failed", "message": "content risk" }] }), Some(GenerationStatus::Failed)),
            (json!({ "code": 1203, "message": "The requested resource does not exist" }), None),
        ] {
            server.reset().await;
            Mock::given(method("GET")).and(path("/wand/kling/tasks/k1")).respond_with(ResponseTemplate::new(200).set_body_json(answer.clone())).mount(&server).await;
            match (pooled(&server.uri()).poll_status(&h).await, want) {
                (Ok(r), Some(w)) => {
                    assert_eq!(r.status, w, "{answer}");
                    if w == GenerationStatus::Completed {
                        assert_eq!(r.video_url.as_deref(), Some(url));
                    }
                    if w == GenerationStatus::Failed {
                        assert_eq!(r.error.as_deref(), Some("content risk"));
                    }
                }
                (Err(e), None) => assert!(!e.is_retryable(), "{e:?}"),
                (got, w) => panic!("{answer}: wanted {w:?}, got {got:?}"),
            }
        }
    }

    async fn minimax_accepting() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/wand/minimax-video/generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "m1", "base_resp": { "status_code": 0, "status_msg": "success" } })))
            .mount(&server).await;
        Mock::given(method("POST")).and(path("/wand/minimax-video-v2/generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "m2", "request_id": "r" })))
            .mount(&server).await;
        server
    }

    async fn posted(server: &MockServer) -> (String, Value) {
        let received = server.received_requests().await.unwrap();
        let r = received.iter().find(|r| r.method == wiremock::http::Method::POST).expect("a submit");
        (r.url.path().to_string(), serde_json::from_slice(&r.body).unwrap())
    }

    /// MiniMax v2.3 resold on TokenHub, flat fields: prompt, an integer
    /// duration, resolution, first_frame_image / last_frame_image (data URIs);
    /// "1080P：仅 6" is refused at 10 s before any call; a non-zero
    /// base_resp.status_code is the vendor's refusal.
    /// @see <https://cloud.tencent.com/document/product/1823/135737> (2026-11-09)
    #[tokio::test]
    async fn minimax_flat_sends_its_fields() {
        let server = minimax_accepting().await;
        let p = pooled(&server.uri());
        let id = "tokenhub/minimax-video-v2.3";
        let mut e = extras();
        e.duration_seconds = 10.0;
        e.resolution = Some("768P".into());
        let frames = MaterializedRequest { refs: vec![image("last_frame", "/9j/4AAQSkZJRgABAQ=="), image("first_frame", "iVBORw0KGgoAAAANSUhEUg==")], cleanup: Cleanup::empty() };
        let h = p.generate(&ref_schema(id), &base("a dancer spins"), &e, &frames).await.expect("submitted");
        assert_eq!(h.provider_job_id, "m1");
        let (at, body) = posted(&server).await;
        assert_eq!(at, "/wand/minimax-video/generation");
        assert_eq!(
            body,
            json!({
                "model": "minimax-video-v2.3", "prompt": "a dancer spins", "duration": 10, "resolution": "768P",
                "first_frame_image": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==",
                "last_frame_image": "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQ=="
            })
        );
        let quiet = MockServer::start().await;
        e.resolution = Some("1080P".into());
        let err = pooled(&quiet.uri()).generate(&ref_schema(id), &base("x"), &e, &no_refs()).await.expect_err("refused");
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        assert!(quiet.received_requests().await.unwrap().is_empty());
        server.reset().await;
        Mock::given(method("POST")).and(path("/wand/minimax-video/generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "", "base_resp": { "status_code": 1026, "status_msg": "input new_sensitive" } })))
            .mount(&server).await;
        let err = p.generate(&ref_schema(id), &base("x"), &extras(), &no_refs()).await.expect_err("refused");
        assert!(!err.is_retryable() && err.to_string().contains("input new_sensitive"), "{err}");
    }

    /// H3 / H3-Max, the `content` shape: a text part, then image_url parts with
    /// role first_frame | last_frame | reference_image; resolution and duration
    /// always (both "必选"); `ratio` on text-to-video only (16:9 when none);
    /// frames and references not mixed; H3-Max's prompt_expansion_mode goes as
    /// `extra.prompt_expansion_mode`.
    #[tokio::test]
    async fn minimax_content_sends_roled_parts() {
        let server = minimax_accepting().await;
        let p = pooled(&server.uri());
        p.generate(&ref_schema("tokenhub/minimax-video-h3"), &base("city at night"), &extras(), &no_refs()).await.expect("submitted");
        let (at, body) = posted(&server).await;
        assert_eq!(at, "/wand/minimax-video-v2/generation");
        assert_eq!(
            body,
            json!({ "model": "minimax-video-h3", "content": [{ "type": "text", "text": "city at night" }], "resolution": "768P", "duration": 5, "ratio": "16:9" })
        );
        server.reset().await;
        Mock::given(method("POST")).and(path("/wand/minimax-video-v2/generation")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "m3" }))).mount(&server).await;
        let refs = MaterializedRequest { refs: vec![image("reference", "iVBORw0KGgo="), image("reference", "iVBORw0KGgo=")], cleanup: Cleanup::empty() };
        p.generate(&ref_schema("tokenhub/minimax-video-h3"), &base("these two meet"), &extras(), &refs).await.expect("submitted");
        let (_, body) = posted(&server).await;
        assert!(body["content"].as_array().unwrap()[1..].iter().all(|c| c["type"] == "image_url" && c["role"] == "reference_image"), "{body}");
        server.reset().await;
        Mock::given(method("POST")).and(path("/wand/minimax-video-v2/generation")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "m4" }))).mount(&server).await;
        let mut e = extras();
        e.extra = Some(json!({ "prompt_expansion_mode": "quality", "aigc_watermark": true }));
        e.aspect_ratio = Some("9:16".into());
        let frame = MaterializedRequest { refs: vec![image("first_frame", "iVBORw0KGgo=")], cleanup: Cleanup::empty() };
        p.generate(&ref_schema("tokenhub/minimax-video-h3-max"), &base("go"), &e, &frame).await.expect("submitted");
        let (_, body) = posted(&server).await;
        assert_eq!(body["extra"], json!({ "prompt_expansion_mode": "quality" }));
        assert_eq!(body["aigc_watermark"], true);
        assert!(body.get("ratio").is_none(), "image-to-video follows its image: {body}");
        assert_eq!(body["content"][1]["role"], "first_frame");
        let quiet = MockServer::start().await;
        let mixed = MaterializedRequest { refs: vec![image("first_frame", "iVBORw0KGgo="), image("reference", "iVBORw0KGgo=")], cleanup: Cleanup::empty() };
        assert!(matches!(pooled(&quiet.uri()).generate(&ref_schema("tokenhub/minimax-video-h3"), &base("x"), &extras(), &mixed).await, Err(ProviderError::InvalidRequest(_))));
        assert!(quiet.received_requests().await.unwrap().is_empty());
    }

    /// Flat polls: "Processing / Success / Fail" (content.url, base_resp.status_msg);
    /// content polls: task.status "queued / running / succeeded / failed / cancelled".
    #[tokio::test]
    async fn minimax_polls_map() {
        let server = MockServer::start().await;
        let url = "https://minimax-out.example/v.mp4";
        for (model, at, answer, want, err) in [
            ("tokenhub/minimax-video-v2.3", "/wand/minimax-video/tasks/j", json!({ "task_id": "j", "status": "Processing", "base_resp": { "status_code": 0 } }), GenerationStatus::Processing, None),
            ("tokenhub/minimax-video-v2.3", "/wand/minimax-video/tasks/j", json!({ "task_id": "j", "status": "Success", "content": { "url": url }, "base_resp": { "status_code": 0 } }), GenerationStatus::Completed, None),
            ("tokenhub/minimax-video-v2.3", "/wand/minimax-video/tasks/j", json!({ "task_id": "j", "status": "Fail", "base_resp": { "status_code": 0, "status_msg": "moderation" } }), GenerationStatus::Failed, Some("moderation")),
            ("tokenhub/minimax-video-h3", "/wand/minimax-video-v2/tasks/j", json!({ "task_id": "j", "task": { "status": "queued" } }), GenerationStatus::Pending, None),
            ("tokenhub/minimax-video-h3", "/wand/minimax-video-v2/tasks/j", json!({ "task_id": "j", "task": { "status": "running" } }), GenerationStatus::Processing, None),
            ("tokenhub/minimax-video-h3", "/wand/minimax-video-v2/tasks/j", json!({ "task_id": "j", "task": { "status": "succeeded" }, "content": { "url": url } }), GenerationStatus::Completed, None),
            ("tokenhub/minimax-video-h3", "/wand/minimax-video-v2/tasks/j", json!({ "task_id": "j", "task": { "status": "cancelled" }, "base_resp": { "status_msg": "cancelled by system" } }), GenerationStatus::Failed, Some("cancelled")),
        ] {
            server.reset().await;
            Mock::given(method("GET")).and(path(at)).respond_with(ResponseTemplate::new(200).set_body_json(answer.clone())).mount(&server).await;
            let h = VideoGenerationHandle { provider_job_id: "j".into(), provider: "tokenhub".into(), model: model.into() };
            let r = pooled(&server.uri()).poll_status(&h).await.expect("polled");
            assert_eq!(r.status, want, "{answer}");
            if want == GenerationStatus::Completed {
                assert_eq!(r.video_url.as_deref(), Some(url));
            }
            if let Some(e) = err {
                assert!(r.error.as_deref().is_some_and(|m| m.contains(e)), "{answer}: {:?}", r.error);
            }
        }
    }

    async fn pixverse_accepting() -> MockServer {
        let server = MockServer::start().await;
        for kind in ["text-to-video", "image-to-video", "start-end-to-video", "reference-to-video"] {
            Mock::given(method("POST")).and(path(format!("/wand/pixverse/{kind}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ErrCode": 0, "ErrMsg": "success", "Resp": { "video_id": 4242 } })))
                .mount(&server).await;
        }
        server
    }

    /// PixVerse resold on TokenHub: the endpoint follows the images (none →
    /// text-to-video with aspect_ratio; one → image-to-video `img_id`; two →
    /// start-end-to-video; references → `image_references[]`), PixVerse's own
    /// fields (`quality`, `seed`, `generate_audio_switch`), `Resp.video_id`
    /// (a number) as the handle.
    /// @see <https://cloud.tencent.com/document/product/1823/135325> (2026-11-09)
    #[tokio::test]
    async fn pixverse_picks_its_endpoint_from_the_images() {
        let id = "tokenhub/pixverse-video-v6.0";
        let server = pixverse_accepting().await;
        let p = pooled(&server.uri());
        let mut e = extras();
        e.aspect_ratio = Some("21:9".into());
        e.audio = Some(true);
        let mut b = base("a tiger walks");
        b.seed = Some(7);
        let h = p.generate(&ref_schema(id), &b, &e, &no_refs()).await.expect("submitted");
        assert_eq!(h.provider_job_id, "4242");
        let (at, body) = posted(&server).await;
        assert_eq!(at, "/wand/pixverse/text-to-video");
        assert_eq!(body, json!({ "model": "pixverse-video-v6.0", "prompt": "a tiger walks", "duration": 5, "quality": "540p", "aspect_ratio": "21:9", "seed": 7, "generate_audio_switch": true }));
        for (refs, at, check) in [
            (vec![image("first_frame", "iVBORw0KGgo=")], "/wand/pixverse/image-to-video", "img_id"),
            (vec![image("last_frame", "iVBORw0KGgo="), image("first_frame", "iVBORw0KGgo=")], "/wand/pixverse/start-end-to-video", "first_frame_img"),
            (vec![image("reference", "iVBORw0KGgo="), image("reference", "iVBORw0KGgo=")], "/wand/pixverse/reference-to-video", "image_references"),
        ] {
            server.reset().await;
            Mock::given(method("POST")).and(path(at)).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ErrCode": 0, "Resp": { "video_id": 7 } }))).mount(&server).await;
            p.generate(&ref_schema(id), &base("go"), &e, &MaterializedRequest { refs, cleanup: Cleanup::empty() }).await.expect("submitted");
            let (sent_at, body) = posted(&server).await;
            assert_eq!(sent_at, at);
            assert!(body.get(check).is_some(), "{at}: {body}");
            if at != "/wand/pixverse/reference-to-video" {
                assert!(body.get("aspect_ratio").is_none(), "{at} follows its image: {body}");
            }
        }
        let (_, body) = posted(&server).await;
        assert_eq!(body["image_references"], json!([{ "img_id": "data:image/png;base64,iVBORw0KGgo=" }, { "img_id": "data:image/png;base64,iVBORw0KGgo=" }]));
    }

    /// Refused before any call: v5.6 at 10 s 1080p ("1080p 不支持 10"), frames with
    /// references, a last frame alone. A non-zero ErrCode is the vendor's refusal.
    #[tokio::test]
    async fn pixverse_refuses_what_it_cannot_make() {
        let quiet = MockServer::start().await;
        let p = pooled(&quiet.uri());
        let mut e = extras();
        e.duration_seconds = 10.0;
        e.resolution = Some("1080p".into());
        assert!(matches!(p.generate(&ref_schema("tokenhub/pixverse-video-v5.6"), &base("x"), &e, &no_refs()).await, Err(ProviderError::InvalidRequest(_))));
        for refs in [vec![image("first_frame", "iVBORw0KGgo="), image("reference", "iVBORw0KGgo=")], vec![image("last_frame", "iVBORw0KGgo=")]] {
            let m = MaterializedRequest { refs, cleanup: Cleanup::empty() };
            assert!(matches!(p.generate(&ref_schema("tokenhub/pixverse-video-c1"), &base("x"), &extras(), &m).await, Err(ProviderError::InvalidRequest(_))));
        }
        assert!(quiet.received_requests().await.unwrap().is_empty());
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/wand/pixverse/text-to-video")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ErrCode": 500054, "ErrMsg": "prompt sensitive" }))).mount(&server).await;
        let err = pooled(&server.uri()).generate(&ref_schema("tokenhub/pixverse-video-c1"), &base("x"), &extras(), &no_refs()).await.expect_err("refused");
        assert!(!err.is_retryable() && err.to_string().contains("prompt sensitive"), "{err}");
    }

    /// The poll: "Resp.status … 1 成功 / 5 生成中 / 6 已删除 / 7 审核失败 / 8 生成失败", Resp.url.
    #[tokio::test]
    async fn pixverse_polls_map() {
        let server = MockServer::start().await;
        let h = VideoGenerationHandle { provider_job_id: "4242".into(), provider: "tokenhub".into(), model: "tokenhub/pixverse-video-v6.0".into() };
        let url = "https://pixverse-out.example/v.mp4";
        for (status, want, why) in [(5, GenerationStatus::Processing, None), (1, GenerationStatus::Completed, None), (7, GenerationStatus::Failed, Some("moderation")), (8, GenerationStatus::Failed, Some("failed")), (6, GenerationStatus::Failed, Some("deleted")), (9, GenerationStatus::Failed, Some("9"))] {
            server.reset().await;
            Mock::given(method("GET")).and(path("/wand/pixverse/tasks/4242"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ErrCode": 0, "Resp": { "id": 4242, "status": status, "url": if status == 1 { url } else { "" } } })))
                .mount(&server).await;
            let r = pooled(&server.uri()).poll_status(&h).await.expect("polled");
            assert_eq!(r.status, want, "status {status}");
            if want == GenerationStatus::Completed {
                assert_eq!(r.video_url.as_deref(), Some(url));
            }
            if let Some(w) = why {
                assert!(r.error.as_deref().is_some_and(|e| e.contains(w)), "status {status}: {:?}", r.error);
            }
        }
    }

    async fn vidu_accepting() -> MockServer {
        let server = MockServer::start().await;
        for kind in ["text-to-video", "image-to-video", "start-end-to-video", "reference-to-video"] {
            Mock::given(method("POST")).and(path(format!("/wand/vidu/{kind}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "vd1", "state": "created" })))
                .mount(&server).await;
        }
        server
    }

    /// Vidu resold on TokenHub: the endpoint follows the images (as
    /// video/vidu.rs picks it) — none → text-to-video, a first frame →
    /// image-to-video, first and last → start-end-to-video (start first), references
    /// → reference-to-video — with Vidu's own fields; `aspect_ratio` on text and
    /// references only; the row's 720p when none is asked.
    /// @see <https://cloud.tencent.com/document/product/1823/135756> (2026-11-09)
    #[tokio::test]
    async fn vidu_picks_its_endpoint_from_the_images() {
        let id = "tokenhub/vidu-video-q3-turbo";
        let server = vidu_accepting().await;
        let p = pooled(&server.uri());
        let mut e = extras();
        e.aspect_ratio = Some("9:16".into());
        e.audio = Some(false);
        let mut b = base("a paper boat on a stream");
        b.seed = Some(11);
        let h = p.generate(&ref_schema(id), &b, &e, &no_refs()).await.expect("submitted");
        assert_eq!(h.provider_job_id, "vd1");
        let (at, body) = posted(&server).await;
        assert_eq!(at, "/wand/vidu/text-to-video");
        assert_eq!(body, json!({ "model": "vidu-video-q3-turbo", "prompt": "a paper boat on a stream", "duration": 5, "resolution": "720p", "aspect_ratio": "9:16", "seed": 11, "audio": false }));
        for (refs, at) in [
            (vec![image("first_frame", "iVBORw0KGgo=")], "/wand/vidu/image-to-video"),
            (vec![image("last_frame", "/9j/4AAQ"), image("first_frame", "iVBORw0KGgo=")], "/wand/vidu/start-end-to-video"),
            (vec![image("reference", "iVBORw0KGgo="), image("reference", "/9j/4AAQ")], "/wand/vidu/reference-to-video"),
        ] {
            server.reset().await;
            Mock::given(method("POST")).and(path(at)).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "vd2", "state": "created" }))).mount(&server).await;
            let mut e = extras();
            e.aspect_ratio = Some("1:1".into());
            p.generate(&ref_schema(id), &base("go"), &e, &MaterializedRequest { refs, cleanup: Cleanup::empty() }).await.expect("submitted");
            let (sent_at, body) = posted(&server).await;
            assert_eq!(sent_at, at);
            match at {
                "/wand/vidu/start-end-to-video" => assert_eq!(body["images"], json!(["data:image/png;base64,iVBORw0KGgo=", "data:image/jpeg;base64,/9j/4AAQ"]), "the start frame first"),
                "/wand/vidu/reference-to-video" => assert_eq!(body["aspect_ratio"], "1:1"),
                _ => assert!(body.get("aspect_ratio").is_none(), "{at}: {body}"),
            }
        }
    }

    /// Refused before any call: Q4 Preview with no image ("文生视频 | 不支持"), sound on
    /// references for any model but q3 ("非主体调用时仅 q3 支持"), a reference prompt over
    /// 2000 characters ("直接参考方式 ≤ 2000 字符"), frames with references, a last frame alone.
    #[tokio::test]
    async fn vidu_refuses_what_its_endpoint_cannot_take() {
        let quiet = MockServer::start().await;
        let p = pooled(&quiet.uri());
        let refs = || MaterializedRequest { refs: vec![image("reference", "iVBORw0KGgo=")], cleanup: Cleanup::empty() };
        let mut sound = extras();
        sound.audio = Some(true);
        let cases: Vec<(&str, BaseGenerationRequest, VideoExtras, MaterializedRequest)> = vec![
            ("tokenhub/vidu-video-q4-preview", base("x"), extras(), no_refs()),
            ("tokenhub/vidu-video-q4-preview", base("x"), sound, refs()),
            ("tokenhub/vidu-video-q3-turbo", base(&"y".repeat(2001)), extras(), refs()),
            ("tokenhub/vidu-video-q3-turbo", base("x"), extras(), MaterializedRequest { refs: vec![image("first_frame", "iVBORw0KGgo="), image("reference", "iVBORw0KGgo=")], cleanup: Cleanup::empty() }),
            ("tokenhub/vidu-video-q3-pro", base("x"), extras(), MaterializedRequest { refs: vec![image("last_frame", "iVBORw0KGgo=")], cleanup: Cleanup::empty() }),
        ];
        for (id, b, e, m) in cases {
            let err = p.generate(&ref_schema(id), &b, &e, &m).await.expect_err("refused");
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{id}: {err:?}");
        }
        assert!(quiet.received_requests().await.unwrap().is_empty());
        // q3 has sound on references.
        let server = vidu_accepting().await;
        let mut e = extras();
        e.audio = Some(true);
        pooled(&server.uri()).generate(&ref_schema("tokenhub/vidu-video-q3"), &base("x"), &e, &refs()).await.expect("q3 takes audio on references");
    }

    /// Q2's start-end endpoint takes "1 ~ 8" seconds and no audio ("仅 q3 系列支持"):
    /// both refused before any call.
    #[tokio::test]
    async fn vidu_q2_start_end_limits_are_refused_before_any_call() {
        let quiet = MockServer::start().await;
        let p = pooled(&quiet.uri());
        let both = || MaterializedRequest { refs: vec![image("first_frame", "iVBORw0KGgo="), image("last_frame", "iVBORw0KGgo=")], cleanup: Cleanup::empty() };
        let mut long = extras();
        long.duration_seconds = 9.0;
        let mut sound = extras();
        sound.audio = Some(true);
        for (id, e) in [("tokenhub/vidu-video-q2-turbo", long), ("tokenhub/vidu-video-q2-pro", sound)] {
            let err = p.generate(&ref_schema(id), &base("x"), &e, &both()).await.expect_err("refused");
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{id}: {err:?}");
        }
        assert!(quiet.received_requests().await.unwrap().is_empty());
    }

    /// "state … created / queueing / processing / success / failed", creations[0].url.
    #[tokio::test]
    async fn vidu_polls_map() {
        let server = MockServer::start().await;
        let h = VideoGenerationHandle { provider_job_id: "vd1".into(), provider: "tokenhub".into(), model: "tokenhub/vidu-video-q3-pro".into() };
        let url = "https://vidu-out.example/v.mp4";
        for (answer, want) in [
            (json!({ "task_id": "vd1", "state": "queueing" }), GenerationStatus::Processing),
            (json!({ "task_id": "vd1", "state": "success", "creations": [{ "url": url, "cover_url": "c" }] }), GenerationStatus::Completed),
            (json!({ "task_id": "vd1", "state": "failed", "err_code": "AuditSubmitIllegal" }), GenerationStatus::Failed),
            (json!({ "task_id": "vd1", "state": "paused" }), GenerationStatus::Failed),
        ] {
            server.reset().await;
            Mock::given(method("GET")).and(path("/wand/vidu/tasks/vd1")).respond_with(ResponseTemplate::new(200).set_body_json(answer.clone())).mount(&server).await;
            let r = pooled(&server.uri()).poll_status(&h).await.expect("polled");
            assert_eq!(r.status, want, "{answer}");
            if want == GenerationStatus::Completed {
                assert_eq!(r.video_url.as_deref(), Some(url));
            }
            if answer["state"] == "failed" {
                assert!(r.error.as_deref().is_some_and(|e| e.contains("AuditSubmitIllegal")), "{:?}", r.error);
            }
        }
    }

    /// YouTu on TokenHub's OpenAI-style video API: POST /api/video/submit with
    /// the old action's fields in snake_case (image {base64}, prompt,
    /// resolution, fps), `id` as the handle; POST /api/video/query {model, id}
    /// answers {status, progress, data: {url}}.
    /// @see <https://cloud.tencent.com/document/product/1823/135716> (2026-11-09)
    #[tokio::test]
    async fn yt_video_submits_and_queries_by_post() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/api/video/submit"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "yt1", "request_id": "r", "object": "video", "created_at": 1, "status": "queued" })))
            .mount(&server).await;
        let id = "tokenhub/yt-video-2.0";
        let mut e = extras();
        e.resolution = Some("1080p".into());
        e.fps = Some(24);
        let h = pooled(&server.uri()).generate(&ref_schema(id), &base("the leaves rustle"), &e, &frame()).await.expect("submitted");
        assert_eq!(h.provider_job_id, "yt1");
        let (at, body) = posted(&server).await;
        assert_eq!(at, "/api/video/submit");
        assert_eq!(body, json!({ "model": "yt-video-2.0", "image": { "base64": "iVBORw0KGgoAAAANSUhEUg==" }, "prompt": "the leaves rustle", "resolution": "1080p", "fps": 24 }));
        let url = "https://yt-out.example/v.mp4";
        for (answer, want) in [
            (json!({ "object": "video", "status": "queued" }), GenerationStatus::Pending),
            (json!({ "object": "video", "status": "in_progress", "progress": 40 }), GenerationStatus::Processing),
            (json!({ "object": "video", "status": "completed", "progress": 100, "data": { "url": url } }), GenerationStatus::Completed),
            (json!({ "object": "video", "status": "failed", "error": { "code": "InternalError", "message": "generate failed" } }), GenerationStatus::Failed),
            (json!({ "object": "video", "status": "paused" }), GenerationStatus::Failed),
        ] {
            server.reset().await;
            Mock::given(method("POST")).and(path("/api/video/query")).respond_with(ResponseTemplate::new(200).set_body_json(answer.clone())).mount(&server).await;
            let r = pooled(&server.uri()).poll_status(&h).await.expect("polled");
            assert_eq!(r.status, want, "{answer}");
            if want == GenerationStatus::Completed {
                assert_eq!(r.video_url.as_deref(), Some(url));
            }
            let received = server.received_requests().await.unwrap();
            let q: Value = serde_json::from_slice(&received[0].body).unwrap();
            assert_eq!(q, json!({ "model": "yt-video-2.0", "id": "yt1" }));
        }
    }

    /// Kling Omni goes to /wand/kling/omni-video for text and frames alike:
    /// `contents` (the prompt, then the frames) and `settings`; O1 with only a
    /// first frame takes 5 or 10 s ("O1 仅用首帧且无其他参考时仅支持 5 / 10").
    /// @see <https://cloud.tencent.com/document/product/1823/135742> (2026-11-09)
    #[tokio::test]
    async fn kling_omni_sends_contents_to_its_own_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/wand/kling/omni-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": { "id": "ko1", "status": "submitted" } })))
            .mount(&server).await;
        let p = pooled(&server.uri());
        let mut e = extras();
        e.aspect_ratio = Some("9:16".into());
        e.audio = Some(true);
        let h = p.generate(&ref_schema("tokenhub/kling-video-v3-omni"), &base("a red fox"), &e, &no_refs()).await.expect("submitted");
        assert_eq!(h.provider_job_id, "ko1");
        let (at, body) = posted(&server).await;
        assert_eq!(at, "/wand/kling/omni-video");
        assert_eq!(
            body,
            json!({
                "model": "kling-video-v3-omni",
                "contents": [{ "type": "prompt", "text": "a red fox" }],
                // The row's resolution when none is asked (E10).
                "settings": { "duration": 5, "aspect_ratio": "9:16", "resolution": "720p", "audio": "native" }
            })
        );
        let quiet = MockServer::start().await;
        let mut seven = extras();
        seven.duration_seconds = 7.0;
        let first = MaterializedRequest { refs: vec![image("first_frame", "iVBORw0KGgo=")], cleanup: Cleanup::empty() };
        assert!(matches!(pooled(&quiet.uri()).generate(&ref_schema("tokenhub/kling-video-o1"), &base("x"), &seven, &first).await, Err(ProviderError::InvalidRequest(_))));
        assert!(quiet.received_requests().await.unwrap().is_empty());
    }

    /// Every TokenHub video row resolves to an endpoint.
    #[test]
    fn every_tokenhub_video_row_has_an_endpoint() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let reg = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        let rows: Vec<_> = reg.for_provider("tokenhub").filter(|m| m.media_type == crate::capabilities::MediaType::Video).collect();
        assert!(!rows.is_empty());
        for m in rows {
            assert!(route(&m.id).is_some(), "{} has no endpoint", m.id);
        }
        assert!(route("tokenhub/hy-video-1.5").is_none(), "the model list's spelling is not a row");
    }
}
