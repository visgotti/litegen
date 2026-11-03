//! Turning an API reply into an MCP tool result. Successes carry the JSON as
//! `structuredContent` and as text (for clients that read only text); errors
//! carry the API's own status, code and message as a tool error, so the model
//! can read it and correct the call. Generated images also come back as small
//! previews, and files as `resource_link`s or embedded blobs.

use base64::Engine;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Resource, ResourceContents};
use serde_json::{json, Value};

use super::auth::Caller;
use super::dispatch::{self, HttpReply};
use super::registry::ToolSpec;
use super::McpShared;

/// The most text one result may carry: under claude.ai's ~150,000-character
/// cap and near Claude Code's 25,000-token one.
pub const MAX_TEXT_CHARS: usize = 100_000;
/// Previews per call, their longest side, and their JPEG quality. Claude Code
/// counts images against its output cap, so they stay small.
const MAX_PREVIEWS: usize = 2;
const PREVIEW_EDGE: u32 = 512;
const PREVIEW_QUALITY: u8 = 80;
/// Largest image fetched for a preview.
const MAX_PREVIEW_SOURCE_BYTES: usize = 16 * 1024 * 1024;
/// Largest file embedded in a result; above it, a link (or an error).
const MAX_EMBEDDED_BYTES: usize = 8 * 1024 * 1024;
/// Largest image asset returned as a preview by get_generation_asset.
const MAX_PREVIEWED_ASSET_BYTES: usize = 4 * 1024 * 1024;
/// A `b64_json` longer than this is replaced in text and structured copies.
const MAX_INLINE_BASE64: usize = 1_024;

/// Shape a tool's reply: JSON with links and previews, a file, or an error.
pub async fn to_result(shared: &McpShared, caller: &Caller, spec: &ToolSpec, args: &JsonObject, reply: &HttpReply) -> CallToolResult {
    if !reply.status.is_success() {
        return error_result(reply, serde_json::from_slice(&reply.body).ok());
    }
    let name = spec.def.name;
    let is_json = content_type(reply).is_none_or(|ct| ct.contains("json")) && serde_json::from_slice::<Value>(&reply.body).is_ok();
    if !is_json {
        return file_result(shared, caller, spec, args, reply).await;
    }
    let parsed: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    let sources = if wants_previews(spec, args) { image_sources(name, &parsed) } else { Vec::new() };
    let mut result = json_result(spec, reply);
    if let Some(value) = &result.structured_content {
        result.content.extend(links(name, value));
    }
    result.content.extend(previews(shared, caller, sources).await);
    result
}

/// A 2xx JSON reply as a result: base64 image data replaced by a pointer,
/// then fitted under the size cap, as `structuredContent` and as text.
pub fn json_result(spec: &ToolSpec, reply: &HttpReply) -> CallToolResult {
    let value = match serde_json::from_slice::<Value>(&reply.body).ok() {
        Some(Value::Object(map)) => Value::Object(map),
        Some(Value::Array(items)) => json!({ "items": items }),
        Some(other) => json!({ "value": other }),
        None => json!({ "value": String::from_utf8_lossy(&reply.body) }),
    };
    let mut value = value;
    redact_base64(&mut value);
    let value = fit(spec.def.name, value, MAX_TEXT_CHARS);
    let mut content = vec![ContentBlock::text(value.to_string())];
    content.extend(notes(reply).into_iter().map(ContentBlock::text));
    let mut result = CallToolResult::success(content);
    result.structured_content = Some(value);
    result
}

/// A non-2xx reply as a tool error: "HTTP 402 budget_exceeded: …".
pub fn error_result(reply: &HttpReply, parsed: Option<Value>) -> CallToolResult {
    let error = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .cloned()
        .unwrap_or_else(|| json!({ "message": String::from_utf8_lossy(&reply.body).trim().to_string() }));
    let code = error
        .get("code")
        .or_else(|| error.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| reply.status.canonical_reason().unwrap_or("error"));
    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
    let text = format!("HTTP {} {}: {}", reply.status.as_u16(), code, message);
    let mut result = CallToolResult::error(vec![ContentBlock::text(text)]);
    result.structured_content = Some(json!({ "status": reply.status.as_u16(), "error": error }));
    result
}

