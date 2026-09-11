//! End-to-end HTTP coverage for the `model3d` family, against the mock provider.
//! Mirrors the aipix acceptance checklist item-for-item.

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

use axum::body::Body;
use axum::http::{Request, StatusCode};
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::DatabaseStore;
use litegen::types::{GenerationStatus, RequestLog};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn submit_poll_and_complete_yields_exactly_one_absolute_mesh_asset() {
    let (app, _db) = harness::app_with_mock_3d().await;

    // Submit.
    let resp = app.clone().oneshot(
        Request::post("/v1/models3d/generations")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"model":"mock/mesh-3d","prompt":"a low-poly fox"}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let submitted: serde_json::Value = harness::json_body(resp).await;
    assert_eq!(submitted["status"], "pending");
    assert_eq!(submitted["progress"], 0);
    assert!(submitted.get("assets").is_none(), "no assets before completion");
    let id = submitted["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("litegen-3d-"));

    // Poll to terminal.
    let mut last = serde_json::Value::Null;
    for _ in 0..10 {
        let resp = app.clone().oneshot(
            Request::get(format!("/v1/models3d/{id}")).body(Body::empty()).unwrap(),
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        last = harness::json_body(resp).await;
        if last["status"] == "completed" || last["status"] == "failed" { break; }
    }

    assert_eq!(last["status"], "completed");
    assert_eq!(last["progress"], 100);
    let assets = last["assets"].as_array().expect("completed carries assets");
    let meshes: Vec<_> = assets.iter().filter(|a| a["kind"] == "mesh").collect();
    assert_eq!(meshes.len(), 1, "exactly one mesh asset, always");
    let url = meshes[0]["url"].as_str().unwrap();
    assert!(url.starts_with("http://") || url.starts_with("https://"), "absolute url required, got {url}");
    assert!(url.ends_with(".glb"));
    assert_eq!(meshes[0]["format"], "glb");
}

#[tokio::test]
async fn the_mesh_url_actually_downloads_a_glb() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let url = harness::run_to_completion(&app, "mock/mesh-3d", "a fox").await;

    // Strip the origin: the asset route is mounted on this same app.
    let path = url.splitn(4, '/').nth(3).map(|p| format!("/{p}")).unwrap();
    let resp = app.oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "model/gltf-binary");
    let bytes = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024).await.unwrap();
    assert_eq!(&bytes[0..4], b"glTF", "the served bytes are a real GLB");
}

#[tokio::test]
async fn lax_mode_drops_unsupported_params_and_reports_them_in_the_header() {
    // The single most important behaviour in the aipix contract.
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.oneshot(
        Request::post("/v1/models3d/generations")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"model":"mock/mesh-3d","prompt":"a fox","strict":false,"rig":true,"symmetry":"on"}"#,
            ))
            .unwrap(),
    ).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK, "lax must not error on unsupported params");
    let dropped = resp.headers().get("x-litegen-dropped-params")
        .expect("X-Litegen-Dropped-Params must be set")
        .to_str().unwrap().to_string();
    assert!(dropped.contains("rig"), "got {dropped}");
    assert!(dropped.contains("symmetry"), "got {dropped}");
}

#[tokio::test]
async fn strict_mode_rejects_the_same_request() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.oneshot(
        Request::post("/v1/models3d/generations")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"model":"mock/mesh-3d","prompt":"a fox","rig":true}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = harness::json_body(resp).await;
    assert_eq!(body["error"]["code"], "param_unsupported");
}

#[tokio::test]
async fn cost_endpoint_returns_a_cost_estimate() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.oneshot(
        Request::post("/v1/models3d/cost")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"model":"mock/mesh-3d","prompt":"a fox"}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = harness::json_body(resp).await;
    for key in ["base_cost_usd", "markup_usd", "total_cost_usd", "tokens_required", "cost_source"] {
        assert!(body.get(key).is_some(), "CostEstimate missing '{key}': {body}");
    }
}

