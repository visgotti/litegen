//! End-to-end HTTP coverage for the `model3d` family, against the mock provider.
//! Mirrors the aipix acceptance checklist item-for-item.

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

use axum::body::Body;
use axum::http::{Request, StatusCode};
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::{DatabaseStore, GenerationWrite};
use litegen::proxy::model3d_convert::{deliverable_formats, derivable_formats};
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
    // Served as the CALLER's set: the yaml's native [glb, obj, stl] first,
    // then every container litegen derives — exactly what the validator
    // accepts.
    let native = ["glb", "obj", "stl"].map(String::from).to_vec();
    let served: Vec<String> = serde_json::from_value(params["output_formats"]["enum_values"].clone()).unwrap();
    assert_eq!(served, deliverable_formats(&native));
    assert_eq!(served[..3], native[..], "native first, in order");
    assert_eq!(params["output_formats"]["derivable_values"], serde_json::json!(derivable_formats()));
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

// ─── A quote runs its generation's request validation (E11b) ───────────────
//
// The dashboard's playground holds Generate back on the quote's refusal, shown
// in the quote's own words, instead of keeping a copy of the validator's
// rules. That holds only while each quote route refuses a body as its
// generation route does. Both take the same `Validated*` extractor today; this
// catches a check moved into a generation handler, or a quote that skips
// validation, either of which lets the playground pass a body the generation
// then refuses. The generation refuses more than the quote (materializing,
// quota, the keep-outputs header), so this pins one direction only.

