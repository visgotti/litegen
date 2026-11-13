//! The axum layers that run hooks around the generation routes, and the
//! error answers they give: `refused_by_hook` (the hook's own status),
//! `hook_failed` (503) and `server_at_capacity` (503, as backpressure).

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::process::{HookCallError, HookFn, HookOutcome, Refusal};
use super::request::{apply_answer, check_answer, headers_view, hook_view, merge_headers, readable};
use super::{Hook, HookSet, OnError};

/// The request as `pre_call` left it, for `post_call` to see without reading
/// the body again.
#[derive(Clone)]
pub(crate) struct PreCallView(pub Value);

/// Headers a refusal may not set: litegen writes the body and its framing.
const NOT_FROM_A_REFUSAL: [&str; 9] =
    ["content-type", "content-length", "transfer-encoding", "connection", "keep-alive", "proxy-connection", "upgrade", "te", "trailer"];

/// Runs every hook's `pre_call` on the raw request, before authentication.
pub async fn pre_call_layer(State(hooks): State<Arc<HookSet>>, req: Request, next: Next) -> Response {
    if !hooks.defines(HookFn::PreCall) {
        return next.run(req).await;
    }
    let reads_body = readable(req.method(), req.uri().path());
    // With every slot taken, refuse now rather than after the upload, as
    // backpressure does on the routes without hooks.
    if reads_body && hooks.buffering.available_permits() == 0 {
        return at_capacity();
    }
    // The body first, charged to the hook layer's budget as it arrives and
    // within its deadline: a client still sending holds only what it sent.
    let (hooked, original) = match hook_view(req, &hooks.body).await {
        Ok(view) => view,
        Err(response) => return response,
    };
    // Then a slot, for a request whose body this layer read: it bounds the
    // requests this layer holds before litegen's own backpressure slot is
    // taken; past it, the same quick 503 that backpressure gives. A request
    // with no body read here (a status read, a file) never waits on it.
    let _buffering = match reads_body.then(|| hooks.buffering.clone().try_acquire_owned()) {
        Some(Err(_)) => return at_capacity(),
        held => held.and_then(Result::ok),
    };
    let mut current = original;
    let mut changed = false;
    for hook in hooks.with(HookFn::PreCall) {
        match hook.process.call(HookFn::PreCall, vec![current.clone()], hook.timeout).await {
            Ok(HookOutcome::Unchanged) => {}
            Ok(HookOutcome::Replaced(next_view)) => match check_answer(&hooked, &next_view) {
                Ok(()) => {
                    current = next_view;
                    changed = true;
                }
                Err(m) => {
                    if let Some(response) = failed(hook, HookFn::PreCall, &HookCallError::Malformed(m)) {
                        return response;
                    }
                }
            },
            Ok(HookOutcome::Refused(r)) => return refused(&hook.name, r),
            Err(e) => {
                if let Some(response) = failed(hook, HookFn::PreCall, &e) {
                    return response;
                }
            }
        }
    }
    // The view holds the whole body: it goes along only for a post_call to
    // read, rather than costing every request a second copy.
    let keep = hooks.defines(HookFn::PostCall).then(|| current.clone());
    match apply_answer(hooked, changed.then_some(current)) {
        Ok(mut req) => {
            if let Some(seen) = keep {
                req.extensions_mut().insert(PreCallView(seen));
            }
            next.run(req).await
        }
        Err(m) => hook_failed("pre_call", &HookCallError::Malformed(m)),
    }
}

