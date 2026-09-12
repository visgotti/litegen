//! End-to-end HTTP coverage for the `model3d` family, against the mock provider.
//! Mirrors the aipix acceptance checklist item-for-item.

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

use axum::body::Body;
use axum::http::{Request, StatusCode};
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::{DatabaseStore, GenerationWrite};
use litegen::types::{GenerationStatus, MediaType, RequestLog};
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
    for key in ["output_formats", "texture", "pbr", "rig", "symmetry", "topology", "target_polycount"] {
        assert!(params.contains_key(key), "3D schema must expose '{key}': {:?}", params.keys());
    }
    assert_eq!(params["target_polycount"]["label"], "Target polycount");
    // The Playground and aipix both render the format control off this spec, so
    // the cardinality has to be on the wire, not only in the validator.
    assert_eq!(params["output_formats"]["kind"], "string_array");
    assert_eq!(params["output_formats"]["max_items"], 3);
    assert_eq!(params["output_formats"]["enum_values"], serde_json::json!(["glb", "obj", "stl"]));
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
    // The `"model3d"` arm of `parse_media_type` has no other assertion: without
    // this, deleting it would keep the suite green and render every 3D row on
    // the Logs surface as an image.
    assert_eq!(log.media_type, MediaType::Model3d, "the log round-trips as model3d");
    assert_eq!(submitted.media_type, MediaType::Model3d, "…at submit time too");
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
    if status == "completed" {
        // A `completed` row ALWAYS carries a mesh — `model3d_response_from_row`
        // reports a mesh-less completed row as `failed` (Ruling R21), so a row
        // seeded to exercise the cost path has to be a legitimate one.
        db.update_generation_metadata(id, &json!({
            "assets": [ { "kind": "mesh", "url": "https://example.test/model.glb", "format": "glb" } ]
        })).await.unwrap();
    }
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

// ─── Two terminal observers, one outcome ────────────────────────────────────
//
// `GET /v1/models3d/{id}` and the background poller BOTH terminalise a 3D
// generation, and the SDK polls faster (2s) than the poller ticks (5s). Every
// poll-driven terminal write therefore has to be guarded on the row still being
// in flight, or the loser of the race overwrites the winner's decided outcome:
// a cancel resurrected as `completed`, or a completed row reaped as `failed`.

async fn seed_pending_row(db: &SqliteDatabase, id: &str) {
    db.insert_generation(
        id, None, "mock/mesh-3d", "mock", "model3d", Some("provider-job-1"),
        0.0, Some(litegen::api::middleware::DEFAULT_ORG_ID), None,
    ).await.unwrap();
}

fn mesh_metadata(url: &str) -> serde_json::Value {
    json!({ "assets": [ { "kind": "mesh", "url": url, "format": "glb" } ] })
}

#[tokio::test]
async fn the_second_observer_of_a_terminal_row_writes_nothing() {
    let (_app, db) = harness::app_with_mock_3d().await;
    let id = "litegen-3d-guarded-row";
    seed_pending_row(&db, id).await;

    // Whoever gets there first wins — here, a cancel.
    let won = db.update_generation_if_active(
        id, "cancelled", 0, None, None, Some(chrono::Utc::now()), None,
    ).await.unwrap();
    assert_eq!(won, GenerationWrite::Written, "the first terminal observer must write the row");

    // The other observer arrives with a `completed` it polled moments earlier.
    let won_again = db.update_generation_if_active(
        id, "completed", 100, Some("https://example.test/model.glb"), None,
        Some(chrono::Utc::now()), Some(&mesh_metadata("https://example.test/model.glb")),
    ).await.unwrap();
    assert_eq!(
        won_again, GenerationWrite::AlreadyTerminal,
        "a terminal row must report the loss, not silently overwrite",
    );
    assert!(!won_again.owns_outcome(), "the loser owns neither the log update nor the webhook");

    let row = db.get_generation(id).await.unwrap().unwrap();
    assert_eq!(row.status, GenerationStatus::Cancelled, "the cancel must stand");
    assert_eq!(row.result_url, None, "no mesh may be attached to a cancelled generation");
    assert!(row.metadata.is_none(), "no assets either: {:?}", row.metadata);
}

