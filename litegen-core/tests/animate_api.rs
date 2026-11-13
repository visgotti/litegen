//! Animation (PixelGenie 3D doc §3C, §3I): an `animate` model adds one clip
//! per requested `name` to a rigged mesh, in request order. litegen names the
//! clips, keeps `in_place` clips in place when the vendor did not, keys them
//! at `fps`, and measures every clip into the manifest from its keys. An
//! animation of a rig delivered in a joint naming is delivered in it too
//! (§3B).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use litegen::providers::model3d::humanoid::{MOCK_JOINTS, VRM_JOINTS};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

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

async fn post(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    post_to(app, "/v1/models3d/generations", body).await
}

async fn finished(app: &axum::Router, body: Value) -> Value {
    let (status, sub) = post(app, body).await;
    assert_eq!(status, StatusCode::OK, "{sub}");
    let done = harness::poll_until_terminal(app, sub["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed", "{done}");
    done
}

fn id(generation: &Value) -> &str {
    generation["id"].as_str().unwrap()
}

/// A finished mock rig: mesh-3d, then rig-3d delivered in `joint_naming`
/// (`None` names none).
async fn named_rig(app: &axum::Router, joint_naming: Option<&str>) -> Value {
    let mesh = finished(app, json!({ "model": "mock/mesh-3d", "prompt": "a knight" })).await;
    let mut body = json!({ "model": "mock/rig-3d", "source_model": { "type": "generation", "value": mesh["id"] } });
    if let Some(naming) = joint_naming {
        body["joint_naming"] = naming.into();
    }
    finished(app, body).await
}

/// A finished mock rig to animate: mesh-3d, then rig-3d.
async fn rigged(app: &axum::Router) -> String {
    id(&named_rig(app, None).await).to_string()
}

/// `mock/animate-3d` of the generation `source`, with `extra`'s fields added
/// (a `model` among them replaces it).
fn animation(source: &str, extra: Value) -> Value {
    let mut body = json!({ "model": "mock/animate-3d", "source_model": { "type": "generation", "value": source } });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    body
}

async fn animate(app: &axum::Router, source: &str, extra: Value) -> Value {
    finished(app, animation(source, extra)).await
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

/// The glb mesh asset's file, read back with litegen's own reader.
async fn glb_scene(app: &axum::Router, done: &Value) -> litegen::mesh::scene::Scene {
    let url = glb_mesh(done)["url"].as_str().unwrap().to_string();
    let path = url.split_once("/v1/").map(|(_, p)| format!("/v1/{p}")).unwrap();
    let resp = app.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    litegen::mesh::read_scene(&bytes, litegen::mesh::MeshFormat::Glb, &litegen::mesh::package::NoSidecars).unwrap().scene
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

/// E11c: a clip that leaves `name` out (or sends null) is quoted and made,
/// and delivered under the name litegen gives it.
#[tokio::test]
async fn an_unnamed_clip_is_delivered_under_the_name_litegen_gives_it() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = rigged(&app).await;
    let asked = json!({ "animations": [
        { "preset": "mock:walk" },
        { "name": null, "preset": "walk" },
        { "prompt": "raises both fists and cheers", "duration_s": 2.0 },
        { "name": "idle", "preset": "mock:idle" },
    ] });
    let (s, quote) = post_to(&app, "/v1/models3d/cost", animation(&src, asked.clone())).await;
    assert_eq!(s, StatusCode::OK, "{quote}");
    let done = animate(&app, &src, asked).await;
    assert_eq!(clips(&done).iter().map(|c| c["name"].as_str().unwrap()).collect::<Vec<_>>(), ["walk", "walk-2", "clip-3", "idle"]);
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
    let scene = glb_scene(&app, &done).await;
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

/// 3D doc §7: `supports_animation` means "takes `animations`", whatever the
/// task — the evidence PixelGenie reads for `model3d.animate` — and §2 rule
/// 5 serves the rig params as flags too.
#[tokio::test]
async fn the_listing_says_which_models_animate_and_what_a_rig_takes() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/v1/models?media_type=model3d").body(Body::empty()).unwrap()).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    let list: Value = serde_json::from_slice(&bytes).unwrap();
    let caps = |id: &str| -> Value {
        list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id} listed"))["capabilities"].clone()
    };
    let (animate, rig, mesh) = (caps("mock/animate-3d"), caps("mock/rig-3d"), caps("mock/mesh-3d"));
    assert_eq!((animate["supports_animation"].as_bool(), mesh["supports_animation"].as_bool()), (Some(true), Some(false)), "{animate}");
    assert_eq!((rig["supports_skeleton"].as_bool(), rig["supports_joint_naming"].as_bool()), (Some(true), Some(true)), "{rig}");
    assert_eq!((mesh["supports_skeleton"].as_bool(), mesh["supports_joint_naming"].as_bool()), (Some(false), Some(false)), "{mesh}");
}

// ─── Joint names across a chain (3D doc §3B) ────────────────────────────────
//
// Chained by a rig's task id, a vendor animates the rig it stored, with its
// own joint names (Tripo and Meshy do; so does the mock), not the copy
// litegen delivered and may have renamed. So an animation that names no
// `joint_naming` is delivered in the naming its rig was delivered in.

/// The mock rig's joints under Unity's names: litegen's renames of the mock's.
const UNITY: [&str; 7] = ["Hips", "Spine", "Head", "LeftUpperArm", "RightUpperArm", "LeftUpperLeg", "RightUpperLeg"];

/// The joint names in the glb mesh asset's file, in skin order.
async fn file_joints(app: &axum::Router, done: &Value) -> Vec<String> {
    let scene = glb_scene(app, done).await;
    let skin = scene.skins.first().expect("a skinned mesh");
    skin.joints.iter().map(|&j| scene.nodes[j].name.clone().unwrap_or_default()).collect()
}

/// A finished mock rig delivered in Unity's joint names.
async fn unity_rig(app: &axum::Router) -> Value {
    let rig = named_rig(app, Some("unity")).await;
    assert_eq!(file_joints(app, &rig).await, UNITY, "the rig as delivered");
    rig
}

/// One clip: a walk, in place.
fn walk() -> Value {
    json!({ "animations": [{ "name": "walk", "preset": "walk", "in_place": true }] })
}

/// A generation's row metadata, as recorded.
async fn row_metadata(db: &litegen::db::SqliteDatabase, generation: &str) -> Value {
    use litegen::db::DatabaseStore;
    db.get_generation(generation).await.unwrap().unwrap().metadata.unwrap()
}

/// What the delivered meshes' manifests note about joint names.
fn naming_notes(done: &Value) -> Vec<String> {
    done["assets"].as_array().unwrap().iter()
        .filter(|a| a["kind"] == "mesh")
        .flat_map(|a| a["scene"]["notes"].as_array().cloned().unwrap_or_default())
        .filter_map(|n| n.as_str().map(str::to_string))
        .filter(|n| n.contains("names") || n.contains("naming"))
        .collect()
}

#[tokio::test]
async fn an_animation_chained_from_a_named_rig_keeps_its_joint_names() {
    let (app, db) = harness::app_with_mock_3d().await;
    let rig = unity_rig(&app).await;
    let done = animate(&app, id(&rig), walk()).await;
    let meta = row_metadata(&db, id(&done)).await;
    assert_eq!(meta["source"]["delivery"], "vendor_task", "the vendor animated the rig it stored: {meta}");
    assert_eq!(file_joints(&app, &done).await, file_joints(&app, &rig).await, "the rig's names, in order");
    let (skin, rig_skin) = (&glb_mesh(&done)["scene"]["skin"], &glb_mesh(&rig)["scene"]["skin"]);
    assert_eq!(rig_skin["joints"], json!(UNITY), "{rig_skin}");
    assert_eq!(skin["joints"], rig_skin["joints"], "{skin}");
    assert_eq!(skin["humanoid"], rig_skin["humanoid"], "{skin}");
    assert_eq!(meta["delivery"]["joint_naming"], "unity", "{meta}");
    assert_eq!(meta["source"]["joint_naming"], "unity", "{meta}");
}

/// An animation of an animation animates its rig (`source_model_api`), and
/// is delivered in that rig's naming.
#[tokio::test]
async fn a_chain_of_animations_keeps_the_rigs_names() {
    let (app, db) = harness::app_with_mock_3d().await;
    let rig = unity_rig(&app).await;
    let first = animate(&app, id(&rig), walk()).await;
    let second = animate(&app, id(&first), walk()).await;
    assert_eq!(file_joints(&app, &second).await, file_joints(&app, &rig).await);
    let meta = row_metadata(&db, id(&second)).await;
    assert_eq!(meta["source"]["generation_id"], id(&first), "{meta}");
    assert_eq!(meta["source"]["joint_naming"], "unity", "the naming of the rig the chain climbs to: {meta}");
}

/// The naming is a fact about the rig, whichever provider made it: a rig of
/// another provider reaches this one without its task id, and is still
/// animated in its naming.
#[tokio::test]
async fn a_rig_from_another_provider_still_passes_its_naming_on() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/litegen.db?mode=rwc", dir.path().display());
    let db = Arc::new(litegen::db::SqliteDatabase::connect(&url).await.unwrap());
    let app = harness::app_over(db.clone(), dir.path(), |_| {}).await;
    let rig = unity_rig(&app).await;
    // As if Tripo had made it, through a second connection to the same file.
    let raw = sqlx::SqlitePool::connect(&url).await.unwrap();
    sqlx::query("UPDATE generations SET provider = 'tripo' WHERE id = ?").bind(id(&rig)).execute(&raw).await.unwrap();
    let done = animate(&app, id(&rig), walk()).await;
    let meta = row_metadata(&db, id(&done)).await;
    assert_eq!(meta["source"]["delivery"], "data_uri", "no task id across providers: {meta}");
    assert_eq!(meta["source"]["joint_naming"], "unity", "{meta}");
    assert_eq!(meta["delivery"]["joint_naming"], "unity", "{meta}");
    assert_eq!(file_joints(&app, &done).await, file_joints(&app, &rig).await);
}

