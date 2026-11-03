//! `wait_for_generation`: an agent cannot sleep between polls, so this tool
//! reads a job's state through the API router (the caller's own permissions,
//! as for every tool) until the job ends or the timeout passes. It returns
//! well inside client limits (45 s by default, 230 s at most) and reports
//! progress when the client asked for it.

use std::time::{Duration, Instant};

use axum::http::{header, StatusCode};
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, ProgressNotificationParam, ProgressToken};
use rmcp::{Peer, RoleServer};
use serde_json::Value;
use tokio::sync::Mutex;

use super::auth::Caller;
use super::dispatch::HttpReply;
use super::registry::{ToolSpec, WAIT_DEFAULT_SECS, WAIT_MAX_SECS};
use super::{content, dispatch, McpShared};
use crate::api::middleware::Scope;

/// The first pause between reads of a job's state, and the longest.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
pub const MAX_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// The pause after the `reads`-th read: two seconds, then half again after
/// each read up to ten, so a quick job is seen at once and a long one is not
/// read every two seconds. Never shorter than the reply asks: a 429's or 503's
/// `Retry-After`, and for a key with a rate limit (its replies carry
/// `X-RateLimit-Limit`), long enough that waiting spends at most half of its
/// requests per minute and leaves the rest to the agent's other calls.
pub fn next_delay(reads: u32, reply: &HttpReply) -> Duration {
    let schedule = (1..reads).fold(POLL_INTERVAL, |pause, _| (pause * 3 / 2).min(MAX_POLL_INTERVAL));
    let retry_after = header_number(reply, header::RETRY_AFTER.as_str()).map(Duration::from_secs);
    let fair_share = header_number(reply, "x-ratelimit-limit").filter(|limit| *limit > 0).map(|limit| Duration::from_secs_f64(120.0 / limit as f64));
    [Some(schedule), retry_after, fair_share].into_iter().flatten().max().unwrap_or(schedule)
}

/// A header holding a whole number of seconds or requests. `Retry-After` may
/// also be a date; LiteGen never sends one, and a date is ignored.
fn header_number(reply: &HttpReply, name: &str) -> Option<u64> {
    reply.headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// A reply that says to come back later rather than that the read failed.
fn busy(status: StatusCode) -> bool {
    matches!(status, StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE)
}

/// Where `caller` reads a job's state. A video or 3D job's own status route
/// moves the job along (it polls the vendor and keeps the finished files), so
/// the API opens it to `generate` only; any other caller reads the stored
/// record, which LiteGen's background poller keeps current.
pub fn status_path(id: &str, caller: &Caller) -> String {
    let encoded = urlencoding::encode(id);
    let own_route = caller.ctx.scopes.contains(&Scope::Generate);
    if own_route && id.starts_with("litegen-vid-") {
        format!("/v1/videos/{encoded}")
    } else if own_route && id.starts_with("litegen-3d-") {
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
    let path = status_path(id.trim(), caller);

    // The last state read: what a busy reply with no time left to sit out returns.
    let mut latest: Option<HttpReply> = None;
    let mut reads = 0;
    loop {
        let reply = dispatch::get(shared, caller, &path).await;
        reads += 1;
        let delay = next_delay(reads, &reply);
        let out_of_time = started.elapsed().saturating_add(delay) > deadline;

        if reply.status.is_success() {
            let (status, percent) = state_of(&reply);
            if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
                return shaped(spec, &reply);
            }
            if let Some(sink) = progress {
                sink.report(percent, format!("{status} ({percent}%)")).await;
            }
            if out_of_time {
                return still_running(spec, &reply);
            }
            latest = Some(reply);
        } else if !busy(reply.status) {
            return content::error_result(&reply, serde_json::from_slice(&reply.body).ok());
        } else if out_of_time {
            return match &latest {
                Some(state) => still_running(spec, state),
                None => content::error_result(&reply, serde_json::from_slice(&reply.body).ok()),
            };
        }
        tokio::time::sleep(delay).await;
    }
}

/// A job's status and percent done.
fn state_of(reply: &HttpReply) -> (String, f64) {
    let state: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    (state["status"].as_str().unwrap_or("unknown").to_string(), state["progress"].as_f64().unwrap_or(0.0))
}

/// The job's state, with a note to call again.
fn still_running(spec: &ToolSpec, reply: &HttpReply) -> CallToolResult {
    let (status, percent) = state_of(reply);
    let mut result = shaped(spec, reply);
    result.content.push(ContentBlock::text(format!("Still {status} ({percent}%). Call wait_for_generation again with the same id.")));
    result
}

/// The job's state, shaped like its status tool's result, files linked.
fn shaped(spec: &ToolSpec, reply: &HttpReply) -> CallToolResult {
    let mut result = content::json_result(spec, reply);
    if let Some(value) = result.structured_content.clone() {
        result.content.extend(content::links(spec.def.name, &value));
    }
    result
}