#[tokio::test]
async fn terminalising_writes_the_status_and_the_assets_in_one_update() {
    // Two separate UPDATEs could leave a row permanently `completed` with no
    // mesh if the second one failed — a generation aipix refunds and litegen
    // billed (Ruling R21).
    let (_app, db) = harness::app_with_mock_3d().await;
    let id = "litegen-3d-atomic-row";
    seed_pending_row(&db, id).await;

    let url = "https://example.test/atomic.glb";
    assert_eq!(
        db.update_generation_if_active(
            id, "completed", 100, Some(url), None, Some(chrono::Utc::now()), Some(&mesh_metadata(url)),
        ).await.unwrap(),
        GenerationWrite::Written,
    );

    let row = db.get_generation(id).await.unwrap().unwrap();
    assert_eq!(row.status, GenerationStatus::Completed);
    assert_eq!(row.result_url.as_deref(), Some(url));
    assert_eq!(row.metadata.as_ref().unwrap()["assets"][0]["url"], url);
    assert!(row.completed_at.is_some());
}

#[tokio::test]
async fn a_non_terminal_progress_update_keeps_the_row_active() {
    let (_app, db) = harness::app_with_mock_3d().await;
    let id = "litegen-3d-progress-row";
    seed_pending_row(&db, id).await;

    assert_eq!(
        db.update_generation_if_active(id, "processing", 40, None, None, None, None).await.unwrap(),
        GenerationWrite::Written,
    );
    let row = db.get_generation(id).await.unwrap().unwrap();
    assert_eq!(row.status, GenerationStatus::Processing);
    assert_eq!(row.progress, 40);
    assert!(row.completed_at.is_none(), "a progress update must not stamp completed_at");
}

#[tokio::test]
async fn a_completed_row_with_no_mesh_reads_back_as_failed() {
    // The other half of the same guarantee: if the assets never landed, the row
    // is not a usable completed generation and must not be reported as one.
    let (app, db) = harness::app_with_mock_3d().await;
    let id = "litegen-3d-completed-no-mesh";
    seed_pending_row(&db, id).await;
    db.update_generation_status(id, "completed", 100, None, None, Some(chrono::Utc::now()))
        .await.unwrap();

    let polled = get_3d(&app, id).await;
    assert_eq!(polled["status"], "failed", "completed with no mesh is a failure: {polled}");
    assert!(polled.get("assets").is_none(), "{polled}");
    assert!(
        polled["error"].as_str().is_some_and(|e| e.contains("mesh")),
        "the caller must be told why: {polled}",
    );
}

// ─── The mock keeps answering a finished job (F15) ──────────────────────────

