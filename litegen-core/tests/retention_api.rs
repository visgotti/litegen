//! Retention and deletion of kept outputs (docs/storage-access.md): the
//! sweep deletes an app's files after its `retention_days`, and the row says
//! so.

mod harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use litegen::access::present::Presenting;
use litegen::api::middleware::{AppState, DEFAULT_APP_ID};
use litegen::db::sqlite::SqliteDatabase;
use std::sync::Arc;
use tower::ServiceExt;

const DAY: chrono::Duration = chrono::Duration::days(1);

async fn app() -> (Router, Arc<AppState>, Arc<SqliteDatabase>, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_over(db.clone(), &dir).await;
    (app, state, db, dir)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<serde_json::Value>) -> (StatusCode, serde_json::Value) {
    let req = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

async fn set_retention(app: &Router, days: serde_json::Value) {
    let (s, v) = call(app, "PUT", &format!("/v1/apps/{DEFAULT_APP_ID}/access"), Some(serde_json::json!({ "retention_days": days }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

/// A finished mesh of the default app, kept in the local store.
async fn mesh(app: &Router) -> String {
    let id = harness::submit(app, "mock/mesh-3d", "a cube").await;
    let done = harness::poll_until_terminal(app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    id
}

fn files_for(dir: &std::path::Path, id: &str) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = e.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.to_string_lossy().contains(id) {
                found.push(path);
            }
        }
    }
    found
}

async fn sweep(state: &Arc<AppState>, now: chrono::DateTime<chrono::Utc>, limit: usize) -> litegen::proxy::retention::SweepReport {
    litegen::proxy::retention::sweep_retention_once(&Presenting::of(state), now, limit).await
}

async fn generation(app: &Router, id: &str) -> serde_json::Value {
    let (s, v) = call(app, "GET", &format!("/v1/generations/{id}"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

#[tokio::test]
async fn files_older_than_the_apps_retention_are_deleted() {
    let (app, state, _db, dir) = app().await;
    set_retention(&app, 1.into()).await;
    let id = mesh(&app).await;
    assert!(!files_for(&dir, &id).is_empty());

    // Half a day on, it is within the retention.
    sweep(&state, chrono::Utc::now() + DAY / 2, 500).await;
    assert!(!files_for(&dir, &id).is_empty(), "kept a day");
    assert!(generation(&app, &id).await["files_deleted_at"].is_null());

    // Two days on, it is past it.
    let report = sweep(&state, chrono::Utc::now() + DAY * 2, 500).await;
    assert_eq!(report.generations, 1, "{report:?}");
    assert!(files_for(&dir, &id).is_empty(), "deleted: {:?}", files_for(&dir, &id));
    let gen = generation(&app, &id).await;
    assert!(gen["files_deleted_at"].is_string(), "{gen}");
    assert_eq!(gen["status"], "completed", "the row stays: {gen}");
    let (s, v) = call(&app, "GET", &format!("/v1/generations/{id}/assets/0"), None).await;
    assert_eq!(s, StatusCode::GONE, "{v}");
    assert_eq!(v["error"]["code"], "files_deleted", "{v}");
    assert!(v["error"]["message"].as_str().is_some_and(|m| m.contains("retention")), "{v}");

    // Swept once: the next sweep has nothing to do.
    assert_eq!(sweep(&state, chrono::Utc::now() + DAY * 3, 500).await.generations, 0);
}

#[tokio::test]
async fn retention_zero_keeps_everything() {
    let (app, state, _db, dir) = app().await;
    let id = mesh(&app).await;
    let report = sweep(&state, chrono::Utc::now() + DAY * 365 * 50, 500).await;
    assert_eq!(report.generations, 0, "{report:?}");
    assert!(!files_for(&dir, &id).is_empty());
}

#[tokio::test]
async fn an_app_override_beats_the_gateway_default() {
    let (app, state, _db, dir) = app().await;
    let (s, v) = call(&app, "PUT", "/v1/admin/settings", Some(serde_json::json!({ "storage_access_retention_days": 1 }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    set_retention(&app, 0.into()).await;
    let id = mesh(&app).await;
    sweep(&state, chrono::Utc::now() + DAY * 2, 500).await;
    assert!(!files_for(&dir, &id).is_empty(), "the app keeps its files until deleted");
    set_retention(&app, serde_json::Value::Null).await;
    sweep(&state, chrono::Utc::now() + DAY * 2, 500).await;
    assert!(files_for(&dir, &id).is_empty(), "the gateway's day applies again");
}

#[tokio::test]
async fn a_failed_delete_is_retried_next_sweep() {
    let (app, state, _db, dir) = app().await;
    set_retention(&app, 1.into()).await;
    let id = mesh(&app).await;
    // A directory where the mesh was: deleting it fails.
    let mesh_file = files_for(&dir, &id).into_iter().find(|p| p.to_string_lossy().ends_with(".glb")).expect("the mesh");
    std::fs::remove_file(&mesh_file).unwrap();
    std::fs::create_dir(&mesh_file).unwrap();
    std::fs::write(mesh_file.join("blocker"), b"x").unwrap();
    let report = sweep(&state, chrono::Utc::now() + DAY * 2, 500).await;
    assert_eq!((report.generations, report.failed), (0, 1), "{report:?}");
    assert!(generation(&app, &id).await["files_deleted_at"].is_null(), "not marked: tried again next time");
    std::fs::remove_dir_all(&mesh_file).unwrap();
    let report = sweep(&state, chrono::Utc::now() + DAY * 2, 500).await;
    assert_eq!(report.generations, 1, "{report:?}");
    assert!(generation(&app, &id).await["files_deleted_at"].is_string());
}

/// A delete that keeps failing (a bucket that refuses, a file that can't be
/// removed) must not hold up the generations behind it (P12): the rows that
/// failed go behind the ones not yet tried.
#[tokio::test]
async fn failed_deletes_do_not_hold_up_newer_generations() {
    let (app, state, _db, dir) = app().await;
    set_retention(&app, 1.into()).await;
    let older = [mesh(&app).await, mesh(&app).await];
    // SQLite keeps whole seconds: the newest is made a second later.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let newest = mesh(&app).await;
    // A directory where each older mesh was: deleting it fails, every time.
    for id in &older {
        let mesh_file = files_for(&dir, id).into_iter().find(|p| p.to_string_lossy().ends_with(".glb")).expect("the mesh");
        std::fs::remove_file(&mesh_file).unwrap();
        std::fs::create_dir(&mesh_file).unwrap();
        std::fs::write(mesh_file.join("blocker"), b"x").unwrap();
    }
    let later = chrono::Utc::now() + DAY * 2;
    let first = sweep(&state, later, 2).await;
    assert_eq!((first.generations, first.failed), (0, 2), "the two oldest fail: {first:?}");
    let second = sweep(&state, later, 2).await;
    assert!(generation(&app, &newest).await["files_deleted_at"].is_string(), "the newest was not swept: {second:?}");
    assert_eq!((second.generations, second.failed), (1, 1), "the newest, then one of the two that fail: {second:?}");
}

/// A sweep keeps going while it deletes (P12, NEW 1): a backlog larger than
/// one batch is worked off in one sweep, not one batch an hour.
#[tokio::test]
async fn a_sweep_keeps_going_while_it_deletes() {
    let (app, state, _db, _dir) = app().await;
    set_retention(&app, 1.into()).await;
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(mesh(&app).await);
    }
    let report = litegen::proxy::retention::sweep_until_done(
        &Presenting::of(&state), chrono::Utc::now() + DAY * 2, 2, std::time::Duration::from_secs(60),
    ).await;
    assert_eq!((report.generations, report.failed), (5, 0), "{report:?}");
    for id in &ids {
        assert!(generation(&app, id).await["files_deleted_at"].is_string(), "{id} was swept");
    }
}

/// ...within its time budget: the first batch always runs, and no further
/// one starts once the budget is spent.
#[tokio::test]
async fn a_sweep_stops_at_its_time_budget() {
    let (app, state, _db, _dir) = app().await;
    set_retention(&app, 1.into()).await;
    for _ in 0..5 {
        mesh(&app).await;
    }
    let report = litegen::proxy::retention::sweep_until_done(
        &Presenting::of(&state), chrono::Utc::now() + DAY * 2, 2, std::time::Duration::ZERO,
    ).await;
    assert_eq!(report.generations, 2, "one batch: {report:?}");
}

/// An app of the default org with a retention of its own.
async fn app_with_retention(app: &Router, name: &str, days: u32) -> String {
    let (s, created) = call(app, "POST", &format!("/v1/orgs/{}/apps", litegen::api::middleware::DEFAULT_ORG_ID), Some(serde_json::json!({ "name": name }))).await;
    assert!(s.is_success(), "{created}");
    let app_id = created["id"].as_str().unwrap().to_string();
    let (s, v) = call(app, "PUT", &format!("/v1/apps/{app_id}/access"), Some(serde_json::json!({ "retention_days": days }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    app_id
}

/// A finished mesh of `app_id` (a request makes only the default app's), kept
/// in litegen's store. Returns its file.
async fn kept_mesh_of(state: &Arc<AppState>, db: &SqliteDatabase, dir: &std::path::Path, app_id: &str, id: &str) -> std::path::PathBuf {
    use litegen::db::DatabaseStore;
    let url = state
        .router
        .model3d_store
        .put(&format!("litegen/3d/{id}/model.glb"), &bytes::Bytes::from_static(b"glTF-ish"), "model/gltf-binary")
        .await
        .unwrap();
    db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, Some(litegen::api::middleware::DEFAULT_ORG_ID), Some(app_id)).await.unwrap();
    let meta = serde_json::json!({ "assets": [{ "index": 0, "kind": "mesh", "format": "glb", "url": url }] });
    db.update_generation_if_active(id, "completed", 100, Some(&url), None, Some(chrono::Utc::now()), Some(&meta)).await.unwrap();
    let mut files = files_for(dir, id);
    assert_eq!(files.len(), 1, "{files:?}");
    files.pop().unwrap()
}

/// A pass whose deletes all fail sits out the rest of the sweep (P12 NEW 1,
/// review M1): while another pass works off its backlog, an app whose bucket
/// refuses is tried once a sweep, not once a batch (each try resolves its
/// stores again and sends it deletes that fail).
#[tokio::test]
async fn a_pass_whose_deletes_fail_is_tried_once_while_another_works_off_its_backlog() {
    let (app, state, db, dir) = app().await;
    let refusing = app_with_retention(&app, "refusing", 1).await;
    let stuck = "litegen-3d-retention-refused";
    let file = kept_mesh_of(&state, &db, &dir, &refusing, stuck).await;
    // A directory where the mesh was: deleting it fails, every time.
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    std::fs::write(file.join("blocker"), b"x").unwrap();
    set_retention(&app, 1.into()).await;
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(mesh(&app).await);
    }
    let report = litegen::proxy::retention::sweep_until_done(
        &Presenting::of(&state), chrono::Utc::now() + DAY * 2, 2, std::time::Duration::from_secs(60),
    ).await;
    assert_eq!((report.generations, report.failed), (5, 1), "the default app's 5, two at a time, and the refused one tried once: {report:?}");
    for id in &ids {
        assert!(generation(&app, id).await["files_deleted_at"].is_string(), "{id} was swept");
    }
    use litegen::db::DatabaseStore;
    assert!(db.get_generation(stuck).await.unwrap().unwrap().files_deleted_at.is_none(), "the refused row is not marked");
}

/// ...and the passes take turns: a sweep whose budget is spent has still given
/// every pass a batch, so one app's backlog can't hold up the gateway's pass
/// (which comes last).
#[tokio::test]
async fn every_pass_gets_a_batch_before_the_budget_ends() {
    let (app, state, db, dir) = app().await;
    let (s, v) = call(&app, "PUT", "/v1/admin/settings", Some(serde_json::json!({ "storage_access_retention_days": 1 }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let busy = app_with_retention(&app, "busy", 1).await;
    for i in 0..3 {
        kept_mesh_of(&state, &db, &dir, &busy, &format!("litegen-3d-retention-busy-{i}")).await;
    }
    // The default app has no retention of its own: the gateway's pass.
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(mesh(&app).await);
    }
    let report = litegen::proxy::retention::sweep_until_done(
        &Presenting::of(&state), chrono::Utc::now() + DAY * 2, 2, std::time::Duration::ZERO,
    ).await;
    assert_eq!((report.generations, report.failed), (4, 0), "a batch of 2 for the busy app's pass and one for the gateway's: {report:?}");
    let mut swept = 0;
    for id in &ids {
        if generation(&app, id).await["files_deleted_at"].is_string() {
            swept += 1;
        }
    }
    assert_eq!(swept, 2, "the gateway's pass had its batch");
}

#[tokio::test]
async fn a_backlog_is_swept_in_batches() {
    let (app, state, _db, _dir) = app().await;
    set_retention(&app, 1.into()).await;
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(mesh(&app).await);
    }
    let later = chrono::Utc::now() + DAY * 2;
    let mut marked = Vec::new();
    for _ in 0..3 {
        sweep(&state, later, 2).await;
        let mut n = 0;
        for id in &ids {
            if generation(&app, id).await["files_deleted_at"].is_string() {
                n += 1;
            }
        }
        marked.push(n);
    }
    assert_eq!(marked, [2, 4, 5], "two at a time");
}

#[tokio::test]
async fn kept_false_assets_are_not_deleted_anywhere() {
    let (app, state, db, _dir) = app().await;
    set_retention(&app, 1.into()).await;
    use litegen::db::DatabaseStore;
    let id = "litegen-vid-retention-vendor";
    db.insert_generation(id, None, "mock/video-gen", "mock", "video", None, 0.0, Some(litegen::api::middleware::DEFAULT_ORG_ID), Some(DEFAULT_APP_ID)).await.unwrap();
    let meta = serde_json::json!({ "keep_outputs": false, "assets": [{ "index": 0, "kind": "video", "format": "mp4", "url": "https://cdn.vendor.example/v.mp4", "kept": false }] });
    db.update_generation_if_active(id, "completed", 100, Some("https://cdn.vendor.example/v.mp4"), None, Some(chrono::Utc::now()), Some(&meta)).await.unwrap();
    let report = sweep(&state, chrono::Utc::now() + DAY * 2, 500).await;
    assert_eq!((report.generations, report.failed), (1, 0), "{report:?}");
    assert!(generation(&app, id).await["files_deleted_at"].is_string());
}

// ─── Deleting ───────────────────────────────────────────────────────────────

async fn artifact_status(app: &Router, id: &str) -> StatusCode {
    call(app, "GET", &format!("/v1/logs/{id}/artifact"), None).await.0
}

#[tokio::test]
async fn deleting_a_generation_deletes_its_files_and_request() {
    let (app, _state, _db, dir) = app().await;
    let id = mesh(&app).await;
    for _ in 0..100 {
        if artifact_status(&app, &id).await == StatusCode::OK {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(artifact_status(&app, &id).await, StatusCode::OK, "the request log holds the prompt");
    let before = generation(&app, &id).await;
    assert!(before["metadata"]["request"].is_object(), "{before}");

    let (s, v) = call(&app, "DELETE", &format!("/v1/generations/{id}"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["files_deleted_at"].is_string(), "{v}");
    assert_eq!(v["files_deleted_reason"], "deleted", "{v}");
    assert!(files_for(&dir, &id).is_empty(), "the files are gone");
    let after = generation(&app, &id).await;
    assert!(after["metadata"]["request"].is_null(), "the prompt and params are gone: {after}");
    assert_eq!(after["cost_usd"], before["cost_usd"], "the billing record stays");
    assert_eq!(artifact_status(&app, &id).await, StatusCode::NOT_FOUND, "the request log's copy is gone too");
    let (s, v) = call(&app, "GET", &format!("/v1/generations/{id}/assets/0"), None).await;
    assert_eq!((s, v["error"]["code"].as_str()), (StatusCode::GONE, Some("files_deleted")), "{v}");
}

#[tokio::test]
async fn a_running_generation_is_not_deleted() {
    let (app, _state, _db, _dir) = app().await;
    let (s, v) = call(&app, "POST", "/v1/videos/generations", Some(serde_json::json!({ "model": "mock/video-gen", "prompt": "a wave" }))).await;
    assert!(s.is_success(), "{v}");
    let id = v["id"].as_str().unwrap();
    let (s, v) = call(&app, "DELETE", &format!("/v1/generations/{id}"), None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "generation_running", "{v}");
}

#[tokio::test]
async fn deleting_needs_the_delete_permission() {
    const MASTER: &str = "sk-retention-master";
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, _db) = harness::app_with_hooks_tweaked(&dir, Some(MASTER), Vec::new(), |_| {}).await;
    let as_key = |key: &str| format!("Bearer {key}");
    let send = |method: &'static str, uri: String, auth: String, body: Option<serde_json::Value>| {
        let app = app.clone();
        async move {
            let req = Request::builder().method(method).uri(uri).header("authorization", auth);
            let req = match body {
                Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
                None => req.body(Body::empty()).unwrap(),
            };
            let resp = app.oneshot(req).await.unwrap();
            let s = resp.status();
            let b = axum::body::to_bytes(resp.into_body(), 1 << 24).await.unwrap();
            (s, serde_json::from_slice::<serde_json::Value>(&b).unwrap_or_default())
        }
    };
    let key = |name: &'static str| send("POST", "/v1/keys".into(), as_key(MASTER), Some(serde_json::json!({ "name": name })));
    let maker = key("maker").await.1["key"].as_str().unwrap().to_string();
    let other = key("other").await.1["key"].as_str().unwrap().to_string();
    let (s, v) = send("POST", "/v1/models3d/generations".into(), as_key(&maker), Some(serde_json::json!({ "model": "mock/mesh-3d", "prompt": "a cube" }))).await;
    assert!(s.is_success(), "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    for _ in 0..20 {
        if send("GET", format!("/v1/models3d/{id}"), as_key(&maker), None).await.1["status"] == "completed" {
            break;
        }
    }
    let (s, v) = send("DELETE", format!("/v1/generations/{id}"), as_key(&other), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "another key, under the default owner scope `key`: {v}");
    let (s, v) = send("DELETE", format!("/v1/generations/{id}"), as_key(&maker), None).await;
    assert_eq!(s, StatusCode::OK, "the key that made it: {v}");
}

#[tokio::test]
async fn deleting_an_app_deletes_its_files_in_litegens_store() {
    let (app, state, db, dir) = app().await;
    let (s, created) = call(&app, "POST", &format!("/v1/orgs/{}/apps", litegen::api::middleware::DEFAULT_ORG_ID), Some(serde_json::json!({ "name": "short-lived" }))).await;
    assert!(s.is_success(), "{created}");
    let app_id = created["id"].as_str().unwrap().to_string();
    let id = "litegen-3d-app-going";
    let url = state
        .router
        .model3d_store
        .put(&format!("litegen/3d/{id}/model.glb"), &bytes::Bytes::from_static(b"glTF-ish"), "model/gltf-binary")
        .await
        .unwrap();
    use litegen::db::DatabaseStore;
    db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, Some(litegen::api::middleware::DEFAULT_ORG_ID), Some(&app_id)).await.unwrap();
    let meta = serde_json::json!({ "assets": [{ "index": 0, "kind": "mesh", "format": "glb", "url": url }] });
    db.update_generation_if_active(id, "completed", 100, Some(&url), None, Some(chrono::Utc::now()), Some(&meta)).await.unwrap();
    assert_eq!(files_for(&dir, id).len(), 1);

    let (s, v) = call(&app, "DELETE", &format!("/v1/apps/{app_id}"), None).await;
    assert!(s.is_success(), "{s} {v}");
    for _ in 0..250 {
        if files_for(&dir, id).is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(files_for(&dir, id).is_empty(), "the app's files in litegen's store went with it");
}