/// A rig delivered in the vendor's names, by default or asked (`vendor`),
/// has no naming to pass on.
#[tokio::test]
async fn a_rig_delivered_with_vendor_names_passes_nothing_on() {
    let (app, db) = harness::app_with_mock_3d().await;
    for naming in [None, Some("vendor")] {
        let rig = named_rig(&app, naming).await;
        let done = animate(&app, id(&rig), walk()).await;
        assert_eq!(file_joints(&app, &done).await, MOCK_JOINTS, "{naming:?}");
        let meta = row_metadata(&db, id(&done)).await;
        assert_eq!(meta["source"]["generation_id"], id(&rig), "{naming:?}: {meta}");
        assert!(meta["delivery"]["joint_naming"].is_null(), "{naming:?}: {meta}");
        assert!(meta["source"].get("joint_naming").is_none(), "{naming:?}: {meta}");
        assert!(naming_notes(&done).is_empty(), "{naming:?}: {:?}", naming_notes(&done));
    }
}

/// The mock names a VRM rig itself, as Tripo's `spec` does, so the rig it
/// stored already has VRM names, and so does its animation.
#[tokio::test]
async fn a_vrm_rig_named_by_the_vendor_stays_vrm() {
    let (app, db) = harness::app_with_mock_3d().await;
    let rig = named_rig(&app, Some("vrm")).await;
    let done = animate(&app, id(&rig), walk()).await;
    let meta = row_metadata(&db, id(&done)).await;
    assert_eq!(meta["delivery"]["joint_naming"], "vrm", "{meta}");
    assert_eq!(meta["source"]["joint_naming"], "vrm", "{meta}");
    let joints = file_joints(&app, &done).await;
    assert_eq!(joints, VRM_JOINTS);
    assert_eq!(joints, file_joints(&app, &rig).await);
    assert!(naming_notes(&done).is_empty(), "{:?}", naming_notes(&done));
}