#[tokio::test]
async fn the_mock_answers_a_finished_3d_job_terminally_on_every_later_poll() {
    use litegen::providers::model3d::mock::MockModel3dProvider;
    use litegen::providers::{Model3dGenerationHandle, Model3dProvider};

    let (app, db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/mesh-3d", "a fox").await;
    wait_for_generation_row(&db, &id).await;
    let row = db.get_generation(&id).await.unwrap().unwrap();
    let handle = Model3dGenerationHandle {
        provider_job_id: row.provider_job_id.clone().expect("submit records the provider job id"),
        provider: "mock".to_string(),
        model: row.model.clone(),
        stage_context: None,
    };

    // The client's GET loop reaches the terminal poll first — it polls every 2s
    // against the poller's 5s tick, so this is the common case, not the corner.
    assert_eq!(harness::poll_until_terminal(&app, &id).await["status"], "completed");

    // The poller then polls the SAME job from its already-fetched row list. A
    // provider that forgets a finished job answers `unknown job` — NON-retryable
    // — and the poller reaps the completed row (and its log) as `failed`. Real
    // vendors keep answering terminally; so must the mock.
    let provider = MockModel3dProvider::new();
    for attempt in 0..3 {
        let poll = provider.poll_status(&handle).await.unwrap_or_else(|e| {
            panic!("poll {attempt} after completion must not error, got: {e}")
        });
        assert_eq!(poll.status, GenerationStatus::Completed, "poll {attempt} after completion");
        assert!(!poll.files.is_empty(), "poll {attempt} must still carry the mesh bytes");
    }
}

// ─── Minted asset URLs must be reachable (F4) ───────────────────────────────

#[test]
fn the_default_public_base_url_is_loopback_not_the_wildcard_bind_address() {
    use litegen::config::ServerConfig;

    let default = ServerConfig::default();
    assert_eq!(default.host, "0.0.0.0", "the default BIND address is unchanged");
    assert_eq!(
        default.public_base_url(), "http://127.0.0.1:4000",
        "0.0.0.0 is a bind address, not an origin a client can fetch",
    );

    let v6 = ServerConfig { host: "::".to_string(), ..ServerConfig::default() };
    assert_eq!(v6.public_base_url(), "http://127.0.0.1:4000");

    let real = ServerConfig { host: "10.0.0.5".to_string(), ..ServerConfig::default() };
    assert_eq!(real.public_base_url(), "http://10.0.0.5:4000", "a real host is used as-is");
}

#[tokio::test]
async fn the_minted_mesh_url_is_fetchable_not_a_wildcard_address() {
    // A default single-tenant install (no S3, no public_base_url) minted
    // http://0.0.0.0:4000/... — absolute, so every check passed, but
    // unreachable for remote clients and outright blocked by Chromium.
    let (app, _db) = harness::app_with_mock_3d().await;
    let url = harness::run_to_completion(&app, "mock/mesh-3d", "a fox").await;
    assert!(!url.contains("0.0.0.0"), "wildcard bind address in a client-facing URL: {url}");
    assert!(!url.contains("[::]"), "wildcard bind address in a client-facing URL: {url}");
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
}

// ─── Async request logs report terminal latency, not submit latency (F10) ───

#[tokio::test]
async fn settling_an_async_request_log_records_the_terminal_latency() {
    // An async submit logs `pending` with the ENQUEUE time as latency_ms
    // (milliseconds). Once those rows settle they join the `status='completed'`
    // percentile window, so /v1/stats p50/p95/p99 collapse toward submit time
    // for a video/3D-heavy tenant. Settling re-stamps the row with the real
    // end-to-end latency instead, which is what the percentile means.
    let (_app, db) = harness::app_with_mock_3d().await;
    let id = "litegen-3d-latency-row";
    db.log_request(id, "mock/mesh-3d", "mock", "pending", "model3d", 0.0, 7, None, None, None, None)
        .await.unwrap();

    // `created_at` is second-resolution in SQLite, so the wait has to cross one.
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    db.update_request_log_status(id, "completed", None).await.unwrap();

    let log = request_log(&db, id).await.unwrap();
    assert_eq!(log.status, GenerationStatus::Completed);
    assert!(
        log.latency_ms >= 1_000,
        "settled async latency must be end-to-end, not the 7ms enqueue: {}ms",
        log.latency_ms,
    );
}

#[tokio::test]
async fn settling_an_already_completed_log_leaves_its_latency_alone() {
    // Sync image rows are logged `completed` with their real latency. Nothing
    // must re-stamp those.
    let (_app, db) = harness::app_with_mock_3d().await;
    let id = "litegen-img-latency-row";
    db.log_request(id, "mock/image-gen", "mock", "completed", "image", 0.0, 1234, None, None, None, None)
        .await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    db.update_request_log_status(id, "failed", Some("after the fact")).await.unwrap();

    let log = request_log(&db, id).await.unwrap();
    assert_eq!(log.latency_ms, 1234, "a row that was never `pending` keeps its measured latency");
}

// ─── Mesh orientation (F34) ─────────────────────────────────────────────────

/// Positions (as `[x, y, z]`) and triangle indices of the first primitive of a
/// GLB's first mesh, read through the accessors rather than assumed offsets.
fn glb_mesh_geometry(glb: &[u8]) -> (Vec<[f32; 3]>, Vec<usize>) {
    let u32_at = |off: usize| u32::from_le_bytes([glb[off], glb[off + 1], glb[off + 2], glb[off + 3]]);
    let json_len = u32_at(12) as usize;
    let doc: serde_json::Value = serde_json::from_slice(&glb[20..20 + json_len]).expect("JSON chunk");
    let bin = &glb[20 + json_len + 8..];

    let prim = &doc["meshes"][0]["primitives"][0];
    let pos_acc = &doc["accessors"][prim["attributes"]["POSITION"].as_u64().unwrap() as usize];
    let idx_acc = &doc["accessors"][prim["indices"].as_u64().unwrap() as usize];
    let view_offset = |acc: &serde_json::Value| -> usize {
        let view = &doc["bufferViews"][acc["bufferView"].as_u64().unwrap() as usize];
        view["byteOffset"].as_u64().unwrap_or(0) as usize
            + acc["byteOffset"].as_u64().unwrap_or(0) as usize
    };

    let pos_off = view_offset(pos_acc);
    let positions = (0..pos_acc["count"].as_u64().unwrap() as usize)
        .map(|i| {
            let mut v = [0f32; 3];
            for (axis, slot) in v.iter_mut().enumerate() {
                let o = pos_off + i * 12 + axis * 4;
                *slot = f32::from_le_bytes([bin[o], bin[o + 1], bin[o + 2], bin[o + 3]]);
            }
            v
        })
        .collect();

    assert_eq!(idx_acc["componentType"], 5123, "indices are u16");
    let idx_off = view_offset(idx_acc);
    let indices = (0..idx_acc["count"].as_u64().unwrap() as usize)
        .map(|i| {
            let o = idx_off + i * 2;
            u16::from_le_bytes([bin[o], bin[o + 1]]) as usize
        })
        .collect();

    (positions, indices)
}

#[tokio::test]
async fn every_triangle_of_the_generated_mesh_faces_outward() {
    // glTF culls back faces by default: a mesh wound inside-out renders as the
    // far interior walls with inverted lighting in every viewer. The silhouette
    // of a cube survives that, which is why screenshots never caught it.
    let (app, _db) = harness::app_with_mock_3d().await;
    let url = harness::run_to_completion(&app, "mock/mesh-3d", "a low-poly fox").await;
    let path = url.splitn(4, '/').nth(3).map(|p| format!("/{p}")).unwrap();
    let resp = app.oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let glb = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024).await.unwrap();

    let (positions, indices) = glb_mesh_geometry(&glb);
    assert_eq!(indices.len() % 3, 0, "indices must be whole triangles");

    // Centroid of the hull, computed rather than assumed to be the origin.
    let mut centroid = [0f64; 3];
    for p in &positions {
        for axis in 0..3 {
            centroid[axis] += p[axis] as f64 / positions.len() as f64;
        }
    }

    for (t, tri) in indices.chunks(3).enumerate() {
        let (a, b, c) = (positions[tri[0]], positions[tri[1]], positions[tri[2]]);
        let ab = [(b[0] - a[0]) as f64, (b[1] - a[1]) as f64, (b[2] - a[2]) as f64];
        let ac = [(c[0] - a[0]) as f64, (c[1] - a[1]) as f64, (c[2] - a[2]) as f64];
        // Counter-clockwise winding ⇒ right-hand normal points out of the hull.
        let n = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        let outward = [
            (a[0] as f64 + b[0] as f64 + c[0] as f64) / 3.0 - centroid[0],
            (a[1] as f64 + b[1] as f64 + c[1] as f64) / 3.0 - centroid[1],
            (a[2] as f64 + b[2] as f64 + c[2] as f64) / 3.0 - centroid[2],
        ];
        let dot = n[0] * outward[0] + n[1] * outward[1] + n[2] * outward[2];
        assert!(
            dot > 0.0,
            "triangle {t} ({:?}) is wound back-facing: normal {n:?} points into the mesh",
            tri,
        );
    }
}