/// What the API said in headers rather than the body.
fn notes(reply: &HttpReply) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(dropped) = reply.headers.get("x-litegen-dropped-params").and_then(|v| v.to_str().ok()) {
        let list = dropped.split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
        if !list.is_empty() {
            notes.push(format!("Note: LiteGen ignored these parameters for this model (strict: false): {list}"));
        }
    }
    if reply.headers.get("idempotent-replayed").and_then(|v| v.to_str().ok()) == Some("true") {
        notes.push("Note: replayed from an earlier call with the same idempotency_key; nothing new was billed.".to_string());
    }
    notes
}

fn content_type(reply: &HttpReply) -> Option<String> {
    reply.headers.get(axum::http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|s| s.to_ascii_lowercase())
}

// ─── Size ─────────────────────────────────────────────────────────────────────

const MODEL_SUMMARY_FIELDS: [&str; 7] = ["id", "name", "provider", "media_type", "status", "is_available", "tags"];

fn chars(value: &Value) -> usize {
    value.to_string().len()
}

/// Keep a result under `max_chars` of JSON. `list_models` first drops each
/// model to its summary fields; then the result's main list is halved until
/// it fits, and `_truncated` says how many entries are shown and how to ask
/// for fewer.
pub fn fit(tool: &str, mut value: Value, max_chars: usize) -> Value {
    if chars(&value) <= max_chars {
        return value;
    }
    if tool == "list_models" {
        if let Some(models) = value.get_mut("data").and_then(Value::as_array_mut) {
            for model in models.iter_mut() {
                if let Value::Object(fields) = model {
                    fields.retain(|k, _| MODEL_SUMMARY_FIELDS.contains(&k.as_str()));
                }
            }
            value["_compact"] = json!("Each model is shown by its summary fields; call get_model for a model's full schema.");
        }
        if chars(&value) <= max_chars {
            return value;
        }
    }

    let Some(key) = main_list(&value) else { return value };
    let total = value[key.as_str()].as_array().map(Vec::len).unwrap_or(0);
    let mut shown = total;
    while chars(&value) > max_chars && shown > 0 {
        shown /= 2;
        if let Some(list) = value.get_mut(key.as_str()).and_then(Value::as_array_mut) {
            list.truncate(shown);
        }
        value["_truncated"] = json!({
            "shown": shown,
            "total": total,
            "hint": "Fewer results: use the tool's page/per_page or filter arguments.",
        });
    }
    value
}

/// `data` or `items` when present, else the largest top-level array.
fn main_list(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    for key in ["data", "items"] {
        if object.get(key).is_some_and(Value::is_array) {
            return Some(key.to_string());
        }
    }
    object.iter().filter(|(_, v)| v.is_array()).max_by_key(|(_, v)| chars(v)).map(|(k, _)| k.clone())
}

/// Replace long `b64_json` strings: the bytes reach the model as a preview,
/// and stay fetchable with get_generation_asset.
fn redact_base64(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                match v {
                    Value::String(s) if key == "b64_json" && s.len() > MAX_INLINE_BASE64 => {
                        let bytes = s.len() / 4 * 3;
                        *v = json!(format!("<{bytes} bytes of base64 omitted; fetch it with get_generation_asset(id, index)>"));
                    }
                    other => redact_base64(other),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_base64),
        _ => {}
    }
}

// ─── Links ────────────────────────────────────────────────────────────────────

/// `resource_link`s to the files a result names.
pub fn links(tool: &str, value: &Value) -> Vec<ContentBlock> {
    let mut out = Vec::new();
    let link = |url: &str, name: String, mime: &str| ContentBlock::resource_link(Resource::new(url, name).with_mime_type(mime));
    match tool {
        "generate_image" => {
            for (i, item) in value["data"].as_array().into_iter().flatten().enumerate() {
                if let Some(url) = item["url"].as_str() {
                    out.push(link(url, format!("image-{}", i + 1), &mime_for(extension(url), "image")));
                }
            }
        }
        "get_video_status" | "get_3d_model_status" | "wait_for_generation" | "get_generation" => {
            // A status route names `video_url` and `assets`; a stored
            // generation record, `metadata.assets` and `result_url`.
            if let Some(url) = value["video_url"].as_str() {
                out.push(link(url, "video".into(), &mime_for(extension(url), "video")));
            }
            let assets = value["assets"].as_array().or_else(|| value["metadata"]["assets"].as_array());
            for asset in assets.into_iter().flatten() {
                out.extend(asset_link(asset));
            }
            if out.is_empty() {
                if let Some(url) = value["result_url"].as_str() {
                    let kind = value["media_type"].as_str().unwrap_or("file");
                    out.push(link(url, kind.to_string(), &mime_for(extension(url), kind)));
                }
            }
        }
        "list_generation_assets" => {
            for asset in value["data"].as_array().into_iter().flatten() {
                out.extend(asset_link(asset));
            }
        }
        _ => {}
    }
    out
}

