//! `Idempotency-Key` on every billed POST (PixelGenie L1, 3D doc §3G): the
//! same key and body within 24 hours replays the first response and bills
//! nothing; the same key with another body is a 409. And a vendor call that
//! timed out is never sent again.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn post(app: &axum::Router, path: &str, key: Option<&str>, body: &Value) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::post(path).header("content-type", "application/json");
    if let Some(k) = key {
        req = req.header("idempotency-key", k);
    }
    let resp = app.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = axum::body::to_bytes(resp.into_body(), 32 * 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn replayed(h: &HeaderMap) -> bool {
    h.get("idempotent-replayed").and_then(|v| v.to_str().ok()) == Some("true")
}

async fn request_logs(db: &litegen::db::sqlite::SqliteDatabase) -> usize {
    use litegen::db::DatabaseStore;
    db.get_request_logs(1, 1000).await.unwrap().0.len()
}

#[tokio::test]
async fn a_repeated_3d_submit_returns_the_same_generation_and_bills_once() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let body = json!({ "model": "mock/mesh-3d", "prompt": "a red chair" });
    let (s1, h1, first) = post(&app, "/v1/models3d/generations", Some("pg-3d-1"), &body).await;
    let (s2, h2, second) = post(&app, "/v1/models3d/generations", Some("pg-3d-1"), &body).await;
    assert_eq!((s1, s2), (StatusCode::OK, StatusCode::OK), "{first} {second}");
    assert_eq!(first["id"], second["id"]);
    assert!(!replayed(&h1) && replayed(&h2));
    assert_eq!(db.count_generations(None).await.unwrap(), 1, "one vendor job, one row");
    assert_eq!(request_logs(&db).await, 1, "one billing row");
}

#[tokio::test]
async fn a_repeated_video_submit_returns_the_same_generation() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let body = json!({ "model": "mock/video-gen", "prompt": "a wave" });
    let (_, _, first) = post(&app, "/v1/videos/generations", Some("pg-vid-1"), &body).await;
    let (s2, h2, second) = post(&app, "/v1/videos/generations", Some("pg-vid-1"), &body).await;
    assert_eq!(s2, StatusCode::OK, "{second}");
    assert_eq!(first["id"], second["id"]);
    assert!(replayed(&h2));
    assert_eq!(db.count_generations(None).await.unwrap(), 1);
}

#[tokio::test]
async fn a_repeated_image_returns_the_same_image_and_logs_once() {
    let (app, db) = harness::app_with_mock_3d().await;
    let body = json!({ "model": "mock/image-gen", "prompt": "a lighthouse", "response_format": "b64_json" });
    let (_, _, first) = post(&app, "/v1/images/generations", Some("pg-img-1"), &body).await;
    let (s2, h2, second) = post(&app, "/v1/images/generations", Some("pg-img-1"), &body).await;
    assert_eq!(s2, StatusCode::OK, "{second}");
    assert_eq!(first, second, "byte-for-byte the first response");
    assert!(replayed(&h2));
    assert_eq!(request_logs(&db).await, 1);
}

#[tokio::test]
async fn the_same_key_with_another_body_is_refused() {
    let (app, _db) = harness::app_with_mock_3d().await;
    post(&app, "/v1/models3d/generations", Some("pg-reuse"), &json!({ "model": "mock/mesh-3d", "prompt": "a chair" })).await;
    let (s, _, body) = post(&app, "/v1/models3d/generations", Some("pg-reuse"), &json!({ "model": "mock/mesh-3d", "prompt": "a table" })).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "idempotency_key_reused", "{body}");
}

#[tokio::test]
async fn a_refused_request_does_not_spend_its_key() {
    let (app, _db) = harness::app_with_mock_3d().await;
    // Refused by validation before anything ran: a corrected request may reuse the key.
    let (s1, _, b1) = post(&app, "/v1/models3d/generations", Some("pg-fix"), &json!({ "model": "mock/mesh-3d", "prompt": "" })).await;
    assert_eq!(s1, StatusCode::BAD_REQUEST, "{b1}");
    let (s2, h2, b2) = post(&app, "/v1/models3d/generations", Some("pg-fix"), &json!({ "model": "mock/mesh-3d", "prompt": "a chair" })).await;
    assert_eq!(s2, StatusCode::OK, "{b2}");
    assert!(!replayed(&h2));
}

#[tokio::test]
async fn a_malformed_key_is_refused() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let long = "k".repeat(256);
    for key in ["has space", long.as_str()] {
        let (s, _, body) = post(&app, "/v1/models3d/generations", Some(key), &json!({ "model": "mock/mesh-3d", "prompt": "x" })).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{key:?}: {body}");
        assert_eq!(body["error"]["code"], "idempotency_key_invalid", "{body}");
    }
}

