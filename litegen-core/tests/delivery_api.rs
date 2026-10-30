//! Normalised delivery (PixelGenie 3D doc §2 rule 3, §3D): every delivered
//! mesh is in glTF's axes and units, needs no glTF extension to read, and
//! carries a manifest litegen measured from its bytes. A vendor file litegen
//! changed is kept beside it as an `original` asset.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use sha2::Digest;
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn get(app: &axum::Router, path: &str) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    (status, axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap().to_vec())
}

async fn finished(app: &axum::Router, body: Value) -> Value {
    let resp = harness::submit_with(app, body).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let id = harness::json_body(resp).await["id"].as_str().unwrap().to_string();
    harness::poll_until_terminal(app, &id).await
}

fn glb_json(glb: &[u8]) -> Value {
    let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&glb[20..20 + len]).unwrap()
}

#[tokio::test]
async fn a_turned_vendor_mesh_is_delivered_facing_z_with_the_vendors_file_kept() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let done = finished(&app, json!({ "model": "mock/turned-3d", "prompt": "a knight", "output_formats": ["glb", "obj"] })).await;
    assert_eq!(done["status"], "completed", "{done}");
    let assets = done["assets"].as_array().unwrap();
    let meshes: Vec<&Value> = assets.iter().filter(|a| a["kind"] == "mesh").collect();
    assert_eq!(meshes.iter().map(|m| m["format"].as_str().unwrap()).collect::<Vec<_>>(), ["glb", "obj"], "{done}");
    for m in &meshes {
        let s = &m["scene"];
        assert_eq!((s["units"].as_str(), s["up"].as_str(), s["front"].as_str(), s["oriented_by"].as_str()),
            (Some("m"), Some("+y"), Some("+z"), Some("litegen")), "{m}");
        // The nose is on the front: +Z once delivered.
        assert!(s["bounds"]["max"][2].as_f64().unwrap() > 0.15, "{m}");
    }
    // Measured from the delivered bytes: the glb keeps its skin, the obj
    // (which cannot carry one) reports none.
    assert_eq!(meshes[0]["scene"]["skin"]["root"], "hips", "{}", meshes[0]);
    assert!(meshes[1]["scene"].get("skin").is_none(), "{}", meshes[1]);
    let original: Vec<&Value> = assets.iter().filter(|a| a["kind"] == "original").collect();
    assert_eq!(original.len(), 1, "the vendor's own file: {done}");
    assert_eq!(original[0]["format"], "glb");
    assert!(original[0].get("scene").is_none(), "only delivered meshes carry a manifest: {done}");
}

#[tokio::test]
async fn every_asset_hash_matches_the_bytes_litegen_serves() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let done = finished(&app, json!({ "model": "mock/turned-3d", "prompt": "a knight", "output_formats": ["glb", "obj"] })).await;
    let id = done["id"].as_str().unwrap();
    let (status, list) = get(&app, &format!("/v1/generations/{id}/assets")).await;
    assert_eq!(status, StatusCode::OK);
    let list: Value = serde_json::from_slice(&list).unwrap();
    let data = list["data"].as_array().unwrap();
    assert!(data.iter().any(|a| a["kind"] == "original"), "{list}");
    for (i, a) in data.iter().enumerate() {
        let (s, bytes) = get(&app, &format!("/v1/generations/{id}/assets/{i}")).await;
        assert_eq!(s, StatusCode::OK, "{a}");
        assert_eq!(a["sha256"].as_str().unwrap(), hex::encode(sha2::Sha256::digest(&bytes)), "{a}");
        if a["kind"] == "mesh" && a["format"] == "glb" {
            let doc = glb_json(&bytes);
            assert!(doc.get("extensionsRequired").and_then(|e| e.as_array()).is_none_or(|e| e.is_empty()), "{doc}");
        }
    }
}

#[tokio::test]
async fn a_model_whose_orientation_is_unproven_says_so() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let done = finished(&app, json!({ "model": "mock/mesh-3d", "prompt": "a crate" })).await;
    let mesh = done["assets"].as_array().unwrap().iter().find(|a| a["kind"] == "mesh").unwrap().clone();
    let s = &mesh["scene"];
    assert_eq!(s["front"], "unverified", "{mesh}");
    assert_eq!(s["oriented_by"], "vendor", "{mesh}");
    assert_eq!((s["vertex_count"].as_u64(), s["triangle_count"].as_u64()), (Some(8), Some(12)), "{mesh}");
    assert_eq!(s["extensions_used"], json!([]), "{mesh}");
    assert!(!done["assets"].as_array().unwrap().iter().any(|a| a["kind"] == "original"), "nothing changed: {done}");
}

#[tokio::test]
async fn the_catalog_says_which_models_have_a_proven_orientation() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (_, turned) = get(&app, "/v1/models/mock/turned-3d").await;
    let turned: Value = serde_json::from_slice(&turned).unwrap();
    assert_eq!(turned["native_orientation"]["up"], "+y", "{turned}");
    assert_eq!(turned["native_orientation"]["front"], "+x", "{turned}");
    assert!(turned["native_orientation"]["evidence"].as_str().is_some_and(|e| !e.is_empty()), "{turned}");
    let (_, cube) = get(&app, "/v1/models/mock/mesh-3d").await;
    let cube: Value = serde_json::from_slice(&cube).unwrap();
    assert!(cube.get("native_orientation").is_none(), "unproven: {cube}");
}

#[tokio::test]
async fn a_mesh_litegen_cannot_read_is_never_delivered() {
    // P-R7: even when it is the only container asked for.
    let (app, _db) = harness::app_with_mock_3d().await;
    let done = finished(&app, json!({ "model": "mock/partial-3d", "prompt": "a fox", "output_formats": ["glb"] })).await;
    assert_eq!(done["status"], "failed", "{done}");
    assert!(done["error"].as_str().is_some_and(|e| e.contains("could not be read")), "{done}");
}
