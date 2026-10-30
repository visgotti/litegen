//! `Idempotency-Key` on every billed POST (PixelGenie L1, 3D doc §3G).
//!
//! A client that crashed after litegen accepted a job, but before it recorded
//! the job's id, retries the same request: without a key that retry is a
//! second billed job. With one, the same key and the same request within 24
//! hours replay the first response byte for byte, and bill nothing:
//!
//! - same key, same request, finished → the stored response, with
//!   `Idempotent-Replayed: true`;
//! - same key, same request, still running → 409 `idempotency_request_in_progress`;
//! - same key, another request → 409 `idempotency_key_reused`.
//!
//! A request refused before any vendor was called (validation, auth, quota,
//! an unconfigured provider) does not spend its key, so a corrected request
//! may reuse it. Handlers say a vendor was called by putting
//! [`VendorAttempted`] in the response's extensions.
//!
//! Keys belong to their owner: the org, app and API key (or session user) the
//! request authenticated as. Another owner's key of the same spelling is a
//! different key.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use sha2::{Digest, Sha256};

use super::{AppState, KeyContext};
use crate::db::{IdempotencyBegin, StoredResponse};

/// Marks a response produced after a vendor was called, so the outcome is
/// what an `Idempotency-Key` records and replays.
#[derive(Debug, Clone, Copy)]
pub struct VendorAttempted;

/// Longest key accepted, as Stripe and the IETF draft suggest.
const MAX_KEY_LEN: usize = 255;

/// The largest request body hashed; the generation routes refuse larger ones.
const MAX_BODY: usize = 25 * 1024 * 1024;

/// The largest response stored for replay (an image response carries its
/// image inline when `b64_json` was asked for).
const MAX_STORED_RESPONSE: usize = 64 * 1024 * 1024;

fn refusal(status: StatusCode, code: &str, message: &str) -> Response {
    let mut resp = (
        status,
        Json(serde_json::json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": code,
                "param": "Idempotency-Key",
            }
        })),
    )
        .into_response();
    if status == StatusCode::CONFLICT && code == "idempotency_request_in_progress" {
        resp.headers_mut().insert("retry-after", HeaderValue::from_static("1"));
    }
    resp
}

/// Whose key this is: one owner's keys never collide with another's.
pub(crate) fn scope_of(ctx: Option<&KeyContext>) -> String {
    let Some(ctx) = ctx else { return "anonymous".to_string() };
    let principal = match (&ctx.key_id, &ctx.user) {
        (Some(k), _) => format!("key:{k}"),
        (None, Some(u)) => format!("user:{}", u.user_id),
        (None, None) => "master".to_string(),
    };
    format!(
        "org:{}|app:{}|{principal}",
        ctx.org_id.as_deref().unwrap_or("-"),
        ctx.app_id.as_deref().unwrap_or("-"),
    )
}

/// The request's identity: method, path and body. A multipart body's boundary
/// is chosen per send, so it is taken out first; a JSON body is canonicalised,
/// so key order and whitespace do not make one request two.
pub(crate) fn request_hash(method: &str, path: &str, content_type: Option<&str>, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(method.as_bytes());
    h.update(b" ");
    h.update(path.as_bytes());
    h.update(b"\n");
    let boundary = content_type
        .filter(|ct| ct.starts_with("multipart/"))
        .and_then(|ct| ct.split(';').find_map(|p| p.trim().strip_prefix("boundary=")))
        .map(|b| b.trim_matches('"').to_string());
    match boundary {
        Some(b) if !b.is_empty() => {
            let text = String::from_utf8_lossy(body);
            h.update(text.replace(&b, "litegen-boundary").as_bytes());
        }
        _ => match serde_json::from_slice::<serde_json::Value>(body) {
            Ok(v) => h.update(v.to_string().as_bytes()),
            Err(_) => h.update(body),
        },
    }
    hex::encode(h.finalize())
}

/// Headers worth replaying: the body's type and litegen's own.
fn replayable_header(name: &str) -> bool {
    name == "content-type" || name == "retry-after" || name.starts_with("x-litegen-")
}

fn rebuild(stored: &StoredResponse) -> Response {
    let mut resp = Response::new(Body::from(stored.body.clone()));
    *resp.status_mut() = StatusCode::from_u16(stored.status).unwrap_or(StatusCode::OK);
    for (k, v) in &stored.headers {
        if let (Ok(k), Ok(v)) = (axum::http::HeaderName::try_from(k.as_str()), HeaderValue::from_str(v)) {
            resp.headers_mut().insert(k, v);
        }
    }
    resp
}