/// Runs every hook's `post_call` on the response the handler produced,
/// inside authentication, so the hooks know the caller. Only a JSON body is
/// read; any other body (the converter's file) streams through untouched.
pub async fn post_call_layer(State(hooks): State<Arc<HookSet>>, req: Request, next: Next) -> Response {
    if !hooks.defines(HookFn::PostCall) {
        return next.run(req).await;
    }
    let reads_body = readable(req.method(), req.uri().path());
    let caller = caller_view(&req);
    // With every slot taken, refuse before the upload (as in pre_call_layer).
    if reads_body && hooks.post_buffering.available_permits() == 0 {
        return at_capacity();
    }
    // The body first, when no pre_call read it already (as in pre_call_layer).
    let (req, request_view) = match req.extensions().get::<PreCallView>().map(|v| v.0.clone()) {
        Some(view) => (req, view),
        None => match hook_view(req, &hooks.body).await {
            Ok((hooked, view)) => match apply_answer(hooked, None) {
                Ok(req) => (req, view),
                Err(m) => return hook_failed("post_call", &HookCallError::Malformed(m)),
            },
            Err(response) => return response,
        },
    };
    // Then a slot, only for a request with a body (as in pre_call_layer).
    let _buffering = match reads_body.then(|| hooks.post_buffering.clone().try_acquire_owned()) {
        Some(Err(_)) => return at_capacity(),
        held => held.and_then(Result::ok),
    };

    let (mut parts, body) = next.run(req).await.into_parts();
    let framing = parts.headers.get(header::CONTENT_LENGTH).cloned();
    let is_json = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));
    let (stream, bytes) = if is_json {
        match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => (None, Some(bytes)),
            Err(e) => return hook_failed("post_call", &HookCallError::Crashed(format!("could not read litegen's response: {e}"))),
        }
    } else {
        (Some(body), None)
    };
    let body_view = bytes.as_ref().map(|b| serde_json::from_slice::<Value>(b).unwrap_or(Value::Null)).unwrap_or(Value::Null);
    let original = json!({ "status": parts.status.as_u16(), "headers": headers_view(&parts.headers), "body": body_view });

    let mut current = original.clone();
    let mut changed = false;
    for hook in hooks.with(HookFn::PostCall) {
        let args = vec![request_view.clone(), current.clone(), caller.clone()];
        match hook.process.call(HookFn::PostCall, args, hook.timeout).await {
            Ok(HookOutcome::Unchanged) => {}
            Ok(HookOutcome::Replaced(next_view)) => match check_response(&next_view, is_json) {
                Ok(()) => {
                    current = next_view;
                    changed = true;
                }
                Err(m) => {
                    if let Some(response) = failed(hook, HookFn::PostCall, &HookCallError::Malformed(m)) {
                        return response;
                    }
                }
            },
            Ok(HookOutcome::Refused(r)) => return refused(&hook.name, r),
            Err(e) => {
                if let Some(response) = failed(hook, HookFn::PostCall, &e) {
                    return response;
                }
            }
        }
    }

    if changed {
        if let Some(status) = current.get("status").and_then(Value::as_u64) {
            parts.status = StatusCode::from_u16(status as u16).unwrap_or(parts.status);
        }
        if let Some(headers) = current.get("headers") {
            match merge_headers(&parts.headers, &original["headers"], headers) {
                Ok(merged) => parts.headers = merged,
                Err(m) => return hook_failed("post_call", &HookCallError::Malformed(m)),
            }
        }
    }
    let body = match (stream, bytes) {
        // The hook saw no streamed body: the stream keeps its own framing.
        (Some(stream), _) => {
            crate::hooks::request::keep_framing(&mut parts.headers, framing);
            stream
        }
        (None, Some(bytes)) => {
            let body_changed = changed && current.get("body").is_some_and(|b| b != &original["body"]);
            let bytes = if body_changed { serde_json::to_vec(&current["body"]).map(Bytes::from).unwrap_or(bytes) } else { bytes };
            parts.headers.insert(header::CONTENT_LENGTH, HeaderValue::from(bytes.len()));
            Body::from(bytes)
        }
        (None, None) => Body::empty(),
    };
    Response::from_parts(parts, body)
}

/// Check a `post_call` answer: a status litegen can send, headers that map
/// names to strings, and a body changed only where litegen's was JSON.
fn check_response(answer: &Value, body_is_json: bool) -> Result<(), String> {
    let Some(obj) = answer.as_object() else {
        return Err("a hook returns the response object, a string, or nothing".into());
    };
    if let Some(status) = obj.get("status") {
        if !status.as_u64().is_some_and(|s| (100..=599).contains(&s)) {
            return Err(format!("status {status} isn't an HTTP status"));
        }
    }
    if let Some(headers) = obj.get("headers") {
        let Some(map) = headers.as_object() else {
            return Err("headers must map names to strings".into());
        };
        for (name, value) in map {
            let Some(value) = value.as_str() else {
                return Err(format!("header {name} must be a string, not {value}"));
            };
            HeaderName::from_bytes(name.to_ascii_lowercase().as_bytes()).map_err(|_| format!("{name:?} isn't a valid header name"))?;
            HeaderValue::from_str(value).map_err(|_| format!("header {name}'s value isn't valid"))?;
        }
    }
    if !body_is_json && obj.get("body").is_some_and(|b| !b.is_null()) {
        return Err("this response's body isn't JSON, so a hook can't change it".into());
    }
    Ok(())
}

/// Who litegen authenticated: what `post_call` gets as `caller`.
fn caller_view(req: &Request) -> Value {
    caller_of(req.extensions().get::<crate::api::middleware::KeyContext>(), req.headers())
}