#[tokio::test]
async fn generation_row_records_media_type_model3d_and_the_asset_metadata() {
    let (app, db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/mesh-3d", "a fox").await;
    harness::poll_until_terminal(&app, &id).await;

    let row = db.get_generation(&id).await.unwrap().unwrap();
    assert_eq!(row.media_type, "model3d", "aipix discriminates on this exact string");
    assert!(row.result_url.as_deref().is_some_and(|u| u.ends_with(".glb")),
        "result_url is the primary mesh: {:?}", row.result_url);
    let meta = row.metadata.expect("assets are persisted for the gallery");
    let assets = meta["assets"].as_array().expect("metadata.assets");
    assert!(assets.iter().any(|a| a["kind"] == "mesh"));
}

#[tokio::test]
async fn list_models_exposes_the_3d_model_and_filters_by_media_type() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(
        Request::get("/v1/models?media_type=model3d").body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = harness::json_body(resp).await;
    let models = body["data"].as_array().unwrap();
    assert!(!models.is_empty(), "at least one model3d model must be listed");
    assert!(models.iter().all(|m| m["media_type"] == "model3d"), "filter must exclude other families");
    let mesh = models.iter().find(|m| m["id"] == "mock/mesh-3d").unwrap();
    assert_eq!(mesh["capabilities"]["supports_text_to_3d"], true);
    assert_eq!(mesh["capabilities"]["output_formats"][0], "glb");
}

#[tokio::test]
async fn model_schema_endpoint_serves_a_populated_params_map_for_3d() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.oneshot(
        Request::get("/v1/models/mock/all-params-3d").body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let schema: serde_json::Value = harness::json_body(resp).await;
    assert_eq!(schema["media_type"], "model3d");
    let params = schema["params"].as_object().expect("params map");
    for key in ["output_format", "texture", "pbr", "rig", "symmetry", "topology", "target_polycount"] {
        assert!(params.contains_key(key), "3D schema must expose '{key}': {:?}", params.keys());
    }
    assert_eq!(params["target_polycount"]["label"], "Target polycount");
}

#[tokio::test]
async fn failing_model_terminates_failed_with_an_error_and_no_assets() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/fail-3d", "anything").await;
    let last = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(last["status"], "failed");
    assert!(last["error"].as_str().is_some_and(|e| !e.is_empty()));
    assert!(last.get("assets").is_none(), "a failed generation carries no assets");
}

// ─── Endpoint ↔ media-type family ───────────────────────────────────────────
//
// Each generation/cost endpoint serves exactly one family. Until Task 17C the
// Validated* extractors never compared the resolved schema's `media_type` with
// the endpoint, so a mesh model posted to `/v1/images/cost` was quoted — and
// posted to `/v1/images/generations`, dispatched — by the image provider.

async fn post_json(app: &axum::Router, path: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let resp = app.clone().oneshot(
        Request::post(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    ).await.unwrap();
    let status = resp.status();
    (status, harness::json_body(resp).await)
}

#[tokio::test]
async fn image_cost_rejects_a_3d_model_as_a_media_type_mismatch() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, body) = post_json(
        &app, "/v1/images/cost", json!({ "model": "mock/mesh-3d", "prompt": "a fox" }),
    ).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["type"], "validation_error");
    assert_eq!(body["error"]["code"], "model_media_type_mismatch");
    assert_eq!(body["error"]["param"], "model");
    assert_eq!(body["error"]["model"], "mock/mesh-3d");
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("/v1/models3d/generations"), "must name the endpoint that serves the model: {msg}");
}

#[tokio::test]
async fn models3d_generation_rejects_an_image_model_as_a_media_type_mismatch() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, body) = post_json(
        &app, "/v1/models3d/generations", json!({ "model": "mock/image-gen", "prompt": "a fox" }),
    ).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["type"], "validation_error");
    assert_eq!(body["error"]["code"], "model_media_type_mismatch");
    assert_eq!(body["error"]["model"], "mock/image-gen");
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("/v1/images/generations"), "must name the endpoint that serves the model: {msg}");
}

