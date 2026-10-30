//! Rigging (PixelGenie 3D doc §3B, §3I): a `rig` model turns a mesh into a
//! skinned one. `skeleton`, `joint_naming` and `height_m` say how; the
//! vendor's free preflight refuses a mesh it cannot rig before anything is
//! billed; the delivered mesh's manifest describes the skin it got.

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

async fn finished(app: &axum::Router, body: Value) -> Value {
    let (status, sub) = post(app, body).await;
    assert_eq!(status, StatusCode::OK, "{sub}");
    let done = harness::poll_until_terminal(app, sub["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed", "{done}");
    done
}

/// A finished mock mesh to rig.
async fn source(app: &axum::Router) -> String {
    finished(app, json!({ "model": "mock/mesh-3d", "prompt": "a knight" })).await["id"].as_str().unwrap().to_string()
}

fn skin(done: &Value) -> Value {
    done["assets"].as_array().unwrap().iter()
        .find(|a| a["kind"] == "mesh" && a["format"] == "glb")
        .unwrap_or_else(|| panic!("no glb mesh: {done}"))["scene"]["skin"].clone()
}

#[tokio::test]
async fn the_mock_rig_reports_its_skeleton_exactly() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let done = finished(&app, json!({ "model": "mock/rig-3d", "source_model": { "type": "generation", "value": src } })).await;
    let skin = skin(&done);
    assert_eq!(skin["joints"], json!(["hips", "torso", "head", "arm_l", "arm_r", "leg_l", "leg_r"]), "{skin}");
    assert_eq!((skin["joint_count"].as_u64(), skin["root"].as_str(), skin["bind_pose"].as_str(), skin["skeleton"].as_str()),
        (Some(7), Some("hips"), Some("t"), Some("humanoid")), "{skin}");
    assert_eq!(skin["humanoid"], json!({
        "hips": "hips", "spine": "torso", "head": "head",
        "leftUpperArm": "arm_l", "rightUpperArm": "arm_r",
        "leftUpperLeg": "leg_l", "rightUpperLeg": "leg_r",
    }), "{skin}");
}

#[tokio::test]
async fn vrm_joint_naming_delivers_vrm_bone_names() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let done = finished(&app, json!({
        "model": "mock/rig-3d", "joint_naming": "vrm",
        "source_model": { "type": "generation", "value": src },
    })).await;
    let skin = skin(&done);
    let vrm = ["hips", "spine", "head", "leftUpperArm", "rightUpperArm", "leftUpperLeg", "rightUpperLeg"];
    assert_eq!(skin["joints"], json!(vrm), "{skin}");
    for bone in vrm {
        assert_eq!(skin["humanoid"][bone], bone, "{skin}");
    }
}

#[tokio::test]
async fn height_m_sets_the_rigged_characters_height() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let done = finished(&app, json!({
        "model": "mock/rig-3d", "height_m": 1.4,
        "source_model": { "type": "generation", "value": src },
    })).await;
    let mesh = done["assets"].as_array().unwrap().iter().find(|a| a["kind"] == "mesh").unwrap().clone();
    let top = mesh["scene"]["bounds"]["max"][1].as_f64().unwrap();
    assert!((top - 1.4).abs() < 1e-3, "{mesh}");
}

/// Two triangles in the ground plane: nothing a rig could stand up.
fn flat_quad_obj() -> String {
    "v -1 0 -1\nv 1 0 -1\nv 1 0 1\nv -1 0 1\nf 1 2 3\nf 1 3 4\n".to_string()
}

#[tokio::test]
async fn a_source_the_vendor_cannot_rig_is_refused_before_anything_is_billed() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let obj = base64::engine::general_purpose::STANDARD.encode(flat_quad_obj());
    let (status, body) = post(&app, json!({
        "model": "mock/rig-3d",
        "source_model": { "type": "base64", "value": obj, "format": "obj" },
    })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "not_riggable", "{body}");
    assert_eq!(body["error"]["param"], "source_model", "{body}");
    assert_eq!(db.get_request_logs(1, 10).await.unwrap().0.len(), 0, "nothing ran, nothing billed");
    assert_eq!(db.count_generations(None).await.unwrap(), 0);
}

/// 3D doc §3B (P2): where the vendor cannot name joints in a scheme, litegen
/// renames them itself, from the humanoid table it reads the rig with. The
/// mock names VRM bones itself; Unity and Mixamo names are litegen's.
#[tokio::test]
async fn unity_and_mixamo_naming_are_litegens_renames_of_the_vendors_joints() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let rig = |naming: &str| json!({ "model": "mock/rig-3d", "joint_naming": naming, "source_model": { "type": "generation", "value": src } });
    let unity = finished(&app, rig("unity")).await;
    let named = skin(&unity);
    assert_eq!(named["joints"], json!(["Hips", "Spine", "Head", "LeftUpperArm", "RightUpperArm", "LeftUpperLeg", "RightUpperLeg"]), "{named}");
    assert_eq!((named["humanoid"]["spine"].as_str(), named["humanoid"]["leftUpperLeg"].as_str()), (Some("Spine"), Some("LeftUpperLeg")), "{named}");
    assert!(unity["assets"].as_array().unwrap().iter().any(|a| a["kind"] == "original"), "the vendor's own names kept in its file: {unity}");

    let mixamo = skin(&finished(&app, rig("mixamo")).await);
    assert_eq!(mixamo["joints"][1], "mixamorig:Spine", "{mixamo}");
    assert_eq!(mixamo["humanoid"]["leftUpperArm"], "mixamorig:LeftArm", "{mixamo}");
}