fn asset_link(asset: &Value) -> Option<ContentBlock> {
    let url = asset["url"].as_str()?;
    let format = asset["format"].as_str().unwrap_or_else(|| extension(url));
    let kind = asset["kind"].as_str().unwrap_or("file");
    let mut resource = Resource::new(url, format!("{kind}.{format}")).with_mime_type(mime_for(format, kind));
    if let Some(size) = asset["size_bytes"].as_u64() {
        resource = resource.with_size(size);
    }
    Some(ContentBlock::resource_link(resource))
}

fn extension(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit_once('.').map(|(_, ext)| ext).filter(|e| !e.contains('/')).unwrap_or("")
}

/// The media type of a file format (`glb`, `png`, `mp4`, …).
pub fn mime_for(format: &str, kind: &str) -> String {
    let mime = match format.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "obj" => "model/obj",
        "stl" => "model/stl",
        "usdz" => "model/vnd.usdz+zip",
        "3mf" => "model/3mf",
        "dae" => "model/vnd.collada+xml",
        _ => match kind {
            "image" => "image/png",
            "video" => "video/mp4",
            _ => "application/octet-stream",
        },
    };
    mime.to_string()
}

// ─── Previews ─────────────────────────────────────────────────────────────────

/// Where a preview's image comes from.
#[derive(Debug, Clone)]
pub enum ImageSource {
    Url(String),
    Base64(String),
}

fn wants_previews(spec: &ToolSpec, args: &JsonObject) -> bool {
    spec.def.name == "generate_image" && args.get("preview").and_then(Value::as_bool) != Some(false)
}

fn image_sources(tool: &str, value: &Value) -> Vec<ImageSource> {
    if tool != "generate_image" {
        return Vec::new();
    }
    value["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item["b64_json"]
                .as_str()
                .map(|b| ImageSource::Base64(b.to_string()))
                .or_else(|| item["url"].as_str().map(|u| ImageSource::Url(u.to_string())))
        })
        .take(MAX_PREVIEWS)
        .collect()
}

/// Small JPEG previews. A source that cannot be fetched or decoded is skipped:
/// a preview never fails a call.
pub async fn previews(shared: &McpShared, caller: &Caller, sources: Vec<ImageSource>) -> Vec<ContentBlock> {
    let mut out = Vec::new();
    for source in sources.into_iter().take(MAX_PREVIEWS) {
        let bytes = match source {
            ImageSource::Base64(data) => {
                let data = data.split_once(";base64,").map(|(_, d)| d).unwrap_or(&data).to_string();
                base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()
            }
            ImageSource::Url(url) => fetch_image(shared, caller, &url).await,
        };
        let Some(bytes) = bytes else { continue };
        if let Ok(Some(jpeg)) = tokio::task::spawn_blocking(move || preview_jpeg(&bytes)).await {
            out.push(ContentBlock::image(base64::engine::general_purpose::STANDARD.encode(jpeg), "image/jpeg"));
        }
    }
    out
}

