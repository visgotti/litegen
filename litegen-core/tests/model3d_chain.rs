//! PixelGenie 3D doc §3J, end to end: a base64 still becomes a mesh, the mesh
//! is rigged by its generation id, and the rig is animated with an in-place
//! `walk` and `idle` — with a restart of litegen between every step, and every
//! step re-sent with its `Idempotency-Key` costing nothing.
//!
//! The final GLB is read twice: by litegen's own reader, and — when
//! `scripts/mesh-verify` has its node dependencies (`npm ci` there) — by
//! `@gltf-transform/core`, a reader litegen does not share code with.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use base64::Engine;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + sqlite

/// litegen over a database file and an asset directory that outlive it: a
/// new call is a restarted process.
struct Deployment {
    dir: tempfile::TempDir,
}

impl Deployment {
    async fn boot(&self) -> (axum::Router, Arc<litegen::db::sqlite::SqliteDatabase>) {
        let url = format!("sqlite://{}/litegen.db?mode=rwc", self.dir.path().display());
        let db = Arc::new(litegen::db::sqlite::SqliteDatabase::connect(&url).await.expect("file sqlite"));
        let assets = self.dir.path().join("assets");
        (harness::app_over(db.clone(), &assets, |_| {}).await, db)
    }
}

async fn post(app: &axum::Router, key: &str, body: &Value) -> (StatusCode, HeaderMap, Value) {
    let req = Request::post("/v1/models3d/generations")
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = axum::body::to_bytes(resp.into_body(), 32 * 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn get_bytes(app: &axum::Router, path: &str) -> Vec<u8> {
    let resp = app.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{path}");
    axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap().to_vec()
}

/// Run one step: submit with `key`, poll to completion, then re-send the same
/// request with the same key — which must answer the same id and bill nothing.
async fn step(app: &axum::Router, db: &litegen::db::sqlite::SqliteDatabase, key: &str, body: Value) -> Value {
    use litegen::db::DatabaseStore;
    let (status, _, sub) = post(app, key, &body).await;
    assert_eq!(status, StatusCode::OK, "{key}: {sub}");
    let id = sub["id"].as_str().unwrap().to_string();
    let done = harness::poll_until_terminal(app, &id).await;
    assert_eq!(done["status"], "completed", "{key}: {done}");

    let (rows, logs) = (db.count_generations(None).await.unwrap(), db.get_request_logs(1, 1000).await.unwrap().0.len());
    let (status, headers, again) = post(app, key, &body).await;
    assert_eq!(status, StatusCode::OK, "{key} re-sent: {again}");
    assert_eq!(again["id"], id.as_str(), "{key}: the same generation");
    assert_eq!(headers.get("idempotent-replayed").and_then(|v| v.to_str().ok()), Some("true"), "{key}");
    assert_eq!(db.count_generations(None).await.unwrap(), rows, "{key}: no second job");
    assert_eq!(db.get_request_logs(1, 1000).await.unwrap().0.len(), logs, "{key}: nothing billed again");
    done
}

#[tokio::test]
async fn a_still_becomes_an_animated_rig_through_restarts_and_retries() {
    let deployment = Deployment { dir: tempfile::tempdir().unwrap() };
    let png = base64::engine::general_purpose::STANDARD.encode(litegen::providers::image::visual_mock::generate_visual_image_png("a knight"));

    // 1. A base64 still → a mesh.
    let (app, db) = deployment.boot().await;
    let mesh = step(&app, &db, "chain-mesh", json!({
        "model": "mock/mesh-3d",
        "prompt": "a knight",
        "reference_images": [{ "type": "base64", "value": png, "role": "init" }],
    })).await;
    drop(app);

    // 2. A restart, then the mesh rigged by its generation id.
    let (app, db) = deployment.boot().await;
    let rig = step(&app, &db, "chain-rig", json!({
        "model": "mock/rig-3d",
        "source_model": { "type": "generation", "value": mesh["id"] },
    })).await;
    drop(app);

    // 3. Another restart, then the rig animated: an in-place walk and idle.
    let (app, db) = deployment.boot().await;
    let animated = step(&app, &db, "chain-animate", json!({
        "model": "mock/animate-3d",
        "source_model": { "type": "generation", "value": rig["id"] },
        "animations": [
            { "name": "walk", "preset": "walk", "in_place": true, "loop": true },
            { "name": "idle", "preset": "idle", "in_place": true, "loop": true },
        ],
    })).await;

    // The manifest: a humanoid map, both clips held in place, facing +Z.
    let glb_asset = animated["assets"].as_array().unwrap().iter()
        .find(|a| a["kind"] == "mesh" && a["format"] == "glb")
        .unwrap_or_else(|| panic!("no glb: {animated}"))
        .clone();
    let scene = &glb_asset["scene"];
    assert!(scene["skin"]["humanoid"].as_object().is_some_and(|h| h.len() >= 7), "{scene}");
    assert_eq!((scene["front"].as_str(), scene["up"].as_str()), (Some("+z"), Some("+y")), "{scene}");
    let clips = scene["animations"].as_array().unwrap();
    assert_eq!(clips.iter().map(|c| c["name"].as_str().unwrap()).collect::<Vec<_>>(), ["walk", "idle"]);
    for c in clips {
        let t = &c["root_travel_m"];
        let horizontal = (t[0].as_f64().unwrap().powi(2) + t[2].as_f64().unwrap().powi(2)).sqrt();
        assert!(c["in_place"] == true && horizontal < 0.01, "{c}");
    }
    assert!(scene["extensions_used"].as_array().unwrap().is_empty(), "{scene}");

    // The file itself, through a restart: litegen's reader…
    drop(app);
    let (app, _db) = deployment.boot().await;
    let url = glb_asset["url"].as_str().unwrap();
    let bytes = get_bytes(&app, &format!("/v1/{}", url.split_once("/v1/").unwrap().1)).await;
    let read = litegen::mesh::read_scene(&bytes, litegen::mesh::MeshFormat::Glb, &litegen::mesh::package::NoSidecars).unwrap().scene;
    let names: Vec<Option<String>> = read.animations.iter().map(|a| a.name.clone()).collect();
    assert_eq!(names, [Some("walk".to_string()), Some("idle".to_string())]);

    // …and an independent one.
    let mut verify = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    verify.pop();
    verify.push("scripts/mesh-verify");
    if !verify.join("node_modules/@gltf-transform/core").exists() {
        eprintln!("skipped the @gltf-transform/core read: run `npm ci` in scripts/mesh-verify");
        return;
    }
    let file = deployment.dir.path().join("chain.glb");
    std::fs::write(&file, &bytes).unwrap();
    let out = std::process::Command::new("node")
        .arg("read-gltf.mjs")
        .arg(&file)
        .current_dir(&verify)
        .output()
        .expect("node runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let seen: Value = serde_json::from_slice(&out.stdout).unwrap();
    let anims = seen["animations"].as_array().unwrap();
    assert_eq!(anims.iter().map(|a| a["name"].as_str().unwrap()).collect::<Vec<_>>(), ["walk", "idle"], "{seen}");
    assert!(anims.iter().all(|a| (a["duration_s"].as_f64().unwrap() - 1.0).abs() < 1e-3), "{seen}");
    assert_eq!(seen["joints"].as_array().unwrap().len(), 7, "{seen}");
    assert_eq!(seen["extensionsRequired"], json!([]), "{seen}");
}