/// Who litegen authenticated, as a hook sees them.
fn caller_of(ctx: Option<&crate::api::middleware::KeyContext>, headers: &axum::http::HeaderMap) -> Value {
    json!({
        "key_id": ctx.and_then(|c| c.key_id).map(|k| k.to_string()),
        "org_id": ctx.and_then(|c| c.org_id.clone()),
        "app_id": ctx.and_then(|c| c.app_id.clone()),
        "end_user": headers.get("x-litegen-end-user").and_then(|v| v.to_str().ok()),
        "user": ctx.and_then(|c| c.user.as_ref()).map(|u| json!({ "id": u.user_id, "email": u.email, "role": u.role })),
    })
}

/// Ask every hook that defines `file_access` whether to serve a stored file,
/// in order; the first refusal is the answer. Runs after litegen's own owner
/// check, so a hook can only refuse what litegen would serve
/// (`docs/storage-access.md`): on the file route, and on a request that names
/// a stored generation as an input, whose `method` it is. `Err` is the
/// response to send instead.
pub async fn file_access(
    hooks: &HookSet,
    method: &axum::http::Method,
    uri: &axum::http::Uri,
    headers: &axum::http::HeaderMap,
    ctx: Option<&crate::api::middleware::KeyContext>,
    file: Value,
) -> Result<(), Response> {
    if !hooks.defines(HookFn::FileAccess) {
        return Ok(());
    }
    let request = json!({
        "method": method.as_str(),
        "path": uri.path(),
        "query": uri.query().unwrap_or(""),
        "headers": headers_view(headers),
        "body": Value::Null,
        "files": [],
    });
    let caller = caller_of(ctx, headers);
    for hook in hooks.with(HookFn::FileAccess) {
        let args = vec![request.clone(), file.clone(), caller.clone()];
        let e = match hook.process.call(HookFn::FileAccess, args, hook.timeout).await {
            Ok(HookOutcome::Unchanged) => continue,
            Ok(HookOutcome::Refused(r)) => return Err(refused(&hook.name, r)),
            // The process maps every other answer of file_access to Malformed.
            Ok(HookOutcome::Replaced(v)) => HookCallError::Malformed(format!("returned {v}")),
            Err(e) => e,
        };
        if let Some(response) = failed(hook, HookFn::FileAccess, &e) {
            return Err(response);
        }
    }
    Ok(())
}