/// L1's done-when: a mock whose first attempt times out shows exactly one
/// vendor call, and the 504 it produced is what a retry with the key gets.
#[tokio::test]
async fn a_timed_out_vendor_call_is_never_resent() {
    let dir = tempfile::tempdir().unwrap();
    let (app, db) = harness::app_with(dir.path(), |c| c.server.vendor_timeout_secs = Some(1)).await;
    let body = json!({ "model": "mock/slow-image", "prompt": "a slow lighthouse" });
    let before = litegen::providers::image::mock::slow_image_calls("a slow lighthouse");

    let first = {
        let app = app.clone();
        let body = body.clone();
        tokio::spawn(async move { post(&app, "/v1/images/generations", Some("pg-slow"), &body).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (s_dup, h_dup, b_dup) = post(&app, "/v1/images/generations", Some("pg-slow"), &body).await;
    assert_eq!(s_dup, StatusCode::CONFLICT, "a duplicate while the first runs: {b_dup}");
    assert_eq!(b_dup["error"]["code"], "idempotency_request_in_progress");
    assert!(h_dup.get("retry-after").is_some());

    let (s1, _, b1) = first.await.unwrap();
    assert_eq!(s1, StatusCode::GATEWAY_TIMEOUT, "{b1}");
    assert_eq!(b1["error"]["code"], "vendor_timeout", "{b1}");
    assert_eq!(litegen::providers::image::mock::slow_image_calls("a slow lighthouse") - before, 1, "one vendor call, never re-sent");

    let (s3, h3, b3) = post(&app, "/v1/images/generations", Some("pg-slow"), &body).await;
    assert_eq!(s3, StatusCode::GATEWAY_TIMEOUT, "{b3}");
    assert!(replayed(&h3), "the outcome is replayed, not re-run");
    assert_eq!(litegen::providers::image::mock::slow_image_calls("a slow lighthouse") - before, 1);
    assert_eq!(request_logs(&db).await, 1);
}

/// A client that hangs up mid-call does not strand its key: the request runs
/// to its end and its outcome is recorded, so the retry the client makes is
/// a replay — never a 409 for minutes, then a second, billed run.
#[tokio::test]
async fn a_client_that_hangs_up_mid_call_gets_the_recorded_outcome_on_retry() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _db) = harness::app_with(dir.path(), |c| c.server.vendor_timeout_secs = Some(1)).await;
    let body = json!({ "model": "mock/slow-image", "prompt": "a lighthouse, abandoned" });

    let hung_up = tokio::time::timeout(std::time::Duration::from_millis(300), post(&app, "/v1/images/generations", Some("pg-hangup"), &body)).await;
    assert!(hung_up.is_err(), "the client gave up before the vendor answered");
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    let (s, h, b) = post(&app, "/v1/images/generations", Some("pg-hangup"), &body).await;
    assert_eq!(s, StatusCode::GATEWAY_TIMEOUT, "{b}");
    assert!(replayed(&h), "the abandoned run's outcome, replayed: {b}");
}

/// E12: a client that hangs up mid-call without an `Idempotency-Key` leaves
/// nothing reserved. The request runs to its end in a task of its own, as a
/// keyed one does, so the vendor's refusal releases the key's quota and its
/// limits' spend, and the failure is logged. The handler used to be dropped
/// at the vendor call, leaving both reservations held for good.
#[tokio::test]
async fn a_client_that_hangs_up_without_a_key_leaves_nothing_reserved() {
    use litegen::db::DatabaseStore;
    use litegen::management::types::EntityType;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    const MASTER: &str = "sk-e12-master";

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fal-ai/flux/dev"))
        .respond_with(
            ResponseTemplate::new(422)
                .set_body_json(json!({ "detail": [{ "msg": "refused", "type": "x" }] }))
                .set_delay(std::time::Duration::from_millis(1500)),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db = std::sync::Arc::new(litegen::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let uri = server.uri();
    let app = harness::app_over_with(
        db.clone(),
        dir.path(),
        move |c| {
            c.providers.insert("fal".into(), serde_json::from_value(json!({ "enabled": true, "api_key": "k", "api_base": uri })).unwrap());
        },
        Some(MASTER),
        litegen::config::Mode::SingleTenant,
    )
    .await;
    let send = |method: &str, uri: &str, bearer: &str, body: Value| {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {bearer}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let created = app.clone().oneshot(send("POST", "/v1/keys", MASTER, json!({ "name": "e12", "token_quota": 10.0 }))).await.unwrap();
    let created: Value = serde_json::from_slice(&axum::body::to_bytes(created.into_body(), 1 << 20).await.unwrap()).unwrap();
    let (key_id, secret) = (created["id"].as_str().unwrap().to_string(), created["key"].as_str().unwrap().to_string());
    let limits = app
        .clone()
        .oneshot(send("PUT", &format!("/v1/keys/{key_id}/limits"), MASTER, json!({ "max_budget_usd": 5.0 })))
        .await
        .unwrap();
    assert_eq!(limits.status(), StatusCode::OK);
    let key_uuid: uuid::Uuid = key_id.parse().unwrap();
    let held = || {
        let db = db.clone();
        let key_id = key_id.clone();
        async move {
            let tokens = db.get_api_key(&key_uuid).await.unwrap().unwrap().tokens_used;
            let spend = db.management().unwrap().get_limits(EntityType::Key, &key_id).await.unwrap().unwrap().spend_usd;
            let failed = db.get_request_logs(1, 100).await.unwrap().0.iter().filter(|r| r.status == litegen::types::GenerationStatus::Failed).count();
            (tokens, spend, failed)
        }
    };

    // No Idempotency-Key. The client hangs up once the vendor has the call.
    let call = tokio::spawn(app.clone().oneshot(send(
        "POST",
        "/v1/images/generations",
        &secret,
        json!({ "model": "fal/flux-dev", "prompt": "a lighthouse, abandoned" }),
    )));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while server.received_requests().await.unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "the vendor never got the call");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let (tokens, spend, _) = held().await;
    assert!(tokens > 0.0 && spend > 0.0, "the request holds its price while the vendor works: key {tokens}, limits {spend}");
    call.abort();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let (mut tokens, mut spend, mut failed) = held().await;
    while (tokens, spend, failed) != (0.0, 0.0, 1) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (tokens, spend, failed) = held().await;
    }
    assert_eq!(
        (tokens, spend),
        (0.0, 0.0),
        "the client hung up and the vendor refused: {tokens} USD still reserved on the key, {spend} on its limits"
    );
    assert_eq!(failed, 1, "the request ran to its end and logged its failure");
}

/// E9: an adapter's refusal before any call (MiniMax Hailuo makes only 6 s
/// at 1080P) is the caller's to fix: a 400 `validation_error` /
/// `request_unsupported` naming the model, and the key isn't spent, so the
/// corrected request may reuse it. It used to answer `invalid_request_error`
/// and record the key, so the corrected request got 409
/// `idempotency_key_reused`.
#[tokio::test]
async fn an_adapter_refusal_does_not_spend_its_key() {
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "base_resp": { "status_code": 2013, "status_msg": "refused" } })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let uri = server.uri();
    let (app, _db) = harness::app_with(dir.path(), move |c| {
        c.providers.insert("minimax".into(), serde_json::from_value(json!({ "enabled": true, "api_key": "k", "api_base": uri })).unwrap());
    })
    .await;
    let asked = json!({ "model": "minimax/MiniMax-Hailuo-02", "prompt": "a wave", "duration_seconds": 10, "resolution": "1080P" });
    let (s1, _, b1) = post(&app, "/v1/videos/generations", Some("pg-adapter-refusal"), &asked).await;
    assert_eq!(s1, StatusCode::BAD_REQUEST, "{b1}");
    let e = &b1["error"];
    assert_eq!((e["type"].as_str(), e["code"].as_str()), (Some("validation_error"), Some("request_unsupported")), "{b1}");
    assert_eq!(e["model"], "minimax/MiniMax-Hailuo-02", "{b1}");
    assert_eq!(e["retryable"], json!(false), "{b1}");
    assert!(server.received_requests().await.unwrap().is_empty(), "refused before any call");

    let corrected = json!({ "model": "minimax/MiniMax-Hailuo-02", "prompt": "a wave", "duration_seconds": 6, "resolution": "1080P" });
    let (s2, _, b2) = post(&app, "/v1/videos/generations", Some("pg-adapter-refusal"), &corrected).await;
    assert_ne!(s2, StatusCode::CONFLICT, "the refused request spent the key: {b2}");
    assert_eq!(b2["error"]["code"], "vendor_rejected", "the corrected request reached the vendor: {b2}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// N1 (P9): a keyed body is read through the same body budget as any other,
/// so a burst of large keyed bodies past `max_in_flight` × 25 MiB is refused
/// as an unkeyed burst is. It used to be read outside every bound.
#[tokio::test]
async fn keyed_bodies_count_against_the_buffering_bound() {
    use futures::StreamExt;
    const MIB: usize = 1024 * 1024;
    let dir = tempfile::tempdir().unwrap();
    let (app, _db) = harness::app_with(dir.path(), |c| c.backpressure.max_in_flight = 1).await;
    // 20 MiB, then nothing more; `taken_in` fires when the server asks for
    // more after the last chunk, so it has taken in all 20 MiB.
    let (taken_in_tx, taken_in_rx) = tokio::sync::oneshot::channel::<()>();
    let chunks = futures::stream::iter((0..20).map(|_| Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b'x'; MIB]))));
    let stall = futures::stream::once(async move {
        let _ = taken_in_tx.send(());
        futures::future::pending::<Result<axum::body::Bytes, std::io::Error>>().await
    });
    let held = Request::post("/v1/images/generations")
        .header("content-type", "application/json")
        .header("idempotency-key", "n1-held")
        .body(Body::from_stream(chunks.chain(stall)))
        .unwrap();
    let held = tokio::spawn(app.clone().oneshot(held));
    taken_in_rx.await.unwrap();
    let padded = json!({ "model": "mock/image-gen", "prompt": "x", "pad": "x".repeat(10 * MIB) });
    let (status, _, body) = post(&app, "/v1/images/generations", Some("n1-second"), &padded).await;
    held.abort();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "server_at_capacity", "{body}");
}
