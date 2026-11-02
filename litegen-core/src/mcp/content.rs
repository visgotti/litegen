//! Turning an API reply into an MCP tool result. Successes carry the JSON as
//! `structuredContent` and as text (for clients that read only text); errors
//! carry the API's own status, code and message as a tool error, so the model
//! can read it and correct the call.

use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{json, Value};

use super::dispatch::HttpReply;
use super::registry::ToolSpec;

/// The most text one result may carry: under claude.ai's ~150,000-character
/// cap and near Claude Code's 25,000-token one.
pub const MAX_TEXT_CHARS: usize = 100_000;

pub fn json_result(spec: &ToolSpec, reply: &HttpReply) -> CallToolResult {
    let parsed: Option<Value> = serde_json::from_slice(&reply.body).ok();
    if reply.status.is_success() {
        let value = match parsed {
            Some(Value::Object(map)) => Value::Object(map),
            Some(Value::Array(items)) => json!({ "items": items }),
            Some(other) => json!({ "value": other }),
            None => json!({ "value": String::from_utf8_lossy(&reply.body) }),
        };
        let value = fit(spec.def.name, value, MAX_TEXT_CHARS);
        let mut content = vec![ContentBlock::text(value.to_string())];
        content.extend(notes(reply).into_iter().map(ContentBlock::text));
        let mut result = CallToolResult::success(content);
        result.structured_content = Some(value);
        result
    } else {
        error_result(reply, parsed)
    }
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
    object
        .iter()
        .filter(|(_, v)| v.is_array())
        .max_by_key(|(_, v)| chars(v))
        .map(|(k, _)| k.clone())
}