#[tokio::test]
async fn a_write_to_a_row_that_does_not_exist_yet_is_not_a_lost_race() {
    // The submit handler inserts the generation row from a SPAWNED task, so a
    // fast first poll can legitimately reach a terminal provider status before
    // the row exists. That is not "another observer won" — nobody else is
    // going to update the request log — and conflating the two left every such
    // generation's log reading `pending`.
    let (_app, db) = harness::app_with_mock_3d().await;
    let missing = db.update_generation_if_active(
        "litegen-3d-never-inserted", "completed", 100, None, None, Some(chrono::Utc::now()), None,
    ).await.unwrap();
    assert_eq!(missing, GenerationWrite::Missing);
    assert!(missing.owns_outcome(), "the caller still owns the request log and the webhook");
}

// ─── output_formats end to end ──────────────────────────────────────────────
//
// The param's whole value is that it is a PROMISE, so these tests are about the
// promise being kept or the generation failing — never a quiet downgrade.

#[tokio::test]
async fn asking_for_several_containers_returns_one_mesh_per_container() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = harness::submit_with(&app, serde_json::json!({
        "model": "mock/all-params-3d",
        "prompt": "a fox",
        "output_formats": ["glb", "obj", "stl"],
    })).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let id = harness::json_body(resp).await["id"].as_str().unwrap().to_string();

    let last = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(last["status"], "completed", "{last}");

    let meshes: Vec<&serde_json::Value> = last["assets"].as_array().unwrap()
        .iter().filter(|a| a["kind"] == "mesh").collect();
    let mut formats: Vec<&str> = meshes.iter().map(|m| m["format"].as_str().unwrap()).collect();
    formats.sort_unstable();
    assert_eq!(formats, ["glb", "obj", "stl"], "{last}");

    // Every container is a distinct key ending in its own extension — the
    // documented `model.<ext>` shape, not `model_1.obj`.
    for m in &meshes {
        let url = m["url"].as_str().unwrap();
        let ext = m["format"].as_str().unwrap();
        assert!(url.ends_with(&format!("/model.{ext}")), "unexpected key for {ext}: {url}");
    }
    // …and each one is real, distinct content rather than the glb three times.
    let sizes: std::collections::HashSet<u64> =
        meshes.iter().map(|m| m["size_bytes"].as_u64().unwrap()).collect();
    assert_eq!(sizes.len(), 3, "the three containers must not be byte-identical: {last}");
}

