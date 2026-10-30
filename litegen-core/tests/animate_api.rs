//! Animation (PixelGenie 3D doc §3C, §3I): an `animate` model adds one clip
//! per requested `name` to a rigged mesh, in request order. litegen names the
//! clips, keeps `in_place` clips in place when the vendor did not, keys them
//! at `fps`, and measures every clip into the manifest from its keys.

use axum::body::Body;
use axum::http::{Request, StatusCode};
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

async fn finished(app: &axum::Router, body: Value) -> Value {
    let (status, sub) = post(app, body).await;
    assert_eq!(status, StatusCode::OK, "{sub}");
    let done = harness::poll_until_terminal(app, sub["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed", "{done}");
    done
}

/// A finished mock rig to animate: mesh-3d, then rig-3d.
async fn rigged(app: &axum::Router) -> String {
    let mesh = finished(app, json!({ "model": "mock/mesh-3d", "prompt": "a knight" })).await;
    let rig = finished(app, json!({ "model": "mock/rig-3d", "source_model": { "type": "generation", "value": mesh["id"] } })).await;
    rig["id"].as_str().unwrap().to_string()
}

async fn animate(app: &axum::Router, source: &str, extra: Value) -> Value {
    let mut body = json!({ "model": "mock/animate-3d", "source_model": { "type": "generation", "value": source } });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    finished(app, body).await
}

fn glb_mesh(done: &Value) -> Value {
    done["assets"].as_array().unwrap().iter()
        .find(|a| a["kind"] == "mesh" && a["format"] == "glb")
        .unwrap_or_else(|| panic!("no glb: {done}"))
        .clone()
}

fn clips(done: &Value) -> Vec<Value> {
    glb_mesh(done)["scene"]["animations"].as_array().unwrap().clone()
}

fn horizontal_travel(clip: &Value) -> f64 {
    let t = &clip["root_travel_m"];
    (t[0].as_f64().unwrap().powi(2) + t[2].as_f64().unwrap().powi(2)).sqrt()
}

#[tokio::test]
async fn one_clip_per_name_in_request_order_kept_in_place_by_the_vendor() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = rigged(&app).await;
    let done = animate(&app, &src, json!({ "animations": [
        { "name": "walk", "preset": "walk", "in_place": true, "loop": true },
        { "name": "idle", "preset": "mock:idle", "in_place": true },
    ] })).await;
    let c = clips(&done);
    assert_eq!(c.iter().map(|c| c["name"].as_str().unwrap()).collect::<Vec<_>>(), ["walk", "idle"], "{c:?}");
    for clip in &c {
        assert!((clip["duration_s"].as_f64().unwrap() - 1.0).abs() < 1e-4, "{clip}");
        assert_eq!((clip["in_place"].as_bool(), clip["in_place_by"].as_str()), (Some(true), Some("vendor")), "{clip}");
        assert!(horizontal_travel(clip) < 0.01, "{clip}");
    }
    assert_eq!(c[0]["loop_requested"], true);
    assert!(c[1].get("loop_requested").is_none(), "{}", c[1]);
    // The rig the clips play on came through.
    assert_eq!(glb_mesh(&done)["scene"]["skin"]["root"], "hips");
}

#[tokio::test]
async fn a_travelling_clip_moves_one_metre_along_z_and_litegen_holds_the_in_place_one() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = rigged(&app).await;
    let done = animate(&app, &src, json!({ "animations": [
        { "name": "walk", "preset": "walk", "in_place": true },
        { "name": "run", "preset": "run", "in_place": false },
    ] })).await;
    let c = clips(&done);
    assert_eq!((c[0]["in_place"].as_bool(), c[0]["in_place_by"].as_str()), (Some(true), Some("litegen")), "{}", c[0]);
    assert!(horizontal_travel(&c[0]) < 0.01, "{}", c[0]);
    assert_eq!(c[1]["in_place"], false, "{}", c[1]);
    let t = &c[1]["root_travel_m"];
    assert!((t[2].as_f64().unwrap() - 1.0).abs() < 1e-3 && t[0].as_f64().unwrap().abs() < 1e-3, "{}", c[1]);
    assert!(done["assets"].as_array().unwrap().iter().any(|a| a["kind"] == "original"), "litegen changed the file, so the vendor's is kept");
}

#[tokio::test]
async fn clips_are_keyed_at_the_requested_fps() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = rigged(&app).await;
    let done = animate(&app, &src, json!({ "fps": 24, "animations": [{ "name": "walk", "preset": "walk", "in_place": true }] })).await;
    let url = glb_mesh(&done)["url"].as_str().unwrap().to_string();
    let path = url.split_once("/v1/").map(|(_, p)| format!("/v1/{p}")).unwrap();
    let resp = app.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    let scene = litegen::mesh::read_scene(&bytes, litegen::mesh::MeshFormat::Glb, &litegen::mesh::package::NoSidecars).unwrap().scene;
    let channel = &scene.animations[0].channels[0];
    assert_eq!(channel.times.len(), 25, "1 s at 24 fps is 25 keys: {:?}", channel.times);
    assert!((channel.times[1] - 1.0 / 24.0).abs() < 1e-5);
}

#[tokio::test]
async fn a_prompt_clip_lasts_the_duration_asked() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = rigged(&app).await;
    let done = animate(&app, &src, json!({ "animations": [
        { "name": "cheer", "prompt": "raises both fists and cheers", "duration_s": 3, "in_place": true },
    ] })).await;
    let c = clips(&done);
    assert!((c[0]["duration_s"].as_f64().unwrap() - 3.0).abs() < 1e-4, "{}", c[0]);
}

#[tokio::test]
async fn the_animated_mesh_faces_where_its_source_did() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = rigged(&app).await;
    let done = animate(&app, &src, json!({ "animations": [{ "name": "walk", "preset": "walk", "in_place": true }] })).await;
    let scene = &glb_mesh(&done)["scene"];
    assert_eq!((scene["front"].as_str(), scene["oriented_by"].as_str()), (Some("+z"), Some("vendor")), "{scene}");
}

#[tokio::test]
async fn an_unrigged_source_is_refused_before_anything_is_billed() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let mesh = finished(&app, json!({ "model": "mock/mesh-3d", "prompt": "a crate" })).await;
    let (status, body) = post(&app, json!({
        "model": "mock/animate-3d",
        "source_model": { "type": "generation", "value": mesh["id"] },
        "animations": [{ "name": "walk", "preset": "walk" }],
    })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "source_not_rigged", "{body}");
    assert_eq!(db.count_generations(None).await.unwrap(), 1, "only the source");
}
