//! Texture, remesh and segment (PixelGenie 3D doc §3F): operations on a mesh
//! submitted like any 3D job, each delivered with what litegen measured — the
//! texture maps, the triangle count, and the parts.

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

/// A finished mock cube to work on.
async fn source(app: &axum::Router) -> Value {
    finished(app, json!({ "model": "mock/mesh-3d", "prompt": "a crate" })).await
}

fn glb(done: &Value) -> Value {
    done["assets"].as_array().unwrap().iter()
        .find(|a| a["kind"] == "mesh" && a["format"] == "glb")
        .unwrap_or_else(|| panic!("no glb mesh: {done}"))
        .clone()
}

fn on(src: &Value) -> Value {
    json!({ "type": "generation", "value": src["id"] })
}

async fn delivered_scene(app: &axum::Router, asset: &Value) -> litegen::mesh::scene::Scene {
    let url = asset["url"].as_str().unwrap();
    let path = url.find("/v1/").map(|i| &url[i..]).unwrap_or(url);
    let resp = app.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    litegen::mesh::read_scene(&bytes, litegen::mesh::MeshFormat::Glb, &litegen::mesh::package::NoSidecars).unwrap().scene
}

#[tokio::test]
async fn texturing_paints_the_source_and_the_manifest_names_its_maps() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let triangles = glb(&src)["scene"]["triangle_count"].clone();
    let done = finished(&app, json!({
        "model": "mock/texture-3d", "source_model": on(&src), "prompt": "rusty iron", "pbr": true, "texture_resolution": 512,
    })).await;
    let mesh = glb(&done);
    let scene = &mesh["scene"];
    assert_eq!(scene["materials"][0]["maps"], json!(["base_color", "metallic_roughness", "normal"]), "{scene}");
    assert_eq!(scene["triangle_count"], triangles, "the source's geometry, painted");
    assert!(scene["uv_sets"].as_u64().unwrap() >= 1, "{scene}");
    let delivered = delivered_scene(&app, &mesh).await;
    let base = delivered.materials[0].base_color_texture.as_ref().expect("a base colour map");
    let png = &delivered.images[delivered.textures[base.texture].image].data;
    assert_eq!(image::load_from_memory(png).unwrap().width(), 512, "texture_resolution is the map's size");

    let plain = finished(&app, json!({ "model": "mock/texture-3d", "source_model": on(&src), "prompt": "oak" })).await;
    assert_eq!(glb(&plain)["scene"]["materials"][0]["maps"], json!(["base_color"]));
}

#[tokio::test]
async fn a_texture_request_says_what_to_paint_and_asks_a_size_the_model_makes() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let logged = db.get_request_logs(1, 100).await.unwrap().0.len();
    for (body, code) in [
        (json!({ "model": "mock/texture-3d", "source_model": on(&src) }), "param_required"),
        (json!({ "model": "mock/texture-3d", "source_model": on(&src), "prompt": "x", "texture_resolution": 333 }), "param_enum_mismatch"),
        (json!({ "model": "mock/remesh-3d", "source_model": on(&src), "keep_uvs": true }), "param_unsupported"),
    ] {
        let (s, resp) = post(&app, body.clone()).await;
        assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{body}: {resp}");
    }
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), logged, "nothing billed");
}

#[tokio::test]
async fn remeshing_delivers_the_asked_triangle_count() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let tri = finished(&app, json!({ "model": "mock/remesh-3d", "source_model": on(&src), "target_polycount": 500 })).await;
    assert_eq!(glb(&tri)["scene"]["triangle_count"], 500);
    // Quads reach glTF as pairs of triangles: 250 quads.
    let quad = finished(&app, json!({ "model": "mock/remesh-3d", "source_model": on(&src), "target_polycount": 501, "topology": "quad" })).await;
    assert_eq!(glb(&quad)["scene"]["triangle_count"], 500);
}

#[tokio::test]
async fn segmenting_splits_the_source_into_named_parts() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let triangles = glb(&src)["scene"]["triangle_count"].as_u64().unwrap();
    let done = finished(&app, json!({ "model": "mock/segment-3d", "source_model": on(&src) })).await;
    let mesh = glb(&done);
    let parts = mesh["scene"]["parts"].as_array().unwrap_or_else(|| panic!("no parts: {mesh}")).clone();
    let names: Vec<&str> = parts.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["negative_x", "part_1"], "the vendor's label, else part_<n>: {parts:?}");
    let counts: Vec<u64> = parts.iter().map(|p| p["triangle_count"].as_u64().unwrap()).collect();
    assert_eq!(counts.iter().sum::<u64>(), triangles, "{parts:?}");
    assert!(counts.iter().all(|c| *c > 0), "{parts:?}");
    // Each part's bounds are its own, measured: inside the whole mesh's.
    let whole = &glb(&src)["scene"]["bounds"];
    for p in &parts {
        for k in 0..3 {
            assert!(p["bounds"]["min"][k].as_f64().unwrap() >= whole["min"][k].as_f64().unwrap() - 1e-5, "{parts:?}");
            assert!(p["bounds"]["max"][k].as_f64().unwrap() <= whole["max"][k].as_f64().unwrap() + 1e-5, "{parts:?}");
        }
    }

    let delivered = delivered_scene(&app, &mesh).await;
    for p in &parts {
        let node = &delivered.nodes[p["node"].as_u64().unwrap() as usize];
        assert_eq!(node.name.as_deref(), p["name"].as_str(), "one glTF node per part, named so");
        assert!(node.mesh.is_some());
    }
    assert!(glb(&src)["scene"].get("parts").is_none(), "only a segment model's manifest has parts");
}

#[tokio::test]
async fn the_listing_says_what_each_operation_takes() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/v1/models?media_type=model3d").body(Body::empty()).unwrap()).await.unwrap();
    let list: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap()).unwrap();
    let caps = |id: &str| list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id}"))["capabilities"].clone();
    let texture = caps("mock/texture-3d");
    assert_eq!(
        (texture["task"].as_str(), texture["texture_resolutions"].clone(), texture["supports_keep_uvs"].as_bool(), texture["supports_pbr"].as_bool()),
        (Some("texture"), json!([512, 1024, 2048]), Some(true), Some(true)),
        "{texture}"
    );
    assert_eq!(caps("mock/remesh-3d")["task"], "remesh");
    assert_eq!(caps("mock/segment-3d")["task"], "segment");
    assert_eq!((caps("mock/mesh-3d")["texture_resolutions"].clone(), caps("mock/mesh-3d")["supports_keep_uvs"].as_bool()), (json!([]), Some(false)));
}
