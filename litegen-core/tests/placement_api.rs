//! Opt-in placement (PixelGenie 3D doc §3D): `ground` (lowest point at
//! y = 0), `center` (bounds centred on x and z) and `height_m`, which litegen
//! applies to any mesh it delivers as one root-node transform, so skins and
//! clips are unchanged.

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

/// `POST /v1/models3d/cost`: validation only, no vendor is called.
async fn quote(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(Request::post("/v1/models3d/cost").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn get(app: &axum::Router, uri: &str) -> Value {
    let resp = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{uri}");
    serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap()).unwrap()
}

async fn finished(app: &axum::Router, body: Value) -> Value {
    let (status, sub) = post(app, body).await;
    assert_eq!(status, StatusCode::OK, "{sub}");
    let done = harness::poll_until_terminal(app, sub["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed", "{done}");
    done
}

fn manifest(done: &Value) -> Value {
    done["assets"].as_array().unwrap().iter()
        .find(|a| a["kind"] == "mesh" && a["format"] == "glb")
        .unwrap_or_else(|| panic!("no glb mesh: {done}"))["scene"]
        .clone()
}

fn bounds(scene: &Value) -> ([f64; 3], [f64; 3]) {
    let v = |k: &str, i: usize| scene["bounds"][k][i].as_f64().unwrap();
    ([v("min", 0), v("min", 1), v("min", 2)], [v("max", 0), v("max", 1), v("max", 2)])
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-4
}

/// The fixture cube floating off-centre, moved by (3, 5, -1).
fn offset_cube() -> String {
    use litegen::mesh::scene::Transform;
    let mut scene = litegen::mesh::fixtures::cube();
    for root in scene.roots.clone() {
        scene.nodes[root].transform = Transform::Trs { translation: [3.0, 5.0, -1.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] };
    }
    base64::engine::general_purpose::STANDARD.encode(litegen::mesh::glb::write_scene(&scene).unwrap())
}

#[tokio::test]
async fn ground_center_and_height_place_the_mesh_and_keep_the_vendors_file() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let source = json!({ "type": "base64", "value": offset_cube(), "format": "glb" });
    let as_is = manifest(&finished(&app, json!({ "model": "mock/texture-3d", "prompt": "x", "source_model": source.clone() })).await);
    let (lo, hi) = bounds(&as_is);
    assert!(lo[1] > 1.0 && (lo[0] + hi[0]) / 2.0 > 2.0, "the fixture floats off-centre: {as_is}");

    let done = finished(&app, json!({
        "model": "mock/texture-3d", "prompt": "x", "source_model": source,
        "ground": true, "center": true, "height_m": 2.5,
    })).await;
    let placed = manifest(&done);
    let (lo, hi) = bounds(&placed);
    assert!(close(lo[1], 0.0), "lowest point at y = 0: {placed}");
    assert!(close(hi[1] - lo[1], 2.5), "height_m tall: {placed}");
    assert!(close((lo[0] + hi[0]) / 2.0, 0.0) && close((lo[2] + hi[2]) / 2.0, 0.0), "centred on x and z: {placed}");
    assert_eq!(placed["triangle_count"], as_is["triangle_count"]);
    assert!(done["assets"].as_array().unwrap().iter().any(|a| a["kind"] == "original"), "the vendor's own file is kept: {done}");
}

#[tokio::test]
async fn height_m_scales_a_rigged_animated_mesh_without_touching_its_skin_or_clips() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let mesh = finished(&app, json!({ "model": "mock/mesh-3d", "prompt": "a knight" })).await;
    let rig = finished(&app, json!({ "model": "mock/rig-3d", "source_model": { "type": "generation", "value": mesh["id"] } })).await;
    let clips = json!([{ "name": "walk", "preset": "walk", "in_place": true }, { "name": "idle", "preset": "idle" }]);
    let plain = manifest(&finished(&app, json!({ "model": "mock/animate-3d", "source_model": { "type": "generation", "value": rig["id"] }, "animations": clips.clone() })).await);
    let tall = manifest(&finished(&app, json!({
        "model": "mock/animate-3d", "source_model": { "type": "generation", "value": rig["id"] }, "animations": clips, "height_m": 3.5,
    })).await);
    let ((lo, hi), (plo, phi)) = (bounds(&tall), bounds(&plain));
    assert!(close(hi[1] - lo[1], 3.5), "{tall}");
    assert!(close(lo[1], plo[1] * 3.5 / (phi[1] - plo[1])), "scaled about the origin, not moved: {tall}");
    assert_eq!(tall["skin"]["joints"], plain["skin"]["joints"]);
    assert_eq!(tall["skin"]["humanoid"], plain["skin"]["humanoid"]);
    for (a, b) in tall["animations"].as_array().unwrap().iter().zip(plain["animations"].as_array().unwrap()) {
        assert_eq!((&a["name"], &a["in_place"]), (&b["name"], &b["in_place"]));
        assert!(close(a["duration_s"].as_f64().unwrap(), b["duration_s"].as_f64().unwrap()), "{a} {b}");
    }
}