/// A URL on LiteGen itself is fetched through the API router as the caller;
/// any other public http(s) URL with the shared client.
async fn fetch_image(shared: &McpShared, caller: &Caller, url: &str) -> Option<Vec<u8>> {
    let base = shared.state.router.config.server.public_base_url();
    if let Some(path) = url.strip_prefix(&base).filter(|p| p.starts_with('/')) {
        let reply = dispatch::get(shared, caller, path).await;
        return (reply.status.is_success() && reply.body.len() <= MAX_PREVIEW_SOURCE_BYTES).then(|| reply.body.to_vec());
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    crate::util::ssrf::validate_public_url(url).await.ok()?;
    let resp = shared.http.get(url).send().await.ok()?;
    if !resp.status().is_success() || resp.content_length().is_some_and(|n| n as usize > MAX_PREVIEW_SOURCE_BYTES) {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    (bytes.len() <= MAX_PREVIEW_SOURCE_BYTES).then(|| bytes.to_vec())
}

fn preview_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = if image.width().max(image.height()) > PREVIEW_EDGE {
        image.resize(PREVIEW_EDGE, PREVIEW_EDGE, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, PREVIEW_QUALITY);
    image.to_rgb8().write_with_encoder(encoder).ok()?;
    Some(out)
}

// ─── Files ────────────────────────────────────────────────────────────────────

/// A reply that is a file rather than JSON.
async fn file_result(shared: &McpShared, caller: &Caller, spec: &ToolSpec, args: &JsonObject, reply: &HttpReply) -> CallToolResult {
    let size = reply.body.len();
    let mime = content_type(reply).map(|ct| ct.split(';').next().unwrap_or("").trim().to_string()).unwrap_or_default();
    let arg = |name: &str| args.get(name).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();

    match spec.def.name {
        "convert_3d_model" => {
            let format = arg("to");
            if size > MAX_EMBEDDED_BYTES {
                let base = shared.state.router.config.server.public_base_url();
                return CallToolResult::error(vec![ContentBlock::text(format!(
                    "The converted file is {} MiB, over MCP's 8 MiB limit; call POST {base}/v1/utils/models3d/convert directly.",
                    size.div_ceil(1024 * 1024)
                ))]);
            }
            let uri = format!("litegen://convert/{}.{format}", uuid::Uuid::new_v4());
            embedded(&uri, &mime_for(&format, "mesh"), &reply.body, format!("Converted to {format}: {size} bytes."))
        }
        "get_generation_asset" => {
            let (id, index) = (arg("id"), arg("index"));
            if mime.starts_with("image/") && size <= MAX_PREVIEWED_ASSET_BYTES && args.get("preview").and_then(Value::as_bool) != Some(false) {
                let preview = previews(shared, caller, vec![ImageSource::Base64(base64::engine::general_purpose::STANDARD.encode(&reply.body))]).await;
                if !preview.is_empty() {
                    let mut content = vec![ContentBlock::text(format!("Asset {index} of {id}: {mime}, {size} bytes (shown as a preview)."))];
                    content.extend(preview);
                    return CallToolResult::success(content);
                }
            }
            if size <= MAX_EMBEDDED_BYTES {
                let uri = format!("litegen://generations/{id}/assets/{index}");
                return embedded(&uri, &mime, &reply.body, format!("Asset {index} of {id}: {mime}, {size} bytes."));
            }
            // Too big to embed: point at the asset's own URL.
            let list = dispatch::get(shared, caller, &format!("/v1/generations/{}/assets", urlencoding::encode(&id))).await;
            let listed: Value = serde_json::from_slice(&list.body).unwrap_or(Value::Null);
            let wanted: Option<u64> = index.parse().ok();
            let asset = listed["data"].as_array().into_iter().flatten().find(|a| wanted.is_some() && a["index"].as_u64() == wanted);
            let mut content = vec![ContentBlock::text(format!("Asset {index} of {id} is {} MiB, too large to embed; download it from its URL.", size.div_ceil(1024 * 1024)))];
            content.extend(asset.and_then(asset_link));
            CallToolResult::success(content)
        }
        _ => {
            if size <= MAX_EMBEDDED_BYTES {
                let uri = format!("litegen://{}/{}", spec.def.name, uuid::Uuid::new_v4());
                embedded(&uri, if mime.is_empty() { "application/octet-stream" } else { &mime }, &reply.body, format!("{mime}, {size} bytes."))
            } else {
                CallToolResult::error(vec![ContentBlock::text(format!("The reply is {size} bytes of {mime}, over MCP's 8 MiB limit; call the HTTP API directly."))])
            }
        }
    }
}

fn embedded(uri: &str, mime: &str, bytes: &[u8], summary: String) -> CallToolResult {
    let blob = ResourceContents::blob(base64::engine::general_purpose::STANDARD.encode(bytes), uri).with_mime_type(mime);
    CallToolResult::success(vec![ContentBlock::text(summary), ContentBlock::resource(blob)])
}