#[tokio::test]
async fn every_generation_and_cost_endpoint_accepts_only_its_own_family() {
    let (app, _db) = harness::app_with_mock_3d().await;
    // (endpoint, the one mock model of the family it serves)
    let endpoints = [
        ("/v1/images/generations", "mock/image-gen"),
        ("/v1/images/cost", "mock/image-gen"),
        ("/v1/videos/generations", "mock/video-gen"),
        ("/v1/videos/cost", "mock/video-gen"),
        ("/v1/models3d/generations", "mock/mesh-3d"),
        ("/v1/models3d/cost", "mock/mesh-3d"),
    ];
    for (path, own_family) in endpoints {
        for model in ["mock/image-gen", "mock/video-gen", "mock/mesh-3d"] {
            let (status, body) = post_json(&app, path, json!({ "model": model, "prompt": "a fox" })).await;
            if model == own_family {
                assert_eq!(status, StatusCode::OK, "same-family {model} on {path} must still work: {body}");
            } else {
                assert_eq!(status, StatusCode::BAD_REQUEST, "{model} on {path}: {body}");
                assert_eq!(body["error"]["code"], "model_media_type_mismatch", "{model} on {path}: {body}");
            }
        }
    }
}

#[tokio::test]
async fn a_media_type_mismatch_is_reported_before_param_validation() {
    // `size` is not a param of mock/mesh-3d, so strict image-param validation
    // alone would answer `param_unsupported` — which hides the real problem.
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, body) = post_json(
        &app,
        "/v1/images/generations",
        json!({ "model": "mock/mesh-3d", "prompt": "a fox", "size": "1024x1024" }),
    ).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "model_media_type_mismatch", "{body}");
}

// ─── Request-log lifecycle ──────────────────────────────────────────────────
//
// The submit handler logs a 3D request `pending` under the generation's id.
// Until Task 17C nothing could update a request log, so every async row in
// `/v1/logs` read `pending` forever, whatever became of the generation.

async fn request_log(db: &SqliteDatabase, id: &str) -> Option<RequestLog> {
    let (logs, _) = db.get_request_logs(1, 100).await.unwrap();
    logs.into_iter().find(|l| l.id == id)
}

