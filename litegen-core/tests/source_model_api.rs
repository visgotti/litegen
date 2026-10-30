//! `source_model` (PixelGenie 3D doc §3A): an operation names the mesh it
//! works on by generation id, URL or base64; litegen reads it, refuses what the
//! vendor would refuse before anything is billed, converts it to a container
//! the vendor takes, and records the lineage on the operation's row.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn post(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(Request::post("/v1/models3d/generations").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn finished_mesh(app: &axum::Router) -> String {
    let (status, sub) = post(app, json!({ "model": "mock/mesh-3d", "prompt": "a crate" })).await;
    assert_eq!(status, StatusCode::OK, "{sub}");
    let id = sub["id"].as_str().unwrap().to_string();
    let done = harness::poll_until_terminal(app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    id
}

#[tokio::test]
async fn a_rig_chained_by_generation_id_records_its_lineage() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let source = finished_mesh(&app).await;
    let source_row = db.get_generation(&source).await.unwrap().unwrap();
    let source_sha = source_row.metadata.as_ref().unwrap()["assets"].as_array().unwrap().iter()
        .find(|a| a["kind"] == "mesh" && a["format"] == "glb").unwrap()["sha256"].clone();

    let (status, sub) = post(&app, json!({ "model": "mock/rig-3d", "source_model": { "type": "generation", "value": source } })).await;
    assert_eq!(status, StatusCode::OK, "{sub}");
    let id = sub["id"].as_str().unwrap().to_string();
    let done = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");

    let row = db.get_generation(&id).await.unwrap().unwrap();
    let src = &row.metadata.unwrap()["source"];
    assert_eq!(src["generation_id"], source, "{src}");
    assert_eq!(src["sha256"], source_sha, "{src}");
    assert_eq!(src["format"], "glb", "{src}");
    // Same vendor, same account: the vendor's own task id, not a re-upload.
    assert_eq!(src["delivery"], "vendor_task", "{src}");
}

#[tokio::test]
async fn a_source_another_org_owns_is_not_found() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    db.insert_generation("litegen-3d-theirs", None, "mock/mesh-3d", "mock", "model3d", Some("j"), 0.0, Some("another-org"), Some("another-app")).await.unwrap();
    let (status, body) = post(&app, json!({ "model": "mock/rig-3d", "source_model": { "type": "generation", "value": "litegen-3d-theirs" } })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "source_not_found", "{body}");
    assert_eq!(body["error"]["param"], "source_model", "{body}");
}

#[tokio::test]
async fn a_source_still_running_is_not_ready() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (_, sub) = post(&app, json!({ "model": "mock/mesh-3d", "prompt": "a crate" })).await;
    let (status, body) = post(&app, json!({ "model": "mock/rig-3d", "source_model": { "type": "generation", "value": sub["id"] } })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "source_not_ready", "{body}");
}

/// An OBJ of `n` separate triangles.
fn obj_with_triangles(n: usize) -> String {
    let mut s = String::with_capacity(n * 60);
    for i in 0..n {
        let x = i as f32 * 0.001;
        s.push_str(&format!("v {x} 0 0\nv {x} 1 0\nv {x} 0 1\n"));
    }
    for i in 0..n {
        let b = i * 3 + 1;
        s.push_str(&format!("f {} {} {}\n", b, b + 1, b + 2));
    }
    s
}

#[tokio::test]
async fn a_refused_source_bills_nothing() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let obj = base64::engine::general_purpose::STANDARD.encode(obj_with_triangles(20_001));
    let (status, body) = post(&app, json!({ "model": "mock/rig-3d", "source_model": { "type": "base64", "value": obj, "format": "obj" } })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "source_too_many_faces", "{body}");
    assert_eq!(db.get_request_logs(1, 10).await.unwrap().0.len(), 0, "nothing ran, nothing billed");
    assert_eq!(db.count_generations(None).await.unwrap(), 0);
}

#[tokio::test]
async fn a_base64_source_in_another_container_reaches_a_glb_only_model_as_glb() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let cube = litegen::providers::model3d::glb::generate_cube_glb("a crate");
    let fbx = litegen::mesh::convert(&cube, litegen::mesh::MeshFormat::Glb, litegen::mesh::MeshFormat::Fbx).unwrap();
    let (status, sub) = post(&app, json!({
        "model": "mock/rig-3d",
        "source_model": { "type": "base64", "value": base64::engine::general_purpose::STANDARD.encode(&fbx), "format": "fbx" },
    })).await;
    assert_eq!(status, StatusCode::OK, "{sub}");
    let row = db.get_generation(sub["id"].as_str().unwrap()).await.unwrap().unwrap();
    let src = &row.metadata.unwrap()["source"];
    assert_eq!(src["received_format"], "fbx", "{src}");
    assert_eq!(src["format"], "glb", "converted for the vendor: {src}");
    assert_eq!(src["delivery"], "data_uri", "{src}");
    assert!(src["generation_id"].is_null(), "{src}");
}

// ─── reference_images by generation id (3D doc §2 rule 2) ────────────────────

async fn post_to(app: &axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(Request::post(path).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_stored_image_is_a_reference_by_its_generation_id() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (_, img) = post_to(&app, "/v1/images/generations", json!({ "model": "mock/image-gen", "prompt": "a hero", "response_format": "b64_json" })).await;
    let image_id = img["id"].as_str().unwrap();
    let as_ref = json!([{ "type": "generation", "value": image_id, "role": "init" }]);

    let (s, body) = post_to(&app, "/v1/images/generations", json!({ "model": "mock/base64-refs-image", "prompt": "the hero, waving", "reference_images": as_ref })).await;
    assert_eq!(s, StatusCode::OK, "an image edit from a stored image: {body}");
    let (s, body) = post(&app, json!({ "model": "mock/mesh-3d", "prompt": "the hero in 3D", "reference_images": as_ref })).await;
    assert_eq!(s, StatusCode::OK, "image-to-3D from a stored image: {body}");
}

#[tokio::test]
async fn an_image_reference_another_org_owns_is_not_found() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    db.insert_generation("litegen-img-theirs", None, "mock/image-gen", "mock", "image", None, 0.0, Some("another-org"), Some("another-app")).await.unwrap();
    let (s, body) = post_to(&app, "/v1/images/generations", json!({
        "model": "mock/base64-refs-image", "prompt": "x",
        "reference_images": [{ "type": "generation", "value": "litegen-img-theirs", "role": "init" }],
    })).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "source_not_found", "{body}");
    assert_eq!(body["error"]["param"], "reference_images", "{body}");
}

#[tokio::test]
async fn a_mesh_is_not_an_image_reference() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let mesh = finished_mesh(&app).await;
    let (s, body) = post_to(&app, "/v1/images/generations", json!({
        "model": "mock/base64-refs-image", "prompt": "x",
        "reference_images": [{ "type": "generation", "value": mesh, "role": "init" }],
    })).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "source_not_found", "{body}");
}