fn at_capacity() -> Response {
    let body = crate::api::middleware::validator::err_body("server_at_capacity", "server at capacity, retry shortly", None, "");
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response();
    response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

/// A hook that failed: its policy decides between answering 503 and carrying
/// on as if it had returned nothing.
fn failed(hook: &Hook, phase: HookFn, e: &HookCallError) -> Option<Response> {
    let policy = match phase {
        HookFn::PreCall => hook.pre_policy,
        HookFn::PostCall => hook.post_policy,
        HookFn::FileAccess => hook.file_policy,
    };
    tracing::warn!(target: "litegen::hooks", hook = %hook.name, phase = phase.name(), error = %e, policy = ?policy, "hook failed");
    match policy {
        OnError::Refuse => Some(hook_failed(&hook.name, e)),
        OnError::Allow => None,
    }
}

pub(crate) fn hook_failed(hook: &str, e: &HookCallError) -> Response {
    let body = json!({ "error": {
        "type": "hook_error",
        "code": "hook_failed",
        "message": format!("hook {hook} {e}"),
        "hook": hook,
    }});
    (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
}

pub(crate) fn refused(hook: &str, r: Refusal) -> Response {
    let status = StatusCode::from_u16(r.status).unwrap_or(StatusCode::FORBIDDEN);
    let body = json!({ "error": {
        "type": "hook_error",
        "code": "refused_by_hook",
        "message": r.message,
        "hook": hook,
    }});
    let mut response = (status, Json(body)).into_response();
    for (name, value) in r.headers {
        let name = name.to_ascii_lowercase();
        if NOT_FROM_A_REFUSAL.contains(&name.as_str()) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(&value)) {
            response.headers_mut().append(name, value);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookConfig;
    use axum::routing::post;
    use tower::ServiceExt;

    fn python() -> bool {
        std::process::Command::new("python3").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    /// Whether the request reaching the handler still carries the view
    /// `pre_call` built, when the hooks are just `file`.
    async fn view_kept_with(file: &str) -> String {
        let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hooks");
        let configs = vec![serde_json::from_value::<HookConfig>(json!({ "path": file })).unwrap()];
        let hooks = HookSet::start(&configs, &fixtures, 4, std::time::Duration::from_secs(60)).await.unwrap().unwrap();
        let probe = |req: Request| async move { if req.extensions().get::<PreCallView>().is_some() { "kept" } else { "dropped" } };
        let app = axum::Router::new()
            .route("/v1/images/generations", post(probe))
            .layer(axum::middleware::from_fn_with_state(hooks, pre_call_layer));
        let req = Request::post("/v1/images/generations")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"prompt":"a fox"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        String::from_utf8(axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
    }

    /// The view holds the whole body as JSON: carrying it to the handler costs
    /// a copy of every request, so it goes along only for a post_call to read.
    #[tokio::test]
    async fn the_request_view_is_kept_only_for_a_post_call_to_read() {
        if !python() {
            eprintln!("SKIPPED: python3 not installed");
            return;
        }
        assert_eq!(view_kept_with("pre_only.py").await, "dropped");
        assert_eq!(view_kept_with("behaviour.py").await, "kept");
    }

    /// With every slot of a hook layer taken, a request with a body is refused
    /// at once, before a byte of its upload is read, as backpressure refuses
    /// on the routes without hooks. Both layers: `post_call` alone reads the
    /// body when no `pre_call` did.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn at_capacity_a_hook_layer_refuses_before_reading_the_body() {
        use futures::StreamExt as _;
        use std::sync::atomic::{AtomicBool, Ordering};
        if !python() {
            eprintln!("SKIPPED: python3 not installed");
            return;
        }
        let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hooks");
        let configs = vec![serde_json::from_value::<HookConfig>(json!({ "path": "behaviour.py" })).unwrap()];
        let mut outcomes = Vec::new();
        for layer in ["pre_call", "post_call"] {
            // One slot, and a handler that keeps the first request until the
            // gate opens: the layer holds its only slot across the handler.
            let hooks = HookSet::start(&configs, &fixtures, 1, std::time::Duration::from_secs(60)).await.unwrap().unwrap();
            let entered = Arc::new(tokio::sync::Notify::new());
            let gate = Arc::new(tokio::sync::Semaphore::new(0));
            let handler = {
                let (entered, gate) = (entered.clone(), gate.clone());
                move || async move {
                    entered.notify_one();
                    let _open = gate.acquire().await;
                    "done"
                }
            };
            let routes = axum::Router::new().route("/v1/images/generations", post(handler));
            let app = match layer {
                "pre_call" => routes.layer(axum::middleware::from_fn_with_state(hooks, pre_call_layer)),
                _ => routes.layer(axum::middleware::from_fn_with_state(hooks, post_call_layer)),
            };
            let first = Request::post("/v1/images/generations")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"prompt":"a fox"}"#))
                .unwrap();
            let parked = tokio::spawn(app.clone().oneshot(first));
            entered.notified().await;

            // A second request whose upload has begun and will never end.
            let polled = Arc::new(AtomicBool::new(false));
            let started = {
                let polled = polled.clone();
                futures::stream::once(async move {
                    polled.store(true, Ordering::SeqCst);
                    Ok::<_, std::io::Error>(Bytes::from_static(b"{\"prompt\":"))
                })
            };
            let uploading = Request::post("/v1/images/generations")
                .header("content-type", "application/json")
                .body(Body::from_stream(started.chain(futures::stream::pending())))
                .unwrap();
            let answered = tokio::time::timeout(std::time::Duration::from_secs(2), app.clone().oneshot(uploading)).await;
            gate.add_permits(1);
            let done = parked.await.unwrap().unwrap();
            assert_eq!(done.status(), StatusCode::OK, "{layer}: the parked request");

            let outcome = match answered {
                Ok(Ok(refused)) => {
                    let status = refused.status().as_u16();
                    let retry_after = refused.headers().get(header::RETRY_AFTER).and_then(|v| v.to_str().ok()).unwrap_or("-").to_string();
                    let body: Value = serde_json::from_slice(&axum::body::to_bytes(refused.into_body(), usize::MAX).await.unwrap()).unwrap();
                    let read = if polled.load(Ordering::SeqCst) { "its body read" } else { "its body unread" };
                    format!("{status} {}, Retry-After {retry_after}, {read}", body["error"]["code"].as_str().unwrap_or("-"))
                }
                Ok(Err(never)) => match never {},
                Err(_) => "no answer within 2 s: the layer was reading the upload".to_string(),
            };
            outcomes.push((layer, outcome));
        }
        let at_once = "503 server_at_capacity, Retry-After 1, its body unread".to_string();
        assert_eq!(
            outcomes,
            vec![("pre_call", at_once.clone()), ("post_call", at_once)],
            "with its only slot taken, each layer refuses a request at once, before reading its upload"
        );
    }
}