#[tokio::test]
async fn the_quote_runs_the_generations_request_validation() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let url = |name: &str, role: &str| json!({ "type": "url", "value": format!("https://example.com/{name}.png"), "role": role });
    // (route family, the refusal intended: code and param, a body that earns
    // it and nothing else). The intended refusal is asserted, not only that
    // the routes agree: a row that forgot its prompt would agree on
    // `prompt_required`.
    let rows = [
        ("models3d", "source_model_required", "source_model", json!({ "model": "mock/rig-3d" })),
        ("models3d", "ref_role_required", "reference_images", json!({
            "model": "mock/all-params-3d", "prompt": "a fox", "reference_images": [url("left", "view-left")],
        })),
        ("models3d", "param_out_of_range", "height_m", json!({ "model": "mock/mesh-3d", "prompt": "a fox", "height_m": 0 })),
        ("images", "param_out_of_range", "mask_grow_px", json!({
            "model": "mock/region-image", "prompt": "make it glow", "mask_grow_px": 101,
            "reference_images": [url("init", "init"), url("mask", "mask")],
        })),
        ("images", "param_out_of_range", "n", json!({ "model": "mock/visual-image-gen", "prompt": "a fox", "n": 2 })),
        ("videos", "source_video_required", "source_video", json!({ "model": "mock/extend-video", "prompt": "a fox" })),
    ];
    fn refusal(status: StatusCode, v: &serde_json::Value) -> (StatusCode, Option<&str>, Option<&str>) {
        (status, v["error"]["code"].as_str(), v["error"]["param"].as_str())
    }
    for (family, code, param, body) in rows {
        let (quote_status, quote) = post_json(&app, &format!("/v1/{family}/cost"), body.clone()).await;
        let (generation_status, generation) = post_json(&app, &format!("/v1/{family}/generations"), body.clone()).await;
        let intended = (StatusCode::BAD_REQUEST, Some(code), Some(param));
        assert_eq!(refusal(quote_status, &quote), intended, "the quote of {body}: {quote}");
        assert_eq!(refusal(generation_status, &generation), intended, "the generation of {body}: {generation}");
        // In the same words: the playground shows the quote's message.
        assert_eq!(quote, generation, "{body}");
    }
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
async fn a_cancelled_video_still_answers_its_status() {
    // Cancel drops the router's video job; the status endpoint must then answer
    // from the row (`cancelled`), not 404 "video job not found".
    let (app, db) = harness::app_with_mock_3d().await;
    let (status, submitted) = post_json(&app, "/v1/videos/generations", json!({ "model": "mock/video-gen", "prompt": "a fox" })).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().expect("video id").to_string();
    wait_for_generation_row(&db, &id).await;
    let (status, cancelled) = cancel(&app, &id).await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");

    let resp = app.clone().oneshot(
        Request::get(format!("/v1/videos/{id}")).body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let polled = harness::json_body(resp).await;
    assert_eq!(polled["status"], "cancelled", "{polled}");
}

#[tokio::test]
async fn a_video_id_is_not_answered_as_a_3d_generation() {
    let (app, db) = harness::app_with_mock_3d().await;
    let (_, submitted) = post_json(&app, "/v1/videos/generations", json!({ "model": "mock/video-gen", "prompt": "a fox" })).await;
    let id = submitted["id"].as_str().expect("video id").to_string();
    wait_for_generation_row(&db, &id).await;
    let resp = app.clone().oneshot(
        Request::get(format!("/v1/models3d/{id}")).body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
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
    // vendor under-delivers when a convert step is skipped — and a glb the
    // converter cannot read: it requires EXT_litegen_mock_undecodable, an
    // extension litegen will never implement (Draco and meshopt it reads).
    // litegen would derive a missing fbx from a glb it can read; from this one
    // nothing can be derived. The caller paid for an fbx they cannot have, so
    // this must fail.
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
    // Before the vendor is billed, not after. Every container litegen names
    // derives from the mock's glb, so only one outside the converter (Sloyd's
    // .blend, say) is undeliverable.
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = harness::submit_with(&app, serde_json::json!({
        "model": "mock/mesh-3d",
        "prompt": "a fox",
        "output_formats": ["blend"],
    })).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let err = harness::json_body(resp).await;
    assert!(
        serde_json::to_string(&err).unwrap().contains("blend"),
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

#[tokio::test]
async fn fal_3d_models_are_served_with_their_deliverable_formats_and_vendor_reference() {
    // A client renders the format picker straight off this spec, so it must
    // list what the validator accepts (native ∪ derived), name which share a
    // vendor job, and keep max_items as that vendor-job budget.
    let (app, _db) = harness::app_with_mock_3d().await;
    for (id, native, docs_url) in [
        ("fal/trellis", vec!["glb"], "https://fal.ai/models/fal-ai/trellis/api"),
        ("fal/hunyuan3d-v2", vec!["glb"], "https://fal.ai/models/fal-ai/hunyuan3d/v2/api"),
        ("fal/hyper3d-rodin", vec!["glb", "usdz", "fbx", "obj", "stl"], "https://fal.ai/models/fal-ai/hyper3d/rodin/api"),
    ] {
        let native: Vec<String> = native.into_iter().map(String::from).collect();
        let enum_values = json!(deliverable_formats(&native));
        let resp = app.clone().oneshot(
            Request::get(format!("/v1/models/{id}")).body(Body::empty()).unwrap(),
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{id}");
        let schema: serde_json::Value = harness::json_body(resp).await;
        assert_eq!(schema["docs_url"], docs_url, "{id}");
        let formats = &schema["params"]["output_formats"];
        assert_eq!(formats["enum_values"], enum_values, "{id}");
        assert_eq!(formats["derivable_values"], json!(derivable_formats()), "{id}");
        for f in ["fbx", "usdz", "3mf", "dae"] {
            assert!(formats["enum_values"].as_array().unwrap().iter().any(|v| v == f), "{id}: {f} is deliverable");
        }
        assert_eq!(formats["max_items"], 1, "{id}");
    }
}

#[tokio::test]
async fn the_models_list_advertises_exactly_the_formats_the_schema_serves() {
    // Two projections of one fact; a client must not see two answers.
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(
        Request::get("/v1/models?media_type=model3d").body(Body::empty()).unwrap(),
    ).await.unwrap();
    let list: serde_json::Value = harness::json_body(resp).await;
    let models = list["data"].as_array().expect("model list");
    assert!(models.len() >= 7, "the catalog's 3D models are listed: {}", models.len());
    for m in models {
        let id = m["id"].as_str().unwrap();
        let resp = app.clone().oneshot(
            Request::get(format!("/v1/models/{id}")).body(Body::empty()).unwrap(),
        ).await.unwrap();
        let schema: serde_json::Value = harness::json_body(resp).await;
        let served = schema["params"]["output_formats"]["enum_values"].clone();
        let advertised = m["capabilities"]["output_formats"].clone();
        if served.is_null() {
            assert_eq!(advertised, json!([]), "{id} declares no formats");
        } else {
            assert_eq!(advertised, served, "{id}");
        }
    }
}

#[tokio::test]
async fn an_omitted_prompt_is_a_validation_error_not_a_parse_error() {
    // `prompt` is optional on the wire; whether a model needs one is its
    // PromptSpec's call, answered with a code a client can act on.
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, body) = post_json(&app, "/v1/models3d/generations", json!({ "model": "mock/mesh-3d" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "prompt_required", "{body}");
}

/// 3D doc §3A: a stored mesh outlives a restart, so a chain of operations
/// that reads its source by generation id survives one between two steps.
#[tokio::test]
async fn a_mesh_still_downloads_after_a_restart() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    // A deployment: its database and its asset directory, both on disk.
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/litegen.db?mode=rwc", dir.path().display());
    let boot = || async {
        let db = std::sync::Arc::new(litegen::db::sqlite::SqliteDatabase::connect(&url).await.expect("file sqlite"));
        harness::app_over(db, &dir.path().join("assets"), |_| {}).await
    };
    let app = boot().await;
    let mesh = harness::run_to_completion(&app, "mock/mesh-3d", "a red chair").await;
    let path = format!("/v1/{}", mesh.split_once("/v1/").expect("an asset url").1);
    drop(app);

    // A new process: nothing in memory, the same database and asset directory.
    // (A file whose generation is gone is not served; see output_access_api.)
    let restarted = boot().await;
    let resp = restarted.oneshot(Request::get(&path).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), 200, "{path}");
    let bytes = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024).await.unwrap();
    assert_eq!(&bytes[..4], b"glTF");
}

/// Review findings 21 and 6, left to the client: `fbx_color_space` and
/// `format_source` are per-request overrides any 3D model takes. They are
/// recorded in the generation's delivery plan (the poller delivers exactly
/// what the client's own poll would), the generation completes with the
/// promised containers, and a value litegen does not take is a 400.
#[tokio::test]
async fn fbx_color_space_and_format_source_are_per_request_overrides() {
    let (app, db) = harness::app_with_mock_3d().await;
    let (status, body) = post_json(
        &app,
        "/v1/models3d/generations",
        serde_json::json!({
            "model": "mock/mesh-3d", "prompt": "a fox", "output_formats": ["glb", "fbx"],
            "fbx_color_space": "linear", "format_source": "glb"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let row = db.get_generation(&id).await.unwrap().unwrap();
    let meta = row.metadata.expect("the delivery plan is recorded at submit");
    assert_eq!(meta["delivery"]["fbx_color_space"], "linear", "{meta}");
    let done = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    let formats: Vec<&str> = done["assets"].as_array().unwrap().iter().filter(|a| a["kind"] == "mesh").filter_map(|a| a["format"].as_str()).collect();
    assert!(formats.contains(&"glb") && formats.contains(&"fbx"), "{formats:?}");
    for bad in [serde_json::json!({"fbx_color_space": "gamma"}), serde_json::json!({"format_source": "vendor"})] {
        let mut req = serde_json::json!({"model": "mock/mesh-3d", "prompt": "a fox"});
        req.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        let (status, body) = post_json(&app, "/v1/models3d/generations", req).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
}

/// `fbx_light_units` and `usd_light_units` are per-request overrides any 3D
/// model takes, as `fbx_color_space` is: recorded in the generation's delivery
/// plan (so the poller delivers what the client's own poll would), with the
/// generation completing with the promised containers. A word litegen does
/// not take is a 400 that starts no job; the default is recorded as asked.
#[tokio::test]
async fn light_units_are_per_request_overrides() {
    let (app, db) = harness::app_with_mock_3d().await;
    let (status, body) = post_json(
        &app,
        "/v1/models3d/generations",
        serde_json::json!({
            "model": "mock/mesh-3d", "prompt": "a lit fox", "output_formats": ["glb", "fbx", "usdz"],
            "fbx_light_units": "blender", "usd_light_units": "photometric"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let row = db.get_generation(&id).await.unwrap().unwrap();
    let meta = row.metadata.expect("the delivery plan is recorded at submit");
    assert_eq!(meta["delivery"]["fbx_light_units"], "blender", "{meta}");
    assert_eq!(meta["delivery"]["usd_light_units"], "photometric", "{meta}");
    let done = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    let formats: Vec<&str> = done["assets"].as_array().unwrap().iter().filter(|a| a["kind"] == "mesh").filter_map(|a| a["format"].as_str()).collect();
    assert!(formats.contains(&"glb") && formats.contains(&"fbx") && formats.contains(&"usdz"), "{formats:?}");

    // Asked for neither: nothing is recorded, as before.
    let (status, body) = post_json(&app, "/v1/models3d/generations", serde_json::json!({"model": "mock/mesh-3d", "prompt": "a fox"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = db.get_generation(body["id"].as_str().unwrap()).await.unwrap().unwrap();
    let delivery = &row.metadata.expect("the delivery plan is recorded at submit")["delivery"];
    assert!(delivery.get("fbx_light_units").is_none() && delivery.get("usd_light_units").is_none(), "{delivery}");

    for bad in [serde_json::json!({"fbx_light_units": "watts"}), serde_json::json!({"usd_light_units": "lumens"})] {
        let mut req = serde_json::json!({"model": "mock/mesh-3d", "prompt": "a fox"});
        req.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        let (status, body) = post_json(&app, "/v1/models3d/generations", req).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
}

/// `fbx_light_units` and `usd_light_units` are optional fields of the
/// generation request, typed `Model3dLightUnits` (`photometric` or
/// `blender`), each saying which files it applies to and whose reading
/// `photometric` is.
#[tokio::test]
async fn the_light_units_fields_are_in_the_openapi_spec() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap()).await.unwrap();
    let spec = harness::json_body(resp).await;
    let request = &spec["components"]["schemas"]["Model3dGenerationRequest"];
    let parts = request["allOf"].as_array().expect("the base request and the 3D properties");
    let enum_of = |s: &serde_json::Value| -> Option<serde_json::Value> {
        s.get("enum").cloned().or_else(|| s.get("oneOf")?.as_array()?.iter().find_map(|v| v.get("enum").cloned()))
    };
    let units = &spec["components"]["schemas"]["Model3dLightUnits"];
    assert_eq!(enum_of(units), Some(json!(["photometric", "blender"])), "{units}");
    let said = units.to_string();
    for word in ["light_units", "FBX", "USD", "Blender", "glTF"] {
        assert!(said.contains(word), "Model3dLightUnits: {word}: {said}");
    }
    // The fields promise the option where litegen reads the vendor's file or
    // writes one, not on a vendor file delivered as it is; and photometric USD
    // is UsdLux's by the schema, not "as Storm reads them" (Storm draws a
    // normalised sphere 4x brighter). Descriptions wrap over doc lines, so the
    // newlines are read as spaces.
    let fbx_words: &[&str] = &["fbx", "Intensity", "photometric", "blender", "when litegen reads the vendor's FBX or writes one for this request (a vendor FBX delivered as it is keeps the vendor's units):"];
    let usd_words: &[&str] = &[
        "usdz",
        "customData",
        "photometric",
        "blender",
        "when litegen reads the vendor's `usdz`, `usda` or `usdc` or writes one for this request (one delivered as it is keeps the vendor's units):",
        "(the default; UsdLux's own, by the schema)",
    ];
    for (name, words) in [("fbx_light_units", fbx_words), ("usd_light_units", usd_words)] {
        let field = parts.iter().find_map(|p| p["properties"].get(name)).unwrap_or_else(|| panic!("no `{name}` property: {request}"));
        assert!(!parts.iter().any(|p| p["required"].as_array().is_some_and(|r| r.iter().any(|f| f == name))), "{name} is optional: photometric is the default");
        assert!(field.to_string().contains("Model3dLightUnits"), "{field}");
        let said = field.to_string().replace("\\n", " ");
        for word in words {
            assert!(said.contains(word), "{name}: {word}: {said}");
        }
        assert!(!said.contains("Storm reads"), "{name}: {said}");
    }
    let colour = parts.iter().find_map(|p| p["properties"].get("fbx_color_space")).expect("fbx_color_space").to_string().replace("\\n", " ");
    assert!(colour.contains("when litegen reads the vendor's FBX or writes one for this request (a vendor FBX delivered as it is keeps the vendor's):"), "{colour}");
}

/// 2026-10-12 review #31: the mock answered only the containers it writes
/// itself (glb, obj, stl), so a request for fbx alone completed with no mesh
/// and failed, where a glb-only vendor is asked for its glb and litegen
/// derives the rest. The mock now answers as that vendor does.
#[tokio::test]
async fn a_container_the_mock_does_not_write_is_derived_from_its_glb() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, submitted) = post_json(
        &app,
        "/v1/models3d/generations",
        serde_json::json!({ "model": "mock/mesh-3d", "prompt": "a fox", "output_formats": ["fbx"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().unwrap().to_string();
    let mut last = serde_json::Value::Null;
    for _ in 0..20 {
        last = get_3d(&app, &id).await;
        if last["status"] == "completed" || last["status"] == "failed" {
            break;
        }
    }
    assert_eq!(last["status"], "completed", "{last}");
    let meshes: Vec<&serde_json::Value> = last["assets"].as_array().unwrap().iter().filter(|a| a["kind"] == "mesh").collect();
    assert!(meshes.iter().any(|m| m["format"] == "fbx"), "{last}");
}

/// Every glTF compression extension, as a vendor's file may use one: the
/// test's own list, the oracle for what a delivered glTF must not name.
const COMPRESSION: [&str; 3] = ["EXT_meshopt_compression", "KHR_meshopt_compression", "KHR_draco_mesh_compression"];

/// mesh-gaps item 2: `model` returns a vendor's compressed GLB of the mesh
/// library's skinned fixture, the file whose sha256 `mock_data/README.md`
/// records as `vendor_sha256`. It is delivered in every container
/// `/v1/models` says the model can deliver, each measured from litegen's
/// read of it. Every glTF litegen delivers is uncompressed: no manifest
/// names a compression extension, the delivered glb and gltf use and require
/// none, and reading the vendor's file added no note to any manifest. The
/// vendor's file, changed, is kept as the `original`.
async fn a_vendor_compressed_glb_is_delivered(model: &str, vendor_sha256: &str) {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/v1/models?media_type=model3d").body(Body::empty()).unwrap()).await.unwrap();
    let list = harness::json_body(resp).await;
    let row = list["data"].as_array().unwrap().iter().find(|m| m["id"] == model);
    assert!(row.is_some(), "{model} is listed: {list}");
    let promised: Vec<String> = serde_json::from_value(row.unwrap()["capabilities"]["output_formats"].clone()).unwrap();
    assert_eq!(promised, deliverable_formats(&["glb".to_string()]), "{model}: its glb, and every container derived from it");

    let done = generate_3d(&app, json!({ "model": model, "prompt": "a skinned leg", "output_formats": promised })).await;
    assert_eq!(done["status"], "completed", "{done}");
    let meshes = assets_of(&done, "mesh");
    let mut delivered: Vec<&str> = meshes.iter().map(|m| m["format"].as_str().unwrap()).collect();
    let mut want: Vec<&str> = promised.iter().map(String::as_str).collect();
    delivered.sort_unstable();
    want.sort_unstable();
    assert_eq!(delivered, want, "one mesh per promised container: {done}");
    for m in &meshes {
        // Measured from litegen's read of each delivered file: the vendor's
        // one primitive, 2,304 indices.
        assert_eq!(m["scene"]["triangle_count"], 768, "{m}");
        let reported: Vec<&str> = m["scene"]["extensions_used"].as_array().unwrap().iter().filter_map(|e| e.as_str()).collect();
        assert!(!reported.iter().any(|e| COMPRESSION.contains(e)), "the delivered {}'s manifest: {reported:?}", m["format"]);
        let notes = notes_of(m);
        assert!(notes.iter().all(|n| !n.starts_with("reading the provider's")), "the delivered {}: {notes:?}", m["format"]);
    }

    for format in ["glb", "gltf"] {
        let mesh = meshes.iter().find(|m| m["format"] == format).unwrap();
        let (status, _, bytes) = fetch(&app, mesh["url"].as_str().unwrap(), None).await;
        assert_eq!(status, StatusCode::OK, "{mesh}");
        let doc: serde_json::Value = if format == "glb" {
            let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
            serde_json::from_slice(&bytes[20..20 + json_len]).unwrap()
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        for key in ["extensionsUsed", "extensionsRequired"] {
            let names: Vec<&str> = doc[key].as_array().map(|a| a.iter().filter_map(|e| e.as_str()).collect()).unwrap_or_default();
            assert!(!names.iter().any(|e| COMPRESSION.contains(e)), "the delivered {format}'s {key}: {names:?}");
        }
        assert!(doc["extensionsRequired"].as_array().is_none_or(|e| e.is_empty()), "the delivered {format}: {}", doc["extensionsRequired"]);
    }
    // The skin and its clip, through the codec's decode.
    let glb = meshes.iter().find(|m| m["format"] == "glb").unwrap();
    assert_eq!(glb["scene"]["skin"]["joints"], json!(["hip", "knee", "ankle"]), "{glb}");
    assert_eq!(glb["scene"]["animations"][0]["name"], "Bend", "{glb}");

    let originals = assets_of(&done, "original");
    assert_eq!(originals.len(), 1, "the vendor's file, as for any changed file: {done}");
    assert_eq!(originals[0]["format"], "glb");
    assert_eq!(originals[0]["sha256"], vendor_sha256, "{done}");
    assert!(originals[0].get("scene").is_none(), "only delivered meshes carry a manifest: {done}");
}

/// [`a_vendor_compressed_glb_is_delivered`] for meshopt: gltfpack `-cc`'s
/// GLB, which requires EXT_meshopt_compression (`mock/meshopt-3d`'s
/// `mock_data/skinned.cc.glb`).
#[tokio::test]
async fn a_vendor_meshopt_glb_is_delivered() {
    a_vendor_compressed_glb_is_delivered("mock/meshopt-3d", "4196f2fb927f55998cbd7a8080bbb49de1280aafcaabda871e30196ce34b1627").await;
}

/// [`a_vendor_compressed_glb_is_delivered`] for Draco: glTF-Transform
/// `draco`'s GLB, which requires KHR_draco_mesh_compression
/// (`mock/draco-3d`'s `mock_data/skinned.draco.glb`): the spec's "the same
/// for a Draco GLB".
#[tokio::test]
async fn a_vendor_draco_glb_is_delivered() {
    a_vendor_compressed_glb_is_delivered("mock/draco-3d", "f7997c911589be5707caacd8043e1263c9738d5fe5c0c108d3b52a8c01c6e8f5").await;
}


// ─── output_packages: each multi-file container as a zip (D1) ───────────────

/// POST a 3D request that keeps nothing (`X-Litegen-Keep-Outputs: false`).
async fn post_keeping_nothing(app: &axum::Router, body: &serde_json::Value) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/v1/models3d/generations")
                .header("content-type", "application/json")
                .header("x-litegen-keep-outputs", "false")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, harness::json_body(resp).await)
}

/// litegen makes every package from a file it keeps, so a request that keeps
/// nothing can't ask for one: refused before the vendor is called.
#[tokio::test]
async fn a_keep_nothing_request_cannot_ask_for_packages() {
    let (app, db) = harness::app_with_mock_3d().await;
    let before = db.list_active_generations(100).await.unwrap().len();
    let body = json!({ "model": "mock/mesh-3d", "prompt": "a fox", "output_packages": true });
    let (status, refused) = post_keeping_nothing(&app, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"]["code"], "option_needs_keeping", "{refused}");
    assert_eq!(refused["error"]["param"], "output_packages", "{refused}");
    assert_eq!(db.list_active_generations(100).await.unwrap().len(), before, "no vendor job was started: {refused}");
}

/// Review Focus 4: keeping nothing without asking for packages (or asking
/// for none) is today's request, and is not refused.
#[tokio::test]
async fn keep_nothing_without_packages_is_unchanged() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for body in [
        json!({ "model": "mock/mesh-3d", "prompt": "a fox" }),
        json!({ "model": "mock/mesh-3d", "prompt": "a fox", "output_packages": false }),
    ] {
        let (status, submitted) = post_keeping_nothing(&app, &body).await;
        assert_eq!(status, StatusCode::OK, "{body}: {submitted}");
        let done = harness::poll_until_terminal(&app, submitted["id"].as_str().unwrap()).await;
        assert_eq!(done["status"], "completed", "{done}");
        let assets = done["assets"].as_array().unwrap();
        assert!(assets.iter().any(|a| a["kind"] == "mesh"), "{done}");
        assert!(assets.iter().all(|a| a["kind"] != "package" && a["kept"] == false), "the vendor's files, as they came: {done}");
    }
}

fn assets_of<'a>(done: &'a serde_json::Value, kind: &str) -> Vec<&'a serde_json::Value> {
    done["assets"].as_array().map(|a| a.iter().filter(|a| a["kind"] == kind).collect()).unwrap_or_default()
}

/// Submit, then poll the generation to its end.
async fn generate_3d(app: &axum::Router, body: serde_json::Value) -> serde_json::Value {
    let (status, submitted) = post_json(app, "/v1/models3d/generations", body).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    harness::poll_until_terminal(app, submitted["id"].as_str().unwrap()).await
}

/// `mock/textured-3d` emits the mesh library's textured fixture: a GLB with
/// a PNG base-colour map, for whatever needs a textured mesh.
#[tokio::test]
async fn the_textured_mock_delivers_a_textured_glb() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let done = generate_3d(&app, json!({ "model": "mock/textured-3d", "prompt": "a textured assembly" })).await;
    assert_eq!(done["status"], "completed", "{done}");
    let glb = assets_of(&done, "mesh").into_iter().find(|m| m["format"] == "glb").unwrap_or_else(|| panic!("{done}"));
    let maps: Vec<&str> = glb["scene"]["materials"].as_array().unwrap().iter().flat_map(|m| m["maps"].as_array().unwrap()).filter_map(|m| m.as_str()).collect();
    assert_eq!(maps, ["base_color"], "{}", glb["scene"]);
    assert_eq!(glb["scene"]["materials"].as_array().unwrap().len(), 3, "{}", glb["scene"]);
}

/// The containers that are a file and its sidecars.
const PACKAGED: [&str; 5] = ["obj", "dae", "usda", "usdc", "gltf"];

fn textured_request(packages: bool) -> serde_json::Value {
    let mut body = json!({
        "model": "mock/textured-3d",
        "prompt": "a textured assembly",
        "output_formats": ["glb", "obj", "dae", "usda", "usdc", "gltf"],
    });
    if packages {
        body["output_packages"] = json!(true);
    }
    body
}

/// GET a URL litegen handed out, on the app that minted it, as `key` if given.
async fn fetch(app: &axum::Router, url: &str, key: Option<&str>) -> (StatusCode, String, Vec<u8>) {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = &after_scheme[after_scheme.find('/').unwrap_or(after_scheme.len())..];
    let mut req = Request::get(path);
    if let Some(key) = key {
        req = req.header("authorization", format!("Bearer {key}"));
    }
    let resp = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let ct = resp.headers().get("content-type").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
    (status, ct, axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap().to_vec())
}

/// A zip's files, read by litegen's own archive reader.
fn unzip(bytes: &[u8]) -> Vec<litegen::mesh::PackageFile> {
    let a = litegen::mesh::zip::Archive::parse(bytes).expect("a zip");
    a.entries.iter().map(|e| litegen::mesh::PackageFile { path: e.name.clone(), bytes: a.read(e).unwrap() }).collect()
}

fn notes_of(asset: &serde_json::Value) -> Vec<String> {
    asset["scene"]["notes"].as_array().map(|n| n.iter().filter_map(|n| n.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// One package asset, fetched as `key`: a zip of the container and its
/// sidecars that reads back with its map, matching what the asset says.
async fn check_package(app: &axum::Router, p: &serde_json::Value, key: Option<&str>) {
    let format = p["format"].as_str().unwrap();
    let (status, ct, zip) = fetch(app, p["url"].as_str().unwrap(), key).await;
    assert_eq!(status, StatusCode::OK, "{p}");
    assert_eq!(ct, "application/zip", "{p}");
    assert_eq!(p["size_bytes"], json!(zip.len()), "{p}");
    assert_eq!(p["sha256"], json!(litegen::proxy::router::sha256_hex(&zip)), "{p}");
    let files = unzip(&zip);
    assert!(files.len() > 1, "{format}: {:?}", files.iter().map(|f| &f.path).collect::<Vec<_>>());
    let fmt = litegen::mesh::MeshFormat::parse(format).unwrap();
    let back = litegen::mesh::read_scene(&files[0].bytes, fmt, &litegen::mesh::package::MemorySidecars { files: &files }).unwrap().scene;
    assert!(back.materials.iter().any(|m| m.base_color_texture.is_some()), "{format}: the map resolves inside the zip");
}

#[tokio::test]
async fn output_packages_delivers_a_zip_per_multi_file_container() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let done = generate_3d(&app, textured_request(true)).await;
    assert_eq!(done["status"], "completed", "{done}");
    let packages = assets_of(&done, "package");
    let formats: Vec<&str> = packages.iter().map(|p| p["format"].as_str().unwrap()).collect();
    assert_eq!(formats, PACKAGED, "one per multi-file container, in request order, none for the glb: {done}");
    for p in &packages {
        let format = p["format"].as_str().unwrap();
        assert!(p["url"].as_str().unwrap().ends_with(&format!("/package_{format}.zip")), "{p}");
        assert!(p.get("kept").is_none(), "kept, so not marked otherwise: {p}");
        check_package(&app, p, None).await;
        let maps: Vec<&str> =
            p["scene"]["materials"].as_array().unwrap().iter().flat_map(|m| m["maps"].as_array().unwrap()).filter_map(|m| m.as_str()).collect();
        assert!(maps.contains(&"base_color"), "{format}: the package's own manifest lists the map: {}", p["scene"]);
        let notes = notes_of(p);
        // Neither the texture nor the materials one OBJ file bakes into its
        // vertices' colours.
        let lost = |n: &String| n.starts_with("textures:") || (n.starts_with("materials:") && n.contains("vertices' colours"));
        assert!(!notes.iter().any(lost), "{format}: {notes:?}");
    }
    // The one-file OBJ still loses what its package keeps, and says so.
    let obj = assets_of(&done, "mesh").into_iter().find(|m| m["format"] == "obj").unwrap();
    assert!(notes_of(obj).iter().any(|n| n.starts_with("textures:")), "{obj}");
    // The meshes are today's, byte for byte.
    let plain = generate_3d(&app, textured_request(false)).await;
    let meshes = |d: &serde_json::Value| -> Vec<serde_json::Value> {
        assets_of(d, "mesh").iter().map(|m| json!([m["format"], m["sha256"], m["size_bytes"], m["scene"]])).collect()
    };
    assert_eq!(meshes(&done), meshes(&plain));
    assert!(assets_of(&plain, "package").is_empty(), "{plain}");
}

const PKG_MASTER: &str = "sk-model3d-packages-master";

/// The app behind `PKG_MASTER`, its stored files handed out in `mode`.
async fn app_handing_out(mode: &str) -> axum::Router {
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, db) = harness::app_with_hooks_tweaked(&dir, Some(PKG_MASTER), Vec::new(), |c| {
        c.secrets_key = Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [7u8; 32]));
    })
    .await;
    db.put_app_file_access(&litegen::types::AppFileAccessUpsert {
        app_id: litegen::api::middleware::DEFAULT_APP_ID.into(),
        mode: Some(mode.into()),
        owner_scope: None,
        signed_url_ttl_seconds: None,
        keep_outputs: None,
        retention_days: None,
        signed_links: None,
    })
    .await
    .unwrap();
    app
}

async fn call_as(app: &axum::Router, method: &str, uri: &str, body: Option<&serde_json::Value>) -> serde_json::Value {
    let req = Request::builder().method(method).uri(uri).header("authorization", format!("Bearer {PKG_MASTER}"));
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    harness::json_body(app.clone().oneshot(req).await.unwrap()).await
}

#[tokio::test]
async fn packages_follow_the_access_mode() {
    for mode in ["signed", "private"] {
        let app = app_handing_out(mode).await;
        let id = call_as(&app, "POST", "/v1/models3d/generations", Some(&textured_request(true))).await["id"].as_str().unwrap().to_string();
        let mut done = serde_json::Value::Null;
        for _ in 0..20 {
            done = call_as(&app, "GET", &format!("/v1/models3d/{id}"), None).await;
            if done["status"] == "completed" || done["status"] == "failed" {
                break;
            }
        }
        assert_eq!(done["status"], "completed", "{mode}: {done}");
        let assets = done["assets"].as_array().unwrap();
        let packages: Vec<(usize, &serde_json::Value)> = assets.iter().enumerate().filter(|(_, a)| a["kind"] == "package").collect();
        assert_eq!(packages.len(), PACKAGED.len(), "{mode}: {done}");
        for (index, p) in packages {
            let url = p["url"].as_str().unwrap();
            match mode {
                "signed" => {
                    assert!(url.contains(&format!("/package_{}.zip?", p["format"].as_str().unwrap())) && url.contains("signature="), "{mode}: {p}");
                    assert!(p["expires_at"].is_string(), "{mode}: {p}");
                    check_package(&app, p, None).await;
                }
                _ => {
                    assert!(url.ends_with(&format!("/v1/generations/{id}/assets/{index}")), "{mode}: {p}");
                    check_package(&app, p, Some(PKG_MASTER)).await;
                    assert_eq!(fetch(&app, url, None).await.0, StatusCode::UNAUTHORIZED, "{mode}: the file route wants a key");
                }
            }
        }
    }
}

/// Only the poller sees the job finish; it delivers the packages from the
/// plan the row recorded, the same ones a client's poll delivers.
#[tokio::test]
async fn the_poller_delivers_packages() {
    let dir = tempfile::tempdir().unwrap().keep();
    let db = std::sync::Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_over(db.clone(), &dir).await;
    let (status, submitted) = post_json(&app, "/v1/models3d/generations", textured_request(true)).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().unwrap().to_string();
    for _ in 0..20 {
        litegen::proxy::poller::poll_once_presenting(&state.db, &state.router.registry, state.secrets_key, state.mode, &state.router.model3d_store, None).await;
        if db.get_generation(&id).await.unwrap().unwrap().status.is_terminal() {
            break;
        }
    }
    let row = db.get_generation(&id).await.unwrap().unwrap();
    assert_eq!(row.status, GenerationStatus::Completed, "{:?}", row.error_message);
    let by_poller = harness::poll_until_terminal(&app, &id).await;
    let by_client = generate_3d(&app, textured_request(true)).await;
    let packages = |d: &serde_json::Value| -> Vec<serde_json::Value> { assets_of(d, "package").iter().map(|p| json!([p["format"], p["sha256"]])).collect() };
    assert_eq!(packages(&by_poller).len(), PACKAGED.len(), "{by_poller}");
    assert_eq!(packages(&by_poller), packages(&by_client));
}

#[tokio::test]
async fn the_assets_list_names_packages() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let done = generate_3d(&app, textured_request(true)).await;
    let id = done["id"].as_str().unwrap();
    let resp = app.clone().oneshot(Request::get(format!("/v1/generations/{id}/assets")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = harness::json_body(resp).await;
    let kinds: Vec<(&str, &str)> = listed["data"]
        .as_array()
        .unwrap_or_else(|| panic!("{listed}"))
        .iter()
        .filter(|a| a["kind"] == "package")
        .map(|a| (a["kind"].as_str().unwrap(), a["format"].as_str().unwrap()))
        .collect();
    assert_eq!(kinds, PACKAGED.map(|f| ("package", f)), "{listed}");
}

// ─── ktx2_textures: KTX2 textures as PNG, or kept in glb and gltf (item 3) ──

/// The vendor's file `mock/basisu-3d` returns: the `textured` fixture whose
/// base colour is a KTX2 (CarConcept's `Dot_C`, ETC1S), requiring
/// `KHR_texture_basisu` (`mock_data/README.md`).
const BASISU_GLB: &[u8] = include_bytes!("../src/providers/model3d/mock_data/textured.basisu.glb");

/// The KTX2 file inside the vendor's GLB.
fn vendor_ktx2() -> Vec<u8> {
    let scene = litegen::mesh::read_scene(BASISU_GLB, litegen::mesh::MeshFormat::Glb, &litegen::mesh::NoSidecars).unwrap().scene;
    assert_eq!(scene.images[0].format, litegen::mesh::scene::ImageFormat::Ktx2);
    scene.images[0].data.clone()
}

/// The pixels that KTX2 file must come out as: the decoder's, which
/// `mesh::ktx2`'s own tests prove byte-equal to the reference transcoder's.
fn reference_rgba() -> Vec<u8> {
    litegen::mesh::ktx2::decode_rgba8(&vendor_ktx2()).unwrap().pixels
}

/// The scene of the delivered `format` mesh of `done`.
async fn delivered_scene(app: &axum::Router, done: &serde_json::Value, format: &str) -> litegen::mesh::Scene {
    let (_, bytes) = delivered_file(app, done, format).await;
    litegen::mesh::read_scene(&bytes, litegen::mesh::MeshFormat::parse(format).unwrap(), &litegen::mesh::NoSidecars)
        .unwrap_or_else(|e| panic!("the delivered {format} reads: {e}"))
        .scene
}

/// The delivered `format` mesh of `done`: its asset, and its bytes.
async fn delivered_file(app: &axum::Router, done: &serde_json::Value, format: &str) -> (serde_json::Value, Vec<u8>) {
    let asset = assets_of(done, "mesh").into_iter().find(|m| m["format"] == format).unwrap_or_else(|| panic!("no {format} in {done}")).clone();
    let (status, _, bytes) = fetch(app, asset["url"].as_str().unwrap(), None).await;
    assert_eq!(status, StatusCode::OK, "{asset}");
    (asset, bytes)
}

/// The first base-colour map of `scene`, as `(image format, image bytes)`.
fn base_colour(scene: &litegen::mesh::Scene) -> (litegen::mesh::scene::ImageFormat, Vec<u8>) {
    let map = scene.materials.iter().find_map(|m| m.base_color_texture.as_ref()).expect("a base-colour map");
    let image = &scene.images[scene.textures[map.texture].image];
    (image.format, image.data.clone())
}

fn glb_json_text(glb: &[u8]) -> String {
    let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    String::from_utf8(glb[20..20 + len].to_vec()).unwrap()
}

/// What `ktx2_textures` is asked for in these tests: every container the
/// textures matter to.
const KTX2_FORMATS: [&str; 5] = ["glb", "gltf", "fbx", "usdz", "3mf"];

fn basisu_request(extra: serde_json::Value) -> serde_json::Value {
    let mut body = json!({ "model": "mock/basisu-3d", "prompt": "a textured crate", "output_formats": KTX2_FORMATS });
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    body
}

/// A vendor's KTX2 texture arrives as PNG in every container (the pixels of
/// the reference transcoder), no manifest says one was lost, and the
/// vendor's file is the `original`, byte for byte.
#[tokio::test]
async fn a_vendors_ktx2_texture_is_delivered_as_png_in_every_container() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let done = generate_3d(&app, basisu_request(json!({}))).await;
    assert_eq!(done["status"], "completed", "{done}");
    let want = reference_rgba();
    for format in KTX2_FORMATS {
        let (asset, bytes) = delivered_file(&app, &done, format).await;
        let scene = litegen::mesh::read_scene(&bytes, litegen::mesh::MeshFormat::parse(format).unwrap(), &litegen::mesh::NoSidecars).unwrap().scene;
        let (image_format, data) = base_colour(&scene);
        assert_eq!(image_format, litegen::mesh::scene::ImageFormat::Png, "{format}");
        assert_eq!(image::load_from_memory(&data).unwrap().to_rgba8().into_raw(), want, "{format}: the reference's pixels");
        let maps: Vec<&str> = asset["scene"]["materials"].as_array().unwrap().iter().flat_map(|m| m["maps"].as_array().unwrap()).filter_map(|m| m.as_str()).collect();
        assert!(maps.contains(&"base_color"), "{format}: {}", asset["scene"]);
        assert!(notes_of(&asset).iter().all(|n| !n.contains("could not be decoded")), "{format}: {:?}", notes_of(&asset));
        if format == "glb" {
            let json = glb_json_text(&bytes);
            assert!(!json.contains("KHR_texture_basisu") && !json.contains("extensionsRequired"), "{json}");
            assert!(asset["scene"]["extensions_used"].as_array().is_some_and(|e| e.is_empty()), "{asset}");
        }
    }
    let originals = assets_of(&done, "original");
    assert_eq!(originals.len(), 1, "{done}");
    let (status, _, bytes) = fetch(&app, originals[0]["url"].as_str().unwrap(), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, BASISU_GLB, "the vendor's file, KTX2 intact");
    assert_eq!(originals[0]["sha256"], litegen::proxy::router::sha256_hex(BASISU_GLB), "{done}");
}

/// `ktx2_textures: keep`: the glb and gltf keep the vendor's KTX2 bytes and
/// require `KHR_texture_basisu`; the fbx, usdz and 3mf of the same request
/// still get PNG. The plan records the choice.
#[tokio::test]
async fn ktx2_textures_keep_leaves_glb_its_ktx2_and_other_containers_png() {
    let (app, db) = harness::app_with_mock_3d().await;
    let (status, submitted) = post_json(&app, "/v1/models3d/generations", basisu_request(json!({ "ktx2_textures": "keep" }))).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().unwrap().to_string();
    let meta = db.get_generation(&id).await.unwrap().unwrap().metadata.expect("the delivery plan is recorded at submit");
    assert_eq!(meta["delivery"]["ktx2_textures"], "keep", "{meta}");
    let done = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    let (ktx2, want) = (vendor_ktx2(), reference_rgba());
    for format in ["glb", "gltf"] {
        let (asset, bytes) = delivered_file(&app, &done, format).await;
        let text = if format == "glb" { glb_json_text(&bytes) } else { String::from_utf8(bytes.clone()).unwrap() };
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["extensionsRequired"], json!(["KHR_texture_basisu"]), "{format}");
        let scene = litegen::mesh::read_scene(&bytes, litegen::mesh::MeshFormat::parse(format).unwrap(), &litegen::mesh::NoSidecars).unwrap().scene;
        assert_eq!(base_colour(&scene), (litegen::mesh::scene::ImageFormat::Ktx2, ktx2.clone()), "{format}: the vendor's KTX2, byte for byte");
        let used: Vec<&str> = asset["scene"]["extensions_used"].as_array().unwrap().iter().filter_map(|e| e.as_str()).collect();
        assert!(used.contains(&"KHR_texture_basisu"), "{format}: the manifest names it: {asset}");
        assert!(notes_of(&asset).iter().all(|n| !n.contains("could not be decoded")), "{format}: {:?}", notes_of(&asset));
    }
    for format in ["fbx", "usdz", "3mf"] {
        let scene = delivered_scene(&app, &done, format).await;
        let (image_format, data) = base_colour(&scene);
        assert_eq!(image_format, litegen::mesh::scene::ImageFormat::Png, "{format}: every other container still gets PNG");
        assert_eq!(image::load_from_memory(&data).unwrap().to_rgba8().into_raw(), want, "{format}");
    }
    let (_, original) = {
        let originals = assets_of(&done, "original");
        assert_eq!(originals.len(), 1, "{done}");
        let (_, _, bytes) = fetch(&app, originals[0]["url"].as_str().unwrap(), None).await;
        (originals[0].clone(), bytes)
    };
    assert_eq!(original, BASISU_GLB, "the vendor's file stays beside, as for any changed file");
}

/// `png` is the default: asking for it, or for nothing, delivers the same
/// bytes, and the plan records nothing for either.
#[tokio::test]
async fn ktx2_textures_png_is_todays_output() {
    let (app, db) = harness::app_with_mock_3d().await;
    let mut delivered = Vec::new();
    for extra in [json!({}), json!({ "ktx2_textures": "png" }), json!({ "ktx2_textures": " PNG " })] {
        let (status, submitted) = post_json(&app, "/v1/models3d/generations", basisu_request(extra.clone())).await;
        assert_eq!(status, StatusCode::OK, "{extra}: {submitted}");
        let id = submitted["id"].as_str().unwrap().to_string();
        let meta = db.get_generation(&id).await.unwrap().unwrap().metadata.expect("the delivery plan is recorded at submit");
        assert!(meta["delivery"].get("ktx2_textures").is_none(), "{extra}: nothing recorded for the default: {meta}");
        let done = harness::poll_until_terminal(&app, &id).await;
        assert_eq!(done["status"], "completed", "{done}");
        delivered.push(assets_of(&done, "mesh").iter().map(|m| json!([m["format"], m["sha256"], m["size_bytes"], m["scene"]])).collect::<Vec<_>>());
    }
    assert!(delivered[0].len() >= KTX2_FORMATS.len(), "{:?}", delivered[0]);
    assert_eq!(delivered[0], delivered[1]);
    assert_eq!(delivered[0], delivered[2]);
}

/// A word `ktx2_textures` does not take is 400 `param_invalid`, naming it,
/// and `keep` from a request that keeps nothing is 400 `option_needs_keeping`;
/// neither starts a vendor job. `png` keeps nothing fine: it is today's.
#[tokio::test]
async fn ktx2_textures_refusals_start_no_job() {
    let (app, db) = harness::app_with_mock_3d().await;
    let before = db.list_active_generations(100).await.unwrap().len();
    let (status, refused) = post_json(&app, "/v1/models3d/generations", basisu_request(json!({ "ktx2_textures": "gamma" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!((&refused["error"]["code"], &refused["error"]["param"]), (&json!("param_invalid"), &json!("ktx2_textures")), "{refused}");
    // The vendor's own glb only: the derived containers would be refused
    // first, as `format_needs_keeping`.
    let (status, refused) = post_keeping_nothing(&app, &json!({ "model": "mock/basisu-3d", "prompt": "a crate", "ktx2_textures": "keep" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!((&refused["error"]["code"], &refused["error"]["param"]), (&json!("option_needs_keeping"), &json!("ktx2_textures")), "{refused}");
    assert_eq!(db.list_active_generations(100).await.unwrap().len(), before, "no vendor job was started");

    let (status, submitted) = post_keeping_nothing(&app, &json!({ "model": "mock/basisu-3d", "prompt": "a crate", "ktx2_textures": "png" })).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let done = harness::poll_until_terminal(&app, submitted["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed", "{done}");
}

/// Only the poller sees the job finish: it delivers the KTX2 choice from the
/// plan the row recorded, the same bytes a client's own poll delivers.
#[tokio::test]
async fn the_poller_honours_the_recorded_ktx2_textures() {
    let dir = tempfile::tempdir().unwrap().keep();
    let db = std::sync::Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_over(db.clone(), &dir).await;
    let request = basisu_request(json!({ "ktx2_textures": "keep" }));
    let (status, submitted) = post_json(&app, "/v1/models3d/generations", request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().unwrap().to_string();
    for _ in 0..20 {
        litegen::proxy::poller::poll_once_presenting(&state.db, &state.router.registry, state.secrets_key, state.mode, &state.router.model3d_store, None).await;
        if db.get_generation(&id).await.unwrap().unwrap().status.is_terminal() {
            break;
        }
    }
    let row = db.get_generation(&id).await.unwrap().unwrap();
    assert_eq!(row.status, GenerationStatus::Completed, "{:?}", row.error_message);
    let by_poller = harness::poll_until_terminal(&app, &id).await;
    let by_client = generate_3d(&app, request).await;
    let kept = |d: &serde_json::Value| -> Vec<serde_json::Value> { assets_of(d, "mesh").iter().map(|m| json!([m["format"], m["sha256"]])).collect() };
    assert_eq!(kept(&by_poller), kept(&by_client));
    let (_, glb) = delivered_file(&app, &by_poller, "glb").await;
    assert_eq!(base_colour(&litegen::mesh::read_scene(&glb, litegen::mesh::MeshFormat::Glb, &litegen::mesh::NoSidecars).unwrap().scene).0, litegen::mesh::scene::ImageFormat::Ktx2);
}

/// The request field as the OpenAPI document shows it: optional, an enum of
/// `png` and `keep`, described by what it does to which containers, with its
/// two refusals named on the endpoint.
#[tokio::test]
async fn the_ktx2_textures_field_is_in_the_openapi_spec() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap()).await.unwrap();
    let spec = harness::json_body(resp).await;
    let request = &spec["components"]["schemas"]["Model3dGenerationRequest"];
    // The request is the shared base plus this family's own properties.
    let parts = request["allOf"].as_array().expect("the base request and the 3D properties");
    let field = parts
        .iter()
        .find_map(|p| p["properties"].get("ktx2_textures"))
        .unwrap_or_else(|| panic!("no `ktx2_textures` property: {request}"));
    assert!(!parts.iter().any(|p| p["required"].as_array().is_some_and(|r| r.iter().any(|f| f == "ktx2_textures"))), "optional: png is the default");
    let enum_of = |s: &serde_json::Value| -> Option<serde_json::Value> {
        s.get("enum").cloned().or_else(|| s.get("oneOf")?.as_array()?.iter().find_map(|v| v.get("enum").cloned()))
    };
    let kinds = &spec["components"]["schemas"]["Model3dKtx2Textures"];
    assert_eq!(enum_of(kinds), Some(json!(["png", "keep"])), "{kinds}");
    assert!(field.to_string().contains("Model3dKtx2Textures"), "{field}");
    // An optional enum is a `oneOf` of null and the enum, which carries the docs.
    let said = field.to_string();
    for word in ["KHR_texture_basisu", "glb", "gltf", "original", "option_needs_keeping"] {
        assert!(said.contains(word), "{word}: {said}");
    }
    let refusals = spec["paths"]["/v1/models3d/generations"]["post"]["responses"]["400"]["description"].as_str().unwrap_or_default();
    assert!(refusals.contains("ktx2_textures"), "{refusals}");
}

/// `ktx2_textures: keep` with `output_packages: true`: the gltf package is
/// written from the scene that kept its KTX2, so the zip holds the vendor's
/// `.ktx2` as a file and its `.gltf` requires `KHR_texture_basisu`; without
/// `keep` the same package holds a PNG.
#[tokio::test]
async fn ktx2_textures_keep_puts_the_ktx2_in_the_gltf_package() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let package_of = |keep: bool| {
        let app = app.clone();
        async move {
            let mut extra = json!({ "output_packages": true });
            if keep {
                extra["ktx2_textures"] = json!("keep");
            }
            let done = generate_3d(&app, basisu_request(extra)).await;
            assert_eq!(done["status"], "completed", "{done}");
            let packages = assets_of(&done, "package");
            assert_eq!(packages.len(), 1, "only the gltf is more than one file: {done}");
            assert_eq!(packages[0]["format"], "gltf");
            let (status, _, zip) = fetch(&app, packages[0]["url"].as_str().unwrap(), None).await;
            assert_eq!(status, StatusCode::OK);
            (packages[0].clone(), unzip(&zip))
        }
    };
    let images = |files: &[litegen::mesh::PackageFile]| -> Vec<(String, Vec<u8>)> {
        files.iter().filter(|f| !f.path.ends_with(".gltf") && !f.path.ends_with(".bin")).map(|f| (f.path.clone(), f.bytes.clone())).collect()
    };

    let (asset, files) = package_of(true).await;
    let doc: serde_json::Value = serde_json::from_slice(&files[0].bytes).expect("the .gltf comes first");
    assert_eq!(doc["extensionsRequired"], json!(["KHR_texture_basisu"]), "{doc}");
    let kept = images(&files);
    assert_eq!(kept.len(), 1, "{:?}", files.iter().map(|f| &f.path).collect::<Vec<_>>());
    assert!(kept[0].0.ends_with(".ktx2"), "{}", kept[0].0);
    assert_eq!(kept[0].1, vendor_ktx2(), "the vendor's KTX2, byte for byte");
    let used: Vec<&str> = asset["scene"]["extensions_used"].as_array().unwrap().iter().filter_map(|e| e.as_str()).collect();
    assert!(used.contains(&"KHR_texture_basisu"), "the package's manifest names it: {asset}");

    let (_, files) = package_of(false).await;
    let plain = images(&files);
    assert_eq!(plain.len(), 1);
    assert!(plain[0].0.ends_with(".png"), "{}", plain[0].0);
    assert!(serde_json::from_slice::<serde_json::Value>(&files[0].bytes).unwrap().get("extensionsRequired").is_none());
}

// ─── dae_morphs: a Collada file's morph targets baked in, or kept (item 8) ──

/// What `dae_morphs` is asked for in these tests: the GLB the mock makes, and
/// the Collada litegen derives from it.
fn morph_request(extra: serde_json::Value) -> serde_json::Value {
    let mut body = json!({ "model": "mock/morph-3d", "prompt": "a blinking face", "output_formats": ["glb", "dae"] });
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    body
}

/// The `format` mesh of `done`, as `(sha256, bytes)`.
async fn delivered_digest(app: &axum::Router, done: &serde_json::Value, format: &str) -> (serde_json::Value, Vec<u8>) {
    let (asset, bytes) = delivered_file(app, done, format).await;
    (asset["sha256"].clone(), bytes)
}

/// `dae_morphs: keep` writes the request's Collada with `<morph>` controllers
/// and leaves the GLB as it was; the plan records the choice at submit.
#[tokio::test]
async fn dae_morphs_keep_delivers_controllers_beside_an_unchanged_glb() {
    let (app, db) = harness::app_with_mock_3d().await;
    let (status, submitted) = post_json(&app, "/v1/models3d/generations", morph_request(json!({ "dae_morphs": "keep" }))).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().unwrap().to_string();
    let meta = db.get_generation(&id).await.unwrap().unwrap().metadata.expect("the delivery plan is recorded at submit");
    assert_eq!(meta["delivery"]["dae_morphs"], "keep", "{meta}");
    let kept = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(kept["status"], "completed", "{kept}");
    let (_, kept_dae) = delivered_digest(&app, &kept, "dae").await;
    assert!(String::from_utf8_lossy(&kept_dae).contains("<morph "), "the Collada holds morph controllers");

    let baked = generate_3d(&app, morph_request(json!({}))).await;
    assert_eq!(baked["status"], "completed", "{baked}");
    let (_, baked_dae) = delivered_digest(&app, &baked, "dae").await;
    assert!(!String::from_utf8_lossy(&baked_dae).contains("<morph"), "no controllers without the option");
    assert!(kept_dae != baked_dae, "the option changes the Collada");
    let (kept_glb, _) = delivered_digest(&app, &kept, "glb").await;
    let (baked_glb, _) = delivered_digest(&app, &baked, "glb").await;
    assert_eq!(kept_glb, baked_glb, "the GLB is the same either way");
    // The manifest no longer lists the weight animation as lost.
    let (kept_asset, _) = delivered_file(&app, &kept, "dae").await;
    let (baked_asset, _) = delivered_file(&app, &baked, "dae").await;
    let morph_notes = |a: &serde_json::Value| notes_of(a).into_iter().filter(|n| n.starts_with("morph_targets")).count();
    assert_eq!(morph_notes(&kept_asset), 0, "kept: {kept_asset}");
    assert!(morph_notes(&baked_asset) > 0, "baking names the animation it drops: {baked_asset}");
}

/// `bake` is the default: asking for it, or for nothing, delivers the same
/// bytes; nothing is recorded for a request that asked nothing, so today's
/// plans are unchanged, and the choice is recorded as asked.
#[tokio::test]
async fn dae_morphs_bake_is_todays_output() {
    let (app, db) = harness::app_with_mock_3d().await;
    let mut delivered = Vec::new();
    for (extra, recorded) in [(json!({}), None), (json!({ "dae_morphs": "bake" }), Some("bake"))] {
        let (status, submitted) = post_json(&app, "/v1/models3d/generations", morph_request(extra.clone())).await;
        assert_eq!(status, StatusCode::OK, "{extra}: {submitted}");
        let id = submitted["id"].as_str().unwrap().to_string();
        let meta = db.get_generation(&id).await.unwrap().unwrap().metadata.expect("the delivery plan is recorded at submit");
        assert_eq!(meta["delivery"].get("dae_morphs").and_then(|v| v.as_str()), recorded, "{extra}: {meta}");
        let done = harness::poll_until_terminal(&app, &id).await;
        assert_eq!(done["status"], "completed", "{done}");
        delivered.push(assets_of(&done, "mesh").iter().map(|m| json!([m["format"], m["sha256"], m["size_bytes"], m["scene"]])).collect::<Vec<_>>());
    }
    assert!(delivered[0].len() >= 2, "{:?}", delivered[0]);
    assert_eq!(delivered[0], delivered[1]);
}

/// A word `dae_morphs` does not take is 400 `malformed_request` (the field is
/// typed, as `fbx_light_units` is), and `keep` from a request that keeps
/// nothing is 400 `option_needs_keeping`; neither starts a vendor job. `bake`
/// keeps nothing fine: it is today's.
#[tokio::test]
async fn dae_morphs_refusals_start_no_job() {
    let (app, db) = harness::app_with_mock_3d().await;
    let before = db.list_active_generations(100).await.unwrap().len();
    for word in ["yes", "KEEP", ""] {
        let (status, refused) = post_json(&app, "/v1/models3d/generations", morph_request(json!({ "dae_morphs": word }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{word:?}: {refused}");
        assert_eq!(refused["error"]["code"], "malformed_request", "{word:?}: {refused}");
    }
    // The vendor's own glb only: the derived containers would be refused
    // first, as `format_needs_keeping`.
    let (status, refused) = post_keeping_nothing(&app, &json!({ "model": "mock/morph-3d", "prompt": "a face", "dae_morphs": "keep" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!((&refused["error"]["code"], &refused["error"]["param"]), (&json!("option_needs_keeping"), &json!("dae_morphs")), "{refused}");
    assert_eq!(db.list_active_generations(100).await.unwrap().len(), before, "no vendor job was started");

    let (status, submitted) = post_keeping_nothing(&app, &json!({ "model": "mock/morph-3d", "prompt": "a face", "dae_morphs": "bake" })).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let done = harness::poll_until_terminal(&app, submitted["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed", "{done}");
}

/// Every 3D model takes `dae_morphs`, not only the morph mock: the plan
/// records it, and the mock's plain GLB has no targets, so `keep` writes the
/// same Collada as `bake`.
#[tokio::test]
async fn every_3d_model_accepts_dae_morphs() {
    let (app, db) = harness::app_with_mock_3d().await;
    let request = |extra: serde_json::Value| {
        let mut body = json!({ "model": "mock/mesh-3d", "prompt": "a fox", "output_formats": ["glb", "dae"] });
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        body
    };
    let (status, submitted) = post_json(&app, "/v1/models3d/generations", request(json!({ "dae_morphs": "keep" }))).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().unwrap().to_string();
    let meta = db.get_generation(&id).await.unwrap().unwrap().metadata.expect("the delivery plan is recorded at submit");
    assert_eq!(meta["delivery"]["dae_morphs"], "keep", "{meta}");
    let kept = harness::poll_until_terminal(&app, &id).await;
    let baked = generate_3d(&app, request(json!({}))).await;
    assert_eq!(kept["status"], "completed", "{kept}");
    let (kept_sha, _) = delivered_digest(&app, &kept, "dae").await;
    let (baked_sha, _) = delivered_digest(&app, &baked, "dae").await;
    assert_eq!(kept_sha, baked_sha, "a mesh with no morph targets has nothing to keep");
}

/// Only the poller sees the job finish: it delivers the Collada choice from
/// the plan the row recorded, the same bytes a client's own poll delivers.
#[tokio::test]
async fn the_poller_honours_the_recorded_dae_morphs() {
    let dir = tempfile::tempdir().unwrap().keep();
    let db = std::sync::Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_over(db.clone(), &dir).await;
    let request = morph_request(json!({ "dae_morphs": "keep" }));
    let (status, submitted) = post_json(&app, "/v1/models3d/generations", request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().unwrap().to_string();
    for _ in 0..20 {
        litegen::proxy::poller::poll_once_presenting(&state.db, &state.router.registry, state.secrets_key, state.mode, &state.router.model3d_store, None).await;
        if db.get_generation(&id).await.unwrap().unwrap().status.is_terminal() {
            break;
        }
    }
    let row = db.get_generation(&id).await.unwrap().unwrap();
    assert_eq!(row.status, GenerationStatus::Completed, "{:?}", row.error_message);
    let by_poller = harness::poll_until_terminal(&app, &id).await;
    let by_client = generate_3d(&app, request).await;
    let (poller_sha, poller_dae) = delivered_digest(&app, &by_poller, "dae").await;
    let (client_sha, _) = delivered_digest(&app, &by_client, "dae").await;
    assert_eq!(poller_sha, client_sha);
    assert!(String::from_utf8_lossy(&poller_dae).contains("<morph "), "the poller wrote the controllers the plan recorded");
}

/// The request field as the OpenAPI document shows it: optional, typed
/// `Model3dDaeMorphs` (`bake` or `keep`), described by what each word does and
/// which readers open the result, with the keep-nothing refusal named on the
/// endpoint.
#[tokio::test]
async fn the_dae_morphs_field_is_in_the_openapi_spec() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap()).await.unwrap();
    let spec = harness::json_body(resp).await;
    let request = &spec["components"]["schemas"]["Model3dGenerationRequest"];
    let parts = request["allOf"].as_array().expect("the base request and the 3D properties");
    let enum_of = |s: &serde_json::Value| -> Option<serde_json::Value> {
        s.get("enum").cloned().or_else(|| s.get("oneOf")?.as_array()?.iter().find_map(|v| v.get("enum").cloned()))
    };
    let words = &spec["components"]["schemas"]["Model3dDaeMorphs"];
    assert_eq!(enum_of(words), Some(json!(["bake", "keep"])), "{words}");
    let field = parts.iter().find_map(|p| p["properties"].get("dae_morphs")).unwrap_or_else(|| panic!("no `dae_morphs` property: {request}"));
    assert!(!parts.iter().any(|p| p["required"].as_array().is_some_and(|r| r.iter().any(|f| f == "dae_morphs"))), "optional: bake is the default");
    assert!(field.to_string().contains("Model3dDaeMorphs"), "{field}");
    // Descriptions wrap over doc lines, so the newlines are read as spaces.
    let said = format!("{field} {words}").replace("\\n", " ");
    for word in [
        "dae",
        "when litegen writes one for this request (a vendor Collada delivered as it is keeps the vendor's)",
        "`bake`",
        "drops their weight animation, which every Collada reader opens",
        "`keep`",
        "`<morph>` controllers",
        "SceneKit",
        "Xcode",
        "Godot",
        "three.js's ColladaLoader and assimp refuse the whole file",
        "option_needs_keeping",
    ] {
        assert!(said.contains(word), "{word}: {said}");
    }
    let refusals = spec["paths"]["/v1/models3d/generations"]["post"]["responses"]["400"]["description"].as_str().unwrap_or_default();
    assert!(refusals.contains("dae_morphs"), "{refusals}");
}

// ─── usdc, usdz_root and usd_clips (mesh-gaps item 6) ───────────────────────

/// `usdc` is a container a request may ask for: the crate litegen derives from
/// the mock's GLB reads back through the mesh library, its manifest says what
/// was read, and it is served as `application/octet-stream` because IANA
/// registers no media type for a binary USD crate.
#[tokio::test]
async fn usdc_output_formats_deliver_a_crate_that_reads_back() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let done = generate_3d(&app, json!({ "model": "mock/textured-3d", "prompt": "a textured assembly", "output_formats": ["glb", "usdc"] })).await;
    assert_eq!(done["status"], "completed", "{done}");
    let usdc = assets_of(&done, "mesh").into_iter().find(|m| m["format"] == "usdc").unwrap_or_else(|| panic!("no usdc in {done}"));
    let (status, content_type, bytes) = fetch(&app, usdc["url"].as_str().unwrap(), None).await;
    assert_eq!(status, StatusCode::OK, "{usdc}");
    assert_eq!(content_type, "application/octet-stream", "{usdc}");
    assert!(bytes.starts_with(b"PXR-USDC"), "a crate, not text");
    let read = litegen::mesh::read_scene(&bytes, litegen::mesh::MeshFormat::Usdc, &litegen::mesh::package::NoSidecars).unwrap_or_else(|e| panic!("the crate does not read: {e}"));
    assert_eq!(usdc["scene"]["triangle_count"], json!(read.scene.triangle_count()), "the manifest is of the crate's own read: {usdc}");
    assert!(read.scene.triangle_count() > 0);
    let formats: Vec<&str> = assets_of(&done, "mesh").iter().filter_map(|m| m["format"].as_str()).collect();
    assert_eq!(formats, ["glb", "usdc"], "the vendor's glb, then the crate: {done}");
}

/// `usdz_root: usdc` roots the USDZ litegen derives in the crate and leaves
/// the other files alone; asked or not, the plan records only what the
/// request said; and the default is today's package.
#[tokio::test]
async fn usdz_root_usdc_delivers_a_crate_rooted_usdz_and_records_the_plan() {
    let (app, db) = harness::app_with_mock_3d().await;
    let request = |extra: serde_json::Value| {
        let mut body = json!({ "model": "mock/textured-3d", "prompt": "a textured assembly", "output_formats": ["glb", "usdz"] });
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        body
    };
    let first_entry = |usdz: &[u8]| litegen::mesh::zip::Archive::parse(usdz).expect("a zip").entries[0].name.clone();
    let mut delivered = Vec::new();
    for (extra, recorded, root) in [(json!({}), None, "model.usda"), (json!({ "usdz_root": "usda" }), Some("usda"), "model.usda"), (json!({ "usdz_root": "usdc" }), Some("usdc"), "model.usdc")] {
        let (status, submitted) = post_json(&app, "/v1/models3d/generations", request(extra.clone())).await;
        assert_eq!(status, StatusCode::OK, "{extra}: {submitted}");
        let id = submitted["id"].as_str().unwrap().to_string();
        let meta = db.get_generation(&id).await.unwrap().unwrap().metadata.expect("the delivery plan is recorded at submit");
        assert_eq!(meta["delivery"].get("usdz_root").and_then(|v| v.as_str()), recorded, "{extra}: {meta}");
        let done = harness::poll_until_terminal(&app, &id).await;
        assert_eq!(done["status"], "completed", "{done}");
        let (_, usdz) = delivered_file(&app, &done, "usdz").await;
        assert_eq!(first_entry(&usdz), root, "{extra}");
        let read = litegen::mesh::read_scene(&usdz, litegen::mesh::MeshFormat::Usdz, &litegen::mesh::package::NoSidecars).unwrap_or_else(|e| panic!("{extra}: {e}"));
        assert!(read.scene.materials.iter().any(|m| m.base_color_texture.is_some()), "{extra}: the texture came with the package");
        delivered.push((usdz, delivered_digest(&app, &done, "glb").await.0));
    }
    assert!(delivered[0].0 == delivered[1].0, "usda is the default root, byte for byte");
    assert!(delivered[0].0 != delivered[2].0, "usdc changes the package");
    assert_eq!(delivered[0].1, delivered[2].1, "the glb is the same either way");
}

/// A word `usdz_root` or `usd_clips` does not take is 400 `malformed_request`
/// (typed fields, as `dae_morphs` is). From a request that keeps nothing,
/// `usdz_root: usdc` and `usd_clips: all` are 400 `option_needs_keeping`, each
/// naming itself: litegen applies them to the file it keeps. `usda` and
/// `first` ask for what a vendor file already is or is made as, so they pass.
/// No refusal starts a vendor job.
#[tokio::test]
async fn usdz_root_and_usd_clips_refusals_start_no_job() {
    let (app, db) = harness::app_with_mock_3d().await;
    let before = db.list_active_generations(100).await.unwrap().len();
    let request = |extra: serde_json::Value| {
        let mut body = json!({ "model": "mock/mesh-3d", "prompt": "a fox" });
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        body
    };
    for field in ["usdz_root", "usd_clips"] {
        for word in ["yes", "USDC", "ALL", ""] {
            let (status, refused) = post_json(&app, "/v1/models3d/generations", request(json!({ field: word }))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{field}={word:?}: {refused}");
            assert_eq!(refused["error"]["code"], "malformed_request", "{field}={word:?}: {refused}");
        }
    }
    // `usdz_root` takes usda and usdc, `usd_clips` all and first, and not each other's.
    for (field, word) in [("usdz_root", "first"), ("usd_clips", "usda")] {
        let (status, refused) = post_json(&app, "/v1/models3d/generations", request(json!({ field: word }))).await;
        assert_eq!((status, &refused["error"]["code"]), (StatusCode::BAD_REQUEST, &json!("malformed_request")), "{field}={word}: {refused}");
    }
    for (field, word) in [("usdz_root", "usdc"), ("usd_clips", "all")] {
        let (status, refused) = post_keeping_nothing(&app, &request(json!({ field: word }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}={word}: {refused}");
        assert_eq!((&refused["error"]["code"], &refused["error"]["param"]), (&json!("option_needs_keeping"), &json!(field)), "{field}={word}: {refused}");
    }
    assert_eq!(db.list_active_generations(100).await.unwrap().len(), before, "no vendor job was started");

    for (field, word) in [("usdz_root", "usda"), ("usd_clips", "first")] {
        let (status, submitted) = post_keeping_nothing(&app, &request(json!({ field: word }))).await;
        assert_eq!(status, StatusCode::OK, "{field}={word}: {submitted}");
        let done = harness::poll_until_terminal(&app, submitted["id"].as_str().unwrap()).await;
        assert_eq!(done["status"], "completed", "{field}={word}: {done}");
    }
}

/// Every 3D model takes `usdz_root` and `usd_clips`, and the plan records each
/// as asked (and nothing for a request that asked neither, so today's plans
/// are unchanged). What each choice does to the files is proved elsewhere:
/// `usd_clips` by
/// `proxy::model3d_deliver::tests::the_requested_usd_clips_decides_how_many_clips_every_derived_usd_keeps`,
/// and `usdz_root` through the poller by `the_poller_honours_the_recorded_usdz_root`.
#[tokio::test]
async fn every_3d_model_accepts_usdz_root_and_usd_clips() {
    let (app, db) = harness::app_with_mock_3d().await;
    for model in ["mock/mesh-3d", "mock/morph-3d"] {
        for (extra, usdz_root, usd_clips) in [
            (json!({}), None, None),
            (json!({ "usdz_root": "usdc", "usd_clips": "all" }), Some("usdc"), Some("all")),
            (json!({ "usdz_root": "usda", "usd_clips": "first" }), Some("usda"), Some("first")),
        ] {
            let mut body = json!({ "model": model, "prompt": "a fox", "output_formats": ["glb", "usdz", "usda", "usdc"] });
            body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            let (status, submitted) = post_json(&app, "/v1/models3d/generations", body).await;
            assert_eq!(status, StatusCode::OK, "{model} {extra}: {submitted}");
            let id = submitted["id"].as_str().unwrap().to_string();
            let meta = db.get_generation(&id).await.unwrap().unwrap().metadata.expect("the delivery plan is recorded at submit");
            let recorded = |key: &str| meta["delivery"].get(key).and_then(|v| v.as_str());
            assert_eq!((recorded("usdz_root"), recorded("usd_clips")), (usdz_root, usd_clips), "{model} {extra}: {meta}");
            let done = harness::poll_until_terminal(&app, &id).await;
            assert_eq!(done["status"], "completed", "{model} {extra}: {done}");
            let formats: Vec<&str> = assets_of(&done, "mesh").iter().filter_map(|m| m["format"].as_str()).collect();
            assert_eq!(formats, ["glb", "usdz", "usda", "usdc"], "{model} {extra}");
        }
    }
}

/// Only the poller sees the job finish: it delivers the USDZ root from the
/// plan the row recorded, the same bytes a client's own poll delivers.
#[tokio::test]
async fn the_poller_honours_the_recorded_usdz_root() {
    let dir = tempfile::tempdir().unwrap().keep();
    let db = std::sync::Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_over(db.clone(), &dir).await;
    let request = json!({ "model": "mock/textured-3d", "prompt": "a textured assembly", "output_formats": ["glb", "usdz"], "usdz_root": "usdc" });
    let (status, submitted) = post_json(&app, "/v1/models3d/generations", request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().unwrap().to_string();
    for _ in 0..20 {
        litegen::proxy::poller::poll_once_presenting(&state.db, &state.router.registry, state.secrets_key, state.mode, &state.router.model3d_store, None).await;
        if db.get_generation(&id).await.unwrap().unwrap().status.is_terminal() {
            break;
        }
    }
    let row = db.get_generation(&id).await.unwrap().unwrap();
    assert_eq!(row.status, GenerationStatus::Completed, "{:?}", row.error_message);
    let by_poller = harness::poll_until_terminal(&app, &id).await;
    let by_client = generate_3d(&app, request).await;
    let (poller_sha, poller_usdz) = delivered_digest(&app, &by_poller, "usdz").await;
    let (client_sha, _) = delivered_digest(&app, &by_client, "usdz").await;
    assert_eq!(poller_sha, client_sha);
    let first = litegen::mesh::zip::Archive::parse(&poller_usdz).expect("a zip").entries[0].name.clone();
    assert_eq!(first, "model.usdc", "the poller wrote the crate root the plan recorded");
}

/// The request fields as the OpenAPI document shows them: optional, typed by
/// their own enums, described by what each word does, with the keep-nothing
/// refusal named on the endpoint, and `usdc` among the formats.
#[tokio::test]
async fn the_usdz_root_and_usd_clips_fields_are_in_the_openapi_spec() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap()).await.unwrap();
    let spec = harness::json_body(resp).await;
    let request = &spec["components"]["schemas"]["Model3dGenerationRequest"];
    let parts = request["allOf"].as_array().expect("the base request and the 3D properties");
    let enum_of = |s: &serde_json::Value| -> Option<serde_json::Value> {
        s.get("enum").cloned().or_else(|| s.get("oneOf")?.as_array()?.iter().find_map(|v| v.get("enum").cloned()))
    };
    for (field, schema, words, said_words) in [
        (
            "usdz_root",
            "Model3dUsdzRoot",
            json!(["usda", "usdc"]),
            vec![
                "(a vendor USDZ delivered as it is keeps the vendor's)",
                "`usda` (the default)",
                "`usdchecker --arkit`",
                "`<model-viewer>`",
                "`usdc` is the binary crate",
                "as Apple's own packagers write it",
                "64-byte aligned",
                "option_needs_keeping",
            ],
        ),
        (
            "usd_clips",
            "Model3dUsdClips",
            json!(["all", "first"]),
            vec![
                "(a vendor USD delivered as it is keeps the vendor's)",
                "`all` keeps every clip as an alternative",
                "variant set `Animation`",
                "`first` keeps the first",
                "`usda` and `usdc`",
                "AR Quick Look",
                "option_needs_keeping",
            ],
        ),
    ] {
        let schema_json = &spec["components"]["schemas"][schema];
        assert_eq!(enum_of(schema_json), Some(words), "{schema_json}");
        let property = parts.iter().find_map(|p| p["properties"].get(field)).unwrap_or_else(|| panic!("no `{field}` property: {request}"));
        assert!(!parts.iter().any(|p| p["required"].as_array().is_some_and(|r| r.iter().any(|f| f == field))), "{field} is optional");
        assert!(property.to_string().contains(schema), "{property}");
        let said = format!("{property} {schema_json}").replace("\\n", " ");
        for word in said_words {
            assert!(said.contains(word), "{field}: {word}: {said}");
        }
    }
    let formats = &spec["components"]["schemas"]["Model3dFormat"];
    assert!(formats.to_string().contains("usdc"), "{formats}");
    let refusals = spec["paths"]["/v1/models3d/generations"]["post"]["responses"]["400"]["description"].as_str().unwrap_or_default();
    assert!(refusals.contains("usdz_root") && refusals.contains("usd_clips"), "{refusals}");
}