#[tokio::test]
async fn placement_is_offered_on_every_3d_model_and_a_height_must_be_one() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    // mock/mesh-3d declares no height_m: litegen places the mesh itself.
    let done = finished(&app, json!({ "model": "mock/mesh-3d", "prompt": "a crate", "height_m": 3.0 })).await;
    let (lo, hi) = bounds(&manifest(&done));
    assert!(close(hi[1] - lo[1], 3.0), "{done}");

    let logged = db.get_request_logs(1, 100).await.unwrap().0.len();
    for h in [0.0, -2.0, 20_000.0] {
        let (s, resp) = post(&app, json!({ "model": "mock/mesh-3d", "prompt": "a crate", "height_m": h })).await;
        assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("param_out_of_range")), "{h}: {resp}");
    }
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), logged, "nothing billed");

    let resp = app.clone().oneshot(Request::get("/v1/models?media_type=model3d").body(Body::empty()).unwrap()).await.unwrap();
    let list: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap()).unwrap();
    for m in list["data"].as_array().unwrap() {
        assert_eq!(m["capabilities"]["supports_placement"], true, "{}", m["id"]);
    }
}

/// E11a: the placement fields are litegen's own, so `GET /v1/models/{id}`
/// serves them in `litegen_params` with the bounds the validator keeps. A
/// client renders them from there and holds no copy of the rule or the caps.
#[tokio::test]
async fn every_3d_model_serves_its_placement_fields_with_the_bounds_the_validator_keeps() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let mesh = get(&app, "/v1/models/mock/mesh-3d").await;
    let lp = &mesh["litegen_params"];
    for f in ["ground", "center"] {
        assert_eq!((lp[f]["kind"].as_str(), lp[f]["default"].as_bool()), (Some("bool"), Some(false)), "{f}: {mesh}");
        assert!(mesh["params"].get(f).is_none(), "{f} is litegen's, not the vendor's: {mesh}");
    }
    let height = &lp["height_m"];
    assert_eq!((height["kind"].as_str(), &height["min"]), (Some("float"), &Value::Null), "above 0 is no inclusive minimum: {height}");
    assert!(mesh["params"].get("height_m").is_none(), "{mesh}");
    let max = height["max"].as_f64().unwrap_or_else(|| panic!("a ceiling: {height}"));
    // The served ceiling is the validator's.
    let (s, resp) = quote(&app, json!({ "model": "mock/mesh-3d", "prompt": "a crate", "height_m": max })).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    let (s, resp) = quote(&app, json!({ "model": "mock/mesh-3d", "prompt": "a crate", "height_m": max + 1.0 })).await;
    assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("param_out_of_range")), "{resp}");
    // Every 3D model the listing says is placed serves the fields.
    let list = get(&app, "/v1/models?media_type=model3d").await;
    for m in list["data"].as_array().unwrap() {
        let id = m["id"].as_str().unwrap();
        let served = get(&app, &format!("/v1/models/{id}")).await;
        assert_eq!(served["litegen_params"].get("ground").is_some(), m["capabilities"]["supports_placement"] == true, "{id}: {served}");
    }
}

/// E11a: a row that declares its own `height_m` without a ceiling is held to
/// litegen's (the validator's `unwrap_or`), so that is the ceiling it is
/// served with; the field stays in `params`, once.
#[tokio::test]
async fn a_declared_height_without_a_ceiling_is_served_with_the_ceiling_the_validator_keeps() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for id in ["meshy/rig", "fal/meshy-rig"] {
        let rig = get(&app, &format!("/v1/models/{id}")).await;
        let max = rig["params"]["height_m"]["max"].as_f64();
        assert_eq!(max, Some(10_000.0), "{id}: {}", rig["params"]["height_m"]);
        assert!(rig["litegen_params"].get("height_m").is_none(), "{id}: declared once, in params: {}", rig["litegen_params"]);
        let body = |h: f64| json!({ "model": id, "source_model": { "type": "generation", "value": "litegen-3d-x" }, "height_m": h });
        let (s, resp) = quote(&app, body(max.unwrap() + 1.0)).await;
        assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("param_out_of_range")), "{id}: {resp}");
        // At the ceiling it passes validation; with no vendor key the quote
        // then stops at the provider.
        let (_, resp) = quote(&app, body(max.unwrap())).await;
        assert_ne!(resp["error"]["code"].as_str(), Some("param_out_of_range"), "{id}: {resp}");
    }
}