/// The middleware. Layered on each billed POST route, inside authentication.
pub async fn idempotency_middleware(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let Some(raw) = req.headers().get("idempotency-key") else {
        return next.run(req).await;
    };
    let key = match raw.to_str() {
        Ok(k) if !k.is_empty() && k.len() <= MAX_KEY_LEN && k.bytes().all(|b| b.is_ascii_graphic()) => k.to_string(),
        _ => {
            return refusal(
                StatusCode::BAD_REQUEST,
                "idempotency_key_invalid",
                "Idempotency-Key must be 1 to 255 visible ASCII characters",
            )
        }
    };
    let scope = scope_of(req.extensions().get::<KeyContext>());
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(crate::api::middleware::validator::err_body("body_too_large", &e.to_string(), None, "")),
            )
                .into_response()
        }
    };
    let content_type = parts.headers.get("content-type").and_then(|v| v.to_str().ok()).map(str::to_string);
    let hash = request_hash(parts.method.as_str(), parts.uri.path(), content_type.as_deref(), &bytes);

    match state.db.idempotency_begin(&scope, &key, &hash).await {
        Err(e) => {
            // Without its record the request cannot be made safe to retry,
            // and running it unguarded is exactly the double bill the key is
            // there to stop.
            tracing::error!(error = %e, "idempotency record unavailable");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": {
                    "message": "the Idempotency-Key record could not be read; retry shortly",
                    "type": "service_unavailable",
                    "code": "idempotency_unavailable",
                }})),
            )
                .into_response();
        }
        Ok(IdempotencyBegin::Replay(stored)) => {
            let mut resp = rebuild(&stored);
            resp.headers_mut().insert("idempotent-replayed", HeaderValue::from_static("true"));
            return resp;
        }
        Ok(IdempotencyBegin::Conflict) => {
            return refusal(
                StatusCode::CONFLICT,
                "idempotency_key_reused",
                "this Idempotency-Key was used for a different request; send a new key",
            )
        }
        Ok(IdempotencyBegin::InProgress) => {
            return refusal(
                StatusCode::CONFLICT,
                "idempotency_request_in_progress",
                "a request with this Idempotency-Key is still running; retry shortly",
            )
        }
        Ok(IdempotencyBegin::Started) => {}
    }

    let resp = next.run(Request::from_parts(parts, Body::from(bytes))).await;

    if resp.extensions().get::<VendorAttempted>().is_none() {
        if let Err(e) = state.db.idempotency_release(&scope, &key).await {
            tracing::warn!(error = %e, "failed to release an unspent idempotency key");
        }
        return resp;
    }

    let (rparts, rbody) = resp.into_parts();
    let rbytes = match axum::body::to_bytes(rbody, MAX_STORED_RESPONSE).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "response too large to record for its Idempotency-Key");
            return (StatusCode::INTERNAL_SERVER_ERROR, "response too large to record").into_response();
        }
    };
    let stored = StoredResponse {
        status: rparts.status.as_u16(),
        headers: rparts
            .headers
            .iter()
            .filter(|(k, _)| replayable_header(k.as_str()))
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), v.to_string())))
            .collect(),
        body: rbytes.to_vec(),
    };
    if let Err(e) = state.db.idempotency_complete(&scope, &key, &stored).await {
        // The vendor was called and this response is the only record of it;
        // answer it anyway. A retry with the key finds the run in progress
        // until it goes stale.
        tracing::error!(error = %e, "failed to record a response under its Idempotency-Key");
    }
    Response::from_parts(rparts, Body::from(rbytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_body_hashes_the_same_whatever_its_layout() {
        let a = request_hash("POST", "/v1/models3d/generations", Some("application/json"), br#"{"model":"m","prompt":"p"}"#);
        let b = request_hash("POST", "/v1/models3d/generations", Some("application/json"), b"{ \"prompt\": \"p\",\n \"model\": \"m\" }");
        assert_eq!(a, b);
        let c = request_hash("POST", "/v1/models3d/generations", Some("application/json"), br#"{"model":"m","prompt":"q"}"#);
        assert_ne!(a, c);
        let other_path = request_hash("POST", "/v1/videos/generations", Some("application/json"), br#"{"model":"m","prompt":"p"}"#);
        assert_ne!(a, other_path);
    }

    #[test]
    fn a_multipart_body_hashes_the_same_whatever_its_boundary() {
        let body = |b: &str| format!("--{b}\r\nContent-Disposition: form-data; name=\"request\"\r\n\r\n{{}}\r\n--{b}--\r\n");
        let a = request_hash("POST", "/x", Some("multipart/form-data; boundary=aaa111"), body("aaa111").as_bytes());
        let b = request_hash("POST", "/x", Some("multipart/form-data; boundary=zzz999"), body("zzz999").as_bytes());
        assert_eq!(a, b);
    }

    #[test]
    fn keys_belong_to_their_owner() {
        let ctx = |org: &str, key: Option<uuid::Uuid>| KeyContext {
            key_id: key,
            scopes: vec![],
            rpm_limit: None,
            quota_remaining: None,
            webhook_url: None,
            user: None,
            permissions: vec![],
            session_id: None,
            org_id: Some(org.to_string()),
            app_id: Some("app".to_string()),
        };
        let k = uuid::Uuid::new_v4();
        assert_ne!(scope_of(Some(&ctx("org-a", Some(k)))), scope_of(Some(&ctx("org-b", Some(k)))));
        assert_ne!(scope_of(Some(&ctx("org-a", Some(k)))), scope_of(Some(&ctx("org-a", None))));
        assert_eq!(scope_of(Some(&ctx("org-a", Some(k)))), scope_of(Some(&ctx("org-a", Some(k)))));
    }
}