#[tokio::test]
async fn the_canonical_result_url_is_the_glb_whatever_order_they_arrive_in() {
    // `result_url` backs every cross-modal consumer, so it must not depend on
    // which container the adapter happened to push first.
    let (app, db) = harness::app_with_mock_3d().await;
    let resp = harness::submit_with(&app, serde_json::json!({
        "model": "mock/all-params-3d",
        "prompt": "a fox",
        "output_formats": ["stl", "obj", "glb"],
    })).await;
    let id = harness::json_body(resp).await["id"].as_str().unwrap().to_string();
    assert_eq!(harness::poll_until_terminal(&app, &id).await["status"], "completed");

    let gen = db.get_generation(&id).await.unwrap().expect("generation row");
    assert!(
        gen.result_url.as_deref().is_some_and(|u| u.ends_with("/model.glb")),
        "result_url must be the glb: {:?}", gen.result_url,
    );
}

#[tokio::test]
async fn a_provider_that_under_delivers_an_underivable_format_fails_the_generation() {
    // mock/partial-3d reports success while returning only glb — the way a real
    // vendor under-delivers when a convert step is skipped. fbx is the case that
    // still matters: obj/stl/ply are derived locally when a vendor omits them,
    // so only a container litegen cannot produce itself can go missing. The
    // caller paid for an fbx they cannot have, so this must fail.
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = harness::submit_with(&app, serde_json::json!({
        "model": "mock/partial-3d",
        "prompt": "a fox",
        "output_formats": ["glb", "fbx"],
    })).await;
    let id = harness::json_body(resp).await["id"].as_str().unwrap().to_string();

    let last = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(last["status"], "failed", "a partial delivery is not a success: {last}");
    assert!(
        last["error"].as_str().is_some_and(|e| e.contains("fbx")),
        "the error must name the container that never arrived: {last}",
    );
    assert!(last.get("assets").is_none(), "a failed generation carries no assets: {last}");
}

#[tokio::test]
async fn a_format_the_provider_omitted_is_derived_rather_than_failed() {
    // The other half of the same contract, and the reason conversion exists:
    // mock/partial-3d returns ONLY glb, and the caller still gets their obj.
    // This is the GLB-only vendor case — fal, Replicate, Stability — where
    // without derivation `output_formats` could only ever mean ["glb"].
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = harness::submit_with(&app, serde_json::json!({
        "model": "mock/mesh-3d",
        "prompt": "a fox",
        "output_formats": ["glb", "obj", "stl", "ply"],
    })).await;
    assert_eq!(resp.status(), StatusCode::OK, "a glb-only model must accept derivable formats");
    let id = harness::json_body(resp).await["id"].as_str().unwrap().to_string();

    let last = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(last["status"], "completed", "{last}");
    let mut formats: Vec<&str> = last["assets"].as_array().unwrap().iter()
        .filter(|a| a["kind"] == "mesh")
        .map(|a| a["format"].as_str().unwrap())
        .collect();
    formats.sort_unstable();
    assert_eq!(formats, ["glb", "obj", "ply", "stl"], "{last}");
    // Derived files are stored and served exactly like native ones.
    for a in last["assets"].as_array().unwrap().iter().filter(|a| a["kind"] == "mesh") {
        let ext = a["format"].as_str().unwrap();
        assert!(a["url"].as_str().unwrap().ends_with(&format!("/model.{ext}")), "{a}");
        assert!(a["size_bytes"].as_u64().unwrap() > 0, "{a}");
    }
}

