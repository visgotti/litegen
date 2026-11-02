//! `wait_for_generation`: an agent cannot sleep between polls, so this tool
//! polls a job's own status route through the API router (the caller's own
//! permissions, as for every tool) until the job ends or the timeout passes.
//! It returns well inside client limits (45 s by default, 230 s at most) and
//! reports progress when the client asked for it.

use std::time::{Duration, Instant};

use rmcp::model::{CallToolResult, ContentBlock, JsonObject, ProgressNotificationParam, ProgressToken};
use rmcp::{Peer, RoleServer};
use serde_json::Value;
use tokio::sync::Mutex;

use super::auth::Caller;
use super::registry::{ToolSpec, WAIT_DEFAULT_SECS, WAIT_MAX_SECS};
use super::{content, dispatch, McpShared};

/// How often a job's status is read.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The status route for a job id: videos and 3D jobs have their own (open to
/// a `generate` key), anything else the generations route.
pub fn status_path(id: &str) -> String {
    let encoded = urlencoding::encode(id);
    if id.starts_with("litegen-vid-") {
        format!("/v1/videos/{encoded}")
    } else if id.starts_with("litegen-3d-") {
        format!("/v1/models3d/{encoded}")
    } else {
        format!("/v1/generations/{encoded}")
    }
}

/// Progress notifications for one request. MCP requires the value to rise
/// every time, so a repeat is bumped above the last one sent.
pub struct ProgressSink {
    token: ProgressToken,
    peer: Peer<RoleServer>,
    last: Mutex<f64>,
}

impl ProgressSink {
    pub fn new(token: ProgressToken, peer: Peer<RoleServer>) -> Self {
        ProgressSink { token, peer, last: Mutex::new(f64::MIN) }
    }

    pub async fn report(&self, progress: f64, message: String) {
        let mut last = self.last.lock().await;
        let value = if progress > *last { progress } else { *last + 1.0 };
        *last = value;
        let param = ProgressNotificationParam::new(self.token.clone(), value).with_message(message);
        if let Err(e) = self.peer.notify_progress(param).await {
            tracing::debug!(error = %e, "progress notification not delivered");
        }
    }
}

pub async fn run(shared: &McpShared, caller: &Caller, spec: &ToolSpec, args: &JsonObject, progress: Option<&ProgressSink>) -> CallToolResult {
    let Some(id) = args.get("id").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) else {
        return CallToolResult::error(vec![ContentBlock::text("missing required argument 'id'")]);
    };
    let timeout = args.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(WAIT_DEFAULT_SECS).clamp(1, WAIT_MAX_SECS);
    let deadline = Duration::from_secs(timeout);
    let started = Instant::now();
    let path = status_path(id.trim());

    loop {
        let reply = dispatch::get(shared, caller, &path).await;
        if !reply.status.is_success() {
            return content::error_result(&reply, serde_json::from_slice(&reply.body).ok());
        }
        let state: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
        let status = state["status"].as_str().unwrap_or("unknown").to_string();
        let percent = state["progress"].as_f64().unwrap_or(0.0);

        if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
            return shaped(spec, &reply);
        }
        if let Some(sink) = progress {
            sink.report(percent, format!("{status} ({percent}%)")).await;
        }
        if started.elapsed() + POLL_INTERVAL > deadline {
            let mut result = shaped(spec, &reply);
            result.content.push(ContentBlock::text(format!(
                "Still {status} ({percent}%). Call wait_for_generation again with the same id."
            )));
            return result;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The job's state, shaped like its status tool's result, files linked.
fn shaped(spec: &ToolSpec, reply: &dispatch::HttpReply) -> CallToolResult {
    let mut result = content::json_result(spec, reply);
    if let Some(value) = result.structured_content.clone() {
        result.content.extend(content::links(spec.def.name, &value));
    }
    result
}