/// The poller delivers from the plan the row recorded at submit, so a chain
/// no client polls is delivered in its rig's naming too.
#[tokio::test]
async fn the_poller_delivers_the_inherited_naming() {
    use litegen::db::DatabaseStore;
    let dir = tempfile::tempdir().unwrap().keep();
    let db = Arc::new(litegen::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_over(db.clone(), &dir).await;
    let rig = unity_rig(&app).await;
    let (status, sub) = post(&app, animation(id(&rig), walk())).await;
    assert_eq!(status, StatusCode::OK, "{sub}");
    let animated = id(&sub).to_string();
    // Only the poller sees the job finish: the client reads it once the row
    // is terminal, and a terminal row is answered from the row.
    for _ in 0..20 {
        litegen::proxy::poller::poll_once_presenting(
            &state.db,
            &state.router.registry,
            state.secrets_key,
            state.mode,
            &state.router.model3d_store,
            None,
        )
        .await;
        if db.get_generation(&animated).await.unwrap().unwrap().status.is_terminal() {
            break;
        }
    }
    let row = db.get_generation(&animated).await.unwrap().unwrap();
    assert_eq!(row.status, litegen::types::GenerationStatus::Completed, "{:?}", row.error_message);
    let done = harness::poll_until_terminal(&app, &animated).await;
    assert_eq!(file_joints(&app, &done).await, file_joints(&app, &rig).await);
}

/// A rig row that recorded no plan (one from before plans were), or one
/// that does not read, passes nothing on, and the animation goes ahead.
#[tokio::test]
async fn a_chain_from_a_row_without_a_plan_inherits_nothing() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    // A label, and the damage it does to the rig row's metadata.
    type Damage = (&'static str, fn(&mut Value));
    let damage: [Damage; 3] = [
        ("no plan", |m: &mut Value| {
            m.as_object_mut().unwrap().remove("delivery");
        }),
        ("a plan that is a string", |m: &mut Value| m["delivery"] = json!("not a plan")),
        ("a naming that is a number", |m: &mut Value| m["delivery"]["joint_naming"] = json!(7)),
    ];
    for (what, edit) in damage {
        let rig = unity_rig(&app).await;
        let mut recorded = row_metadata(&db, id(&rig)).await;
        edit(&mut recorded);
        db.update_generation_metadata(id(&rig), &recorded).await.unwrap();
        let done = animate(&app, id(&rig), walk()).await;
        assert_eq!(file_joints(&app, &done).await, MOCK_JOINTS, "{what}");
        let meta = row_metadata(&db, id(&done)).await;
        assert!(meta["delivery"]["joint_naming"].is_null(), "{what}: {meta}");
        assert!(meta["source"].get("joint_naming").is_none(), "{what}: {meta}");
    }
}

/// Its own id: a runtime model may not shadow a catalog one.
const OPERATOR_ANIMATE: &str = "mock/operator-animate-3d";

/// An operator's animate row that takes `joint_naming`, added at runtime: the
/// catalog's `mock/animate-3d` row (models/mock.yaml) under
/// [`OPERATOR_ANIMATE`], with `mock/rig-3d`'s `joint_naming` param.
fn operator_animate_row() -> Value {
    json!({
        "id": OPERATOR_ANIMATE,
        "provider": "mock",
        "media_type": "model3d",
        "task": "animate",
        "native_orientation": {
            "up": "source",
            "front": "source",
            "evidence": "the mock hands back its source mesh unchanged (or, chained by a mock rig's task id, the mesh that rig job stored), plus clips (litegen-core/src/providers/model3d/mock.rs)"
        },
        "display_name": "Operator Animate 3D",
        "description": "mock/animate-3d as an operator adds it, taking joint_naming.",
        "pricing": { "base_cost_usd": 0.0 },
        "capabilities": {},
        "prompt": { "required": false, "max_length": 0 },
        "params": {
            "source_model": {
                "kind": "source_model",
                "formats": ["glb"],
                "max_bytes": 52428800,
                "requires_skin": true,
                "label": "Source mesh",
                "description": "The rigged mesh to animate."
            },
            "animations": {
                "kind": "animation_list",
                "max_items": 5,
                "presets": [
                    { "id": "mock:idle", "canonical": "idle", "loopable": true },
                    { "id": "mock:walk", "canonical": "walk", "loopable": true },
                    { "id": "mock:run", "canonical": "run", "loopable": true },
                    { "id": "mock:wave" }
                ],
                "text_to_motion": { "min_duration_s": 1, "max_duration_s": 10 },
                "in_place": "native",
                "label": "Animations",
                "description": "Clips to make, in order, each named as given."
            },
            "fps": { "kind": "int", "min": 1, "max": 60, "default": 30, "label": "Frames per second" },
            "joint_naming": {
                "kind": "string",
                "enum_values": ["vendor", "vrm", "unity", "mixamo"],
                "default": "vendor",
                "label": "Joint naming",
                "description": "`vendor` keeps the mock's own joint names; `vrm`, `unity` and `mixamo` are litegen's renames of them."
            },
            "output_formats": {
                "kind": "string_array",
                "enum_values": ["glb"],
                "max_items": 1,
                "default": ["glb"],
                "label": "Output formats",
                "description": "Mesh containers. The mock emits glb; litegen derives the rest."
            }
        },
        "extra_allowlist": [],
        "tags": ["mock", "test"]
    })
}

/// A request that names its joint naming gets it, `vendor` included,
/// whatever its rig was delivered in. No carried animate row takes
/// `joint_naming`; an operator's may.
#[tokio::test]
async fn an_explicit_vendor_naming_is_not_overridden() {
    let (app, db) = harness::app_with_mock_3d().await;
    let (status, added) = post_to(&app, "/v1/admin/models", operator_animate_row()).await;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    let rig = unity_rig(&app).await;
    let mut asked = walk();
    asked["model"] = OPERATOR_ANIMATE.into();
    asked["joint_naming"] = "vendor".into();
    let done = animate(&app, id(&rig), asked).await;
    assert_eq!(file_joints(&app, &done).await, MOCK_JOINTS);
    let meta = row_metadata(&db, id(&done)).await;
    assert!(meta["delivery"]["joint_naming"].is_null(), "{meta}");
    assert_eq!(meta["source"]["joint_naming"], "unity", "the rig's naming was there to inherit: {meta}");
}

/// Only an animation inherits: a texture, remesh or segment of a rig, or a
/// re-rig, can come back unskinned or with a new skeleton. Its lineage still
/// records the naming its source was delivered in.
#[tokio::test]
async fn only_animation_inherits_the_naming() {
    let (app, db) = harness::app_with_mock_3d().await;
    let rig = unity_rig(&app).await;
    let done = finished(&app, json!({ "model": "mock/texture-3d", "prompt": "oak", "source_model": { "type": "generation", "value": id(&rig) } })).await;
    let meta = row_metadata(&db, id(&done)).await;
    assert!(meta["delivery"]["joint_naming"].is_null(), "{meta}");
    assert!(naming_notes(&done).is_empty(), "{:?}", naming_notes(&done));
    assert_eq!(meta["source"]["joint_naming"], "unity", "recorded for any chained source: {meta}");
}