#[tokio::test]
async fn the_row_answered_path_reaches_the_same_verdict_as_the_live_poll() {
    // Three observers enforce this contract independently. If the row-answered
    // path disagreed, whichever observer won the race would decide whether the
    // generation succeeded.
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = harness::submit_with(&app, serde_json::json!({
        "model": "mock/partial-3d",
        "prompt": "a fox",
        "output_formats": ["glb", "fbx"],
    })).await;
    let id = harness::json_body(resp).await["id"].as_str().unwrap().to_string();
    assert_eq!(harness::poll_until_terminal(&app, &id).await["status"], "failed");

    // The router has dropped the job by now, so this is answered from the row.
    let resp = app.clone().oneshot(
        Request::get(format!("/v1/models3d/{id}")).body(Body::empty()).unwrap(),
    ).await.unwrap();
    let again = harness::json_body(resp).await;
    assert_eq!(again["status"], "failed", "the row-answered path must agree: {again}");
}

#[tokio::test]
async fn a_container_the_model_cannot_emit_is_rejected_at_submit() {
    // Before the vendor is billed, not after.
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = harness::submit_with(&app, serde_json::json!({
        "model": "mock/mesh-3d",
        "prompt": "a fox",
        "output_formats": ["fbx"],
    })).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let err = harness::json_body(resp).await;
    assert!(
        serde_json::to_string(&err).unwrap().contains("fbx"),
        "the rejection must name the format: {err}",
    );
}

#[tokio::test]
async fn omitting_the_param_still_promises_glb() {
    // The guard must mean something on every generation, not only the ones that
    // named a format.
    let (app, _db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/mesh-3d", "a fox").await;
    let last = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(last["status"], "completed", "{last}");
    let formats: Vec<&str> = last["assets"].as_array().unwrap().iter()
        .filter(|a| a["kind"] == "mesh")
        .map(|a| a["format"].as_str().unwrap())
        .collect();
    assert_eq!(formats, ["glb"], "{last}");
}

#[tokio::test]
async fn an_adapters_stage_context_reaches_the_row_so_the_poller_can_use_it() {
    // The poller rebuilds the provider handle FROM THE ROW — it cannot see the
    // router's in-memory job map — so an adapter needing more than a job id to
    // poll with (fal's status_url/response_url, Meshy's preview task id) only
    // works if submit persists this. poller.rs has always read
    // `metadata.stage_context`; nothing wrote it until now, which made every
    // multi-stage adapter story rest on a field that was read-only in practice.
    let (app, db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/mesh-3d", "a fox").await;

    // The row insert and the metadata write are both spawned at submit.
    let mut row = None;
    for _ in 0..200 {
        if let Ok(Some(g)) = db.get_generation(&id).await {
            if g.metadata.is_some() { row = Some(g); break; }
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let row = row.expect("submit never wrote the generation metadata");
    let meta = row.metadata.unwrap();

    // The mock is single-stage, so it sets no stage_context — what must hold is
    // that the promise is recorded and the key is absent rather than null.
    assert_eq!(meta["requested_formats"], serde_json::json!(["glb"]), "{meta}");
    assert!(meta.get("stage_context").is_none(), "a single-stage adapter writes none: {meta}");
}

#[tokio::test]
async fn a_mid_flight_poll_does_not_erase_what_submit_recorded() {
    // Mid-flight progress updates must pass `None` for metadata, or every poll
    // would wipe requested_formats and stage_context — and the completion guard
    // would silently fall back to the GLB floor for the rest of the job.
    let (app, db) = harness::app_with_mock_3d().await;
    let resp = harness::submit_with(&app, serde_json::json!({
        "model": "mock/all-params-3d",
        "prompt": "a fox",
        "output_formats": ["glb", "obj"],
    })).await;
    let id = harness::json_body(resp).await["id"].as_str().unwrap().to_string();

    for _ in 0..200 {
        if db.get_generation(&id).await.ok().flatten().and_then(|g| g.metadata).is_some() { break; }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    // One poll short of terminal — the mock needs three.
    let r = app.clone().oneshot(
        Request::get(format!("/v1/models3d/{id}")).body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_ne!(harness::json_body(r).await["status"], "completed");

    let meta = db.get_generation(&id).await.unwrap().unwrap().metadata.expect("metadata survived");
    assert_eq!(meta["requested_formats"], serde_json::json!(["glb", "obj"]), "{meta}");
}