/// The submit handler writes the log row from a spawned task. Waiting for it
/// keeps the terminal assertions about the terminal update alone — a terminal
/// update that raced ahead of the insert would be a different failure.
async fn wait_for_request_log(db: &SqliteDatabase, id: &str) -> RequestLog {
    for _ in 0..200 {
        if let Some(log) = request_log(db, id).await {
            return log;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("the submit handler never wrote a request log for {id}");
}

#[tokio::test]
async fn a_completed_3d_generation_moves_its_request_log_from_pending_to_completed() {
    let (app, db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/mesh-3d", "a fox").await;
    let submitted = wait_for_request_log(&db, &id).await;
    assert_eq!(submitted.status, GenerationStatus::Pending, "submit-time logging is unchanged");

    let last = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(last["status"], "completed");

    let log = request_log(&db, &id).await.unwrap();
    assert_eq!(log.status, GenerationStatus::Completed);
    assert_eq!(log.error, None);
}

#[tokio::test]
async fn a_failed_3d_generation_moves_its_request_log_to_failed_with_the_error() {
    let (app, db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/fail-3d", "anything").await;
    wait_for_request_log(&db, &id).await;

    let last = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(last["status"], "failed");

    let log = request_log(&db, &id).await.unwrap();
    assert_eq!(log.status, GenerationStatus::Failed);
    assert!(log.error.as_deref().is_some_and(|e| !e.is_empty()), "{:?}", log.error);
    assert_eq!(log.error.as_deref(), last["error"].as_str(), "the log carries the error the caller saw");
}

// ─── `usage` on the poll response (aipix §7: "usage.cost_usd / cost_source
// populated on completion") ────────────────────────────────────────────────
//
// The contract says `usage` "carries the same cost_usd / cost_source / tokens
// triple as video, so aipix's existing applyMarkup + usdToTokens cost path
// works unchanged". Until Task 17D only the SUBMIT response carried it: every
// poll — router-answered or DB-answered — hard-coded `usage: None`, so the
// consumer's cost path saw nothing on the one response it actually reads.
//
// The mock 3D models are priced $0, so the assertion here is that the field is
// PRESENT and well-formed; the derivation from a non-zero cost is unit-tested
// in `litegen-core/src/api/handlers/mod.rs` (`model3d_usage_derivation_tests`).

fn assert_usage_triple(resp: &serde_json::Value, ctx: &str) {
    let usage = resp.get("usage").unwrap_or_else(|| panic!("{ctx}: `usage` missing: {resp}"));
    assert!(!usage.is_null(), "{ctx}: `usage` is null: {resp}");
    assert!(usage["cost_usd"].is_number(), "{ctx}: usage.cost_usd must be a number: {usage}");
    assert!(usage["tokens"].is_number(), "{ctx}: usage.tokens must be a number: {usage}");
    assert_eq!(usage["cost_source"], "estimated", "{ctx}: usage.cost_source: {usage}");
}

#[tokio::test]
async fn a_completed_3d_poll_carries_the_usage_triple() {
    let (app, _db) = harness::app_with_mock_3d().await;

    let resp = app.clone().oneshot(
        Request::post("/v1/models3d/generations")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"model":"mock/mesh-3d","prompt":"a fox"}"#))
            .unwrap(),
    ).await.unwrap();
    let submitted = harness::json_body(resp).await;
    assert_usage_triple(&submitted, "submit");
    let id = submitted["id"].as_str().unwrap().to_string();

    let last = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(last["status"], "completed", "{last}");
    assert_usage_triple(&last, "the completed poll");
    assert_eq!(
        last["usage"]["cost_usd"], submitted["usage"]["cost_usd"],
        "the poll must report the same cost the submit quoted: {last}",
    );
}

#[tokio::test]
async fn the_db_answered_poll_after_completion_still_carries_usage() {
    // Once a job has terminalised the router drops it, so every later poll is
    // answered from the generation row. That path must carry `usage` too — a
    // client that polls once more (or comes back later) still needs the cost.
    let (app, _db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/mesh-3d", "a fox").await;
    assert_eq!(harness::poll_until_terminal(&app, &id).await["status"], "completed");

    let resp = app.clone().oneshot(
        Request::get(format!("/v1/models3d/{id}")).body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let again = harness::json_body(resp).await;
    assert_eq!(again["status"], "completed", "a terminal row must not be re-polled: {again}");
    assert_usage_triple(&again, "the DB-answered poll");
}

/// Seed a terminal 3D generation row directly, so the cost is a real one.
/// Every mock 3D model is priced $0, and `usage.cost_usd` == 0.0 would pass an
/// "is populated" assertion whether or not the cost actually came from the row.
async fn seed_terminal_row(db: &SqliteDatabase, id: &str, cost_usd: f64, status: &str) {
    db.insert_generation(
        id, None, "mock/mesh-3d", "mock", "model3d", Some("provider-job-1"),
        cost_usd, Some(litegen::api::middleware::DEFAULT_ORG_ID), None,
    ).await.unwrap();
    db.update_generation_status(id, status, 100, None, None, Some(chrono::Utc::now()))
        .await.unwrap();
}

#[tokio::test]
async fn the_poll_reports_the_rows_real_cost_not_a_placeholder() {
    // The value derivation, through the real router: $0.042 at $0.001/token is
    // 42 tokens. A hard-coded or re-estimated cost would not survive this.
    let (app, db) = harness::app_with_mock_3d().await;
    let id = "litegen-3d-priced-row";
    seed_terminal_row(&db, id, 0.042, "completed").await;

    let polled = get_3d(&app, id).await;
    assert_eq!(polled["status"], "completed");
    assert_usage_triple(&polled, "a priced row");
    assert_eq!(polled["usage"]["cost_usd"], 0.042, "{polled}");
    assert_eq!(polled["usage"]["tokens"], 42, "$0.042 / $0.001 per token: {polled}");
}

#[tokio::test]
async fn a_failed_3d_poll_carries_usage_too() {
    // A failed generation was still dispatched, so the consumer needs the cost
    // triple to reconcile (and refund) against.
    let (app, _db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/fail-3d", "anything").await;
    let last = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(last["status"], "failed");
    assert_usage_triple(&last, "the failed poll");
}

// ─── Cancellation (aipix §7: "PATCH /v1/generations/{id} with cancelled stops
// an in-flight job") ───────────────────────────────────────────────────────
//
// `PATCH` marked the row cancelled, but the router kept the in-flight job, so
// the very next `GET /v1/models3d/{id}` polled the provider anyway, saw
// `Completed`, re-hosted the assets and persisted `completed` OVER the
// cancelled row. Cancellation has to be final in both places.

async fn wait_for_generation_row(db: &SqliteDatabase, id: &str) {
    // The submit handler inserts the generation row from a spawned task, and
    // PATCH needs the row to exist before it can cancel it.
    for _ in 0..200 {
        if db.get_generation(id).await.unwrap().is_some() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("the submit handler never inserted a generation row for {id}");
}

async fn cancel(app: &axum::Router, id: &str) -> (StatusCode, serde_json::Value) {
    let resp = app.clone().oneshot(
        Request::patch(format!("/v1/generations/{id}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"status":"cancelled"}"#))
            .unwrap(),
    ).await.unwrap();
    let status = resp.status();
    (status, harness::json_body(resp).await)
}

async fn get_3d(app: &axum::Router, id: &str) -> serde_json::Value {
    let resp = app.clone().oneshot(
        Request::get(format!("/v1/models3d/{id}")).body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    harness::json_body(resp).await
}

#[tokio::test]
async fn cancelling_an_in_flight_3d_job_survives_every_later_poll() {
    let (app, db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/mesh-3d", "a fox").await;
    wait_for_generation_row(&db, &id).await;

    // One in-flight poll, so the job is genuinely mid-ramp when it is cancelled.
    let inflight = get_3d(&app, &id).await;
    assert!(inflight["status"] == "pending" || inflight["status"] == "processing", "{inflight}");

    let (status, cancelled) = cancel(&app, &id).await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(cancelled["status"], "cancelled", "{cancelled}");

    // The mock needs 3 polls to complete; before Task 17D these two polls drove
    // it there, re-hosted the mesh and wrote `completed` over the cancelled row.
    for attempt in 0..3 {
        let polled = get_3d(&app, &id).await;
        assert_eq!(polled["status"], "cancelled", "poll {attempt} overturned the cancel: {polled}");
        assert!(polled.get("assets").is_none(), "poll {attempt} re-hosted assets: {polled}");
    }

    let row = db.get_generation(&id).await.unwrap().unwrap();
    assert_eq!(row.status, GenerationStatus::Cancelled, "the row must stay cancelled");
    assert_eq!(row.result_url, None, "no mesh may be persisted for a cancelled generation");
    let has_assets = row.metadata.as_ref()
        .and_then(|m| m.get("assets"))
        .and_then(|a| a.as_array())
        .is_some_and(|a| !a.is_empty());
    assert!(!has_assets, "no assets may be re-hosted for a cancelled generation: {:?}", row.metadata);
}

#[tokio::test]
async fn a_cancelled_3d_generation_reports_usage_and_no_error() {
    let (app, db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/mesh-3d", "a fox").await;
    wait_for_generation_row(&db, &id).await;
    assert_eq!(cancel(&app, &id).await.0, StatusCode::OK);

    let polled = get_3d(&app, &id).await;
    assert_eq!(polled["status"], "cancelled");
    assert_usage_triple(&polled, "the cancelled poll");
}
