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
//!
//! Every billed POST, key or no key, runs to its end in a task of its own: a
//! client that hangs up mid-call does not cancel the vendor call, and the
//! request's reservations are settled exactly as if it had stayed.

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

/// The largest request body hashed: the generation routes' own limit, which
/// `GET /v1/models` serves as `capabilities.max_request_bytes`, so a keyed
/// request may be as large as the listing says (dashboard-server review #1).
const MAX_BODY: usize = super::body::MAX_GENERATION_BODY;

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
    // Up to 25 MiB parsed, re-serialised and hashed: off the async workers (P15).
    crate::util::blocking::cpu_heavy(body.len(), || hash_request(method, path, content_type, body))
}

fn hash_request(method: &str, path: &str, content_type: Option<&str>, body: &[u8]) -> String {
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
            // Byte for byte: a file part needn't be UTF-8, and a lossy read
            // would make two different uploads one request.
            let needle = b.as_bytes();
            let mut rest = body;
            while let Some(i) = rest.windows(needle.len()).position(|w| w == needle) {
                h.update(&rest[..i]);
                h.update(b"litegen-boundary");
                rest = &rest[i + needle.len()..];
            }
            h.update(rest);
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
        return run_to_completion(next.run(req)).await;
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
    // Read through the same body budget and deadline as the routes behind
    // (`body.rs`): a keyed body is charged as it arrives, and handed on with
    // its charge, so the route reads it from memory without charging again.
    let (parts, body) = match state.in_flight.body().read_request(req, MAX_BODY).await {
        Ok(read) => read,
        Err(e) => return e.into_response(),
    };
    let content_type = parts.headers.get("content-type").and_then(|v| v.to_str().ok()).map(str::to_string);
    let hash = request_hash(parts.method.as_str(), parts.uri.path(), content_type.as_deref(), &body.bytes);

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

    // Run and record in a task of its own (see `run_to_completion`): the
    // outcome is stored even when the client hung up, so its retry replays it
    // instead of waiting out a stale "in progress" record and running (and
    // billing) again.
    run_to_completion(async move {
        let resp = next.run(body.into_request(parts)).await;
        settle(&*state.db, &scope, &key, resp).await
    })
    .await
}

/// Run a billed request to its end in a task of its own. A client that hangs
/// up mid-call drops this future, not the request: the vendor call still
/// finishes, and its reservations are settled exactly as if the client had
/// stayed (released when nothing was made, charged when something was). Run
/// inline, the handler was dropped at its vendor `.await` and its key quota
/// and limit reservations stayed held for good (E12). The task keeps the
/// request's span, so its vendor calls carry the request's `traceparent`.
async fn run_to_completion(fut: impl std::future::Future<Output = Response> + Send + 'static) -> Response {
    use tracing::Instrument;
    match tokio::spawn(fut.instrument(tracing::Span::current())).await {
        Ok(resp) => resp,
        Err(e) => {
            // The panic's text stays in the log; the caller gets the JSON
            // `internal_error` every other 500 has (handlers review #9).
            tracing::error!(error = %e, "a billed request's task failed");
            super::internal_error_response("internal server error")
        }
    }
}

