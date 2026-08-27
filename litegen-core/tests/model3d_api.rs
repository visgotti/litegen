//! End-to-end HTTP coverage for the `model3d` family, against the mock provider.
//! Mirrors the aipix acceptance checklist item-for-item.

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

use axum::body::Body;
use axum::http::{Request, StatusCode};
use litegen::db::DatabaseStore;
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