/// Record `resp` under its key — or free the key when no vendor was called —
/// and hand it back.
async fn settle(db: &dyn crate::db::DatabaseStore, scope: &str, key: &str, resp: Response) -> Response {
    if resp.extensions().get::<VendorAttempted>().is_none() {
        if let Err(e) = db.idempotency_release(scope, key).await {
            tracing::warn!(error = %e, "failed to release an unspent idempotency key");
        }
        return resp;
    }

    let (rparts, rbody) = resp.into_parts();
    let rbytes = match axum::body::to_bytes(rbody, MAX_STORED_RESPONSE).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "response too large to record for its Idempotency-Key");
            // The JSON `internal_error` every other 500 has (FU-IDEM).
            return super::internal_error_response("response too large to record");
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
    if let Err(e) = db.idempotency_complete(scope, key, &stored).await {
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
    fn two_uploads_that_differ_only_in_binary_bytes_are_two_requests() {
        // Image bytes aren't UTF-8; read lossily, both files became U+FFFD.
        let body = |file: &[u8]| {
            let mut b = b"--bnd\r\nContent-Disposition: form-data; name=\"image\"; filename=\"a.png\"\r\n\r\n".to_vec();
            b.extend_from_slice(file);
            b.extend_from_slice(b"\r\n--bnd--\r\n");
            b
        };
        let ct = Some("multipart/form-data; boundary=bnd");
        let a = request_hash("POST", "/x", ct, &body(&[0x89, 0xff, 0xfe]));
        let b = request_hash("POST", "/x", ct, &body(&[0x89, 0xfd, 0xfc]));
        assert_ne!(a, b);
    }

    /// E12: the task a billed request runs in keeps the request's span, so its
    /// vendor calls still carry its `traceparent` (`inject_trace_headers`
    /// reads `Span::current()`) and its logs stay in `TraceLayer`'s trace.
    #[tokio::test]
    async fn a_billed_request_keeps_its_span_in_its_own_task() {
        use tracing::Instrument;
        let _subscriber = tracing::subscriber::set_default(tracing_subscriber::registry());
        let resp = run_to_completion(async {
            let name = tracing::Span::current().metadata().map_or("none", |m| m.name());
            let mut r = StatusCode::OK.into_response();
            r.headers_mut().insert("x-span", HeaderValue::from_static(name));
            r
        })
        .instrument(tracing::info_span!("billed-request"))
        .await;
        assert_eq!(resp.headers()["x-span"], "billed-request", "the handler ran outside the request's span");
    }

    /// A billed handler that panics.
    async fn a_handler_that_panics() -> Response {
        panic!("a handler bug at src/api/handlers/x.rs:12")
    }

    /// Handlers review #9: a billed request whose task panicked answers the
    /// JSON `internal_error` body every other 500 has, naming nothing of the
    /// panic. It answered `text/plain` "the request failed", which a client
    /// that reads `error.code` can't parse; since E12 that is every billed
    /// POST, keyed or not.
    #[tokio::test]
    async fn a_billed_request_that_panics_answers_the_json_internal_error() {
        let resp = run_to_completion(a_handler_that_panics()).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        assert_eq!(
            (content_type.as_str(), body["error"]["code"].as_str(), body["error"]["type"].as_str()),
            ("application/json", Some("internal_error"), Some("internal_error")),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(body["error"]["message"], "internal server error", "{body}");
    }

    /// FU-IDEM (follow-ups 2026-11-13): a vendor's answer too large to record
    /// under its Idempotency-Key answers the JSON `internal_error` body every
    /// other 500 has. It answered `text/plain` "response too large to
    /// record", which a client that reads `error.code` can't parse. The body
    /// is a MiB over the limit, sent a MiB at a time from one static buffer,
    /// so the test copies none of it.
    #[tokio::test]
    async fn a_response_too_large_to_record_answers_the_json_internal_error() {
        static MIB: [u8; 1 << 20] = [0; 1 << 20];
        let chunks = MAX_STORED_RESPONSE / MIB.len() + 1;
        let body = Body::from_stream(futures::stream::iter(
            (0..chunks).map(|_| Ok::<_, std::convert::Infallible>(bytes::Bytes::from_static(&MIB))),
        ));
        let mut resp = Response::new(body);
        resp.extensions_mut().insert(VendorAttempted);
        let db = crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap();

        let resp = settle(&db, "org:-|app:-|master", "too-large", resp).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        assert_eq!(
            (content_type.as_str(), body["error"]["code"].as_str(), body["error"]["type"].as_str()),
            ("application/json", Some("internal_error"), Some("internal_error")),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(body["error"]["message"], "response too large to record", "{body}");
    }

    /// P15 (timing; run alone): hashing a maximum-size body holds no async
    /// worker. On today's one-worker runtime, a task queued behind the hash
    /// waited as long as the hash took (over a second in a debug build).
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn hashing_a_large_body_leaves_the_runtime_free() {
        let pad = "A".repeat(25 * 1024 * 1024 - 1024);
        let body = format!(r#"{{"model":"m","prompt":"p","reference_images":[{{"type":"base64","value":"{pad}"}}]}}"#).into_bytes();
        let started = std::time::Instant::now();
        let inline = request_hash("POST", "/v1/images/generations", Some("application/json"), &body);
        let alone = started.elapsed();
        assert!(alone >= std::time::Duration::from_millis(200), "too light to show a stall: {alone:?}");
        let (hash, stall) =
            crate::util::blocking::longest_stall_while(async move { request_hash("POST", "/v1/images/generations", Some("application/json"), &body) }).await;
        assert_eq!(hash, inline, "the same hash, wherever it ran");
        assert!(stall < alone / 4, "the runtime stalled {stall:?} while hashing (the hash alone takes {alone:?})");
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
            oauth_grant_id: None,
        };
        let k = uuid::Uuid::new_v4();
        assert_ne!(scope_of(Some(&ctx("org-a", Some(k)))), scope_of(Some(&ctx("org-b", Some(k)))));
        assert_ne!(scope_of(Some(&ctx("org-a", Some(k)))), scope_of(Some(&ctx("org-a", None))));
        assert_eq!(scope_of(Some(&ctx("org-a", Some(k)))), scope_of(Some(&ctx("org-a", Some(k)))));
    }
}
