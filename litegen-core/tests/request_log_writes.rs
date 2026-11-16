//! A billed request's log row is written before its answer: a process that
//! stops, or dies, right after it answered keeps the row of a request it
//! billed. Each test answers in a process of its own (a runtime the test
//! drops at once, with whatever it hadn't finished), then reads the log as
//! the next process over the same database file would.

mod harness;

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::DatabaseStore;
use serde_json::{json, Value};
use tower::ServiceExt;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime")
}

/// A database file in `dir`, as a URL.
fn database(dir: &Path) -> String {
    format!("sqlite://{}?mode=rwc", dir.join("litegen.db").display())
}

async fn post(app: &axum::Router, uri: &str, body: &Value) -> (StatusCode, Value) {
    let req = Request::post(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A process over the database at `url` answers each POST of `before`
/// (each must succeed), then a POST of `body` to `uri`, and stops at once.
/// The last answer's status and body.
fn answer_then_stop(dir: &Path, url: &str, before: &[(&str, Value)], uri: &str, body: Value) -> (StatusCode, Value) {
    let process = runtime();
    let answered = process.block_on(async {
        let db = Arc::new(SqliteDatabase::connect(url).await.expect("the database file"));
        let app = harness::app_over(db, dir, |_| {}).await;
        for (uri, body) in before {
            let (status, answer) = post(&app, uri, body).await;
            assert!(status.is_success(), "{uri}: {status} {answer}");
        }
        post(&app, uri, &body).await
    });
    drop(process);
    answered
}

/// The request log as the next process reads it: each row's id, model,
/// status and cost, and whether its artifact is there.
fn logged(url: &str) -> Vec<(String, String, String, f64, bool)> {
    runtime().block_on(async {
        let db = SqliteDatabase::connect(url).await.expect("the database file");
        let (rows, _) = db.get_request_logs(1, 100).await.unwrap();
        let mut logged = Vec::new();
        for row in rows {
            let artifact = db.get_request_artifact(&row.id).await.unwrap().is_some();
            let status = serde_json::to_value(row.status).unwrap().as_str().unwrap_or_default().to_string();
            logged.push((row.id, row.model, status, row.cost_usd, artifact));
        }
        logged
    })
}

/// What the next process reads in the request log for `id`: its status and
/// cost, and whether its artifact is there.
fn logged_as(url: &str, id: &str) -> Option<(String, f64, bool)> {
    logged(url).into_iter().find(|row| row.0 == id).map(|(_, _, status, cost, artifact)| (status, cost, artifact))
}

fn photo() -> String {
    use base64::Engine;
    let img = image::RgbaImage::from_fn(64, 48, |x, y| image::Rgba([(x * 3) as u8, (y * 4) as u8, 90, 255]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(&out)
}

#[test]
fn an_images_request_log_is_written_before_its_answer() {
    let dir = tempfile::tempdir().unwrap();
    let url = database(dir.path());
    // A priced row ($5), so its charge shows.
    let (status, body) = answer_then_stop(dir.path(), &url, &[], "/v1/images/generations", json!({ "model": "mock/expensive-image", "prompt": "a red cube" }));
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().expect("an id").to_string();

    let (status, cost, artifact) = logged_as(&url, &id).expect("the billed image's log row outlives the process that answered");
    assert_eq!(status, "completed");
    assert!(cost > 0.0, "its charge: {cost}");
    assert!(artifact, "and its artifact");
}

#[test]
fn a_segmentations_request_log_is_written_before_its_answer() {
    let dir = tempfile::tempdir().unwrap();
    let url = database(dir.path());
    let request = json!({ "model": "mock/segment-image", "image": photo(), "prompts": [{ "type": "box", "x": 4, "y": 4, "w": 16, "h": 16 }] });
    let (status, body) = answer_then_stop(dir.path(), &url, &[], "/v1/images/segmentations", request);
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_str().expect("an id").to_string();

    let (status, cost, _) = logged_as(&url, &id).expect("the billed segmentation's log row outlives the process that answered");
    assert_eq!(status, "completed");
    assert!(cost > 0.0, "its charge: {cost}");
}

/// The failed one's row too: a segmentation row the mock vendor refuses
/// (it segments only with `mock/segment-image`), added as a runtime model.
#[test]
fn a_failed_segmentations_request_log_is_written_before_its_answer() {
    let dir = tempfile::tempdir().unwrap();
    let url = database(dir.path());
    let row = json!({
        "id": "mock/segment-image-refused",
        "provider": "mock",
        "media_type": "image",
        "task": "segment",
        "display_name": "Mock segmentation the vendor refuses",
        "pricing": { "base_cost_usd": 0.005 },
        "capabilities": {},
        "prompt": { "required": false, "max_length": 0 },
        "params": { "prompts": { "kind": "segmentation_prompts", "types": ["box"], "max_items": 16, "max_masks": 8 } }
    });
    let request = json!({ "model": "mock/segment-image-refused", "image": photo(), "prompts": [{ "type": "box", "x": 4, "y": 4, "w": 16, "h": 16 }] });
    let (status, body) = answer_then_stop(dir.path(), &url, &[("/v1/admin/models", row)], "/v1/images/segmentations", request);
    assert!(!status.is_success(), "the vendor refused it: {status} {body}");

    let rows: Vec<_> = logged(&url).into_iter().filter(|row| row.1 == "mock/segment-image-refused").collect();
    assert_eq!(rows.len(), 1, "the failed segmentation's log row outlives the process that answered: {rows:?}");
    assert_eq!((rows[0].2.as_str(), rows[0].3), ("failed", 0.0), "failed, and charged nothing");
}

/// A 3D submit's log row was written before its answer already; its
/// artifact now is too.
#[test]
fn a_3d_submits_request_log_and_artifact_are_written_before_its_answer() {
    let dir = tempfile::tempdir().unwrap();
    let url = database(dir.path());
    let (status, body) = answer_then_stop(dir.path(), &url, &[], "/v1/models3d/generations", json!({ "model": "mock/mesh-3d", "prompt": "a cube" }));
    assert!(status.is_success(), "{status} {body}");
    let id = body["id"].as_str().expect("an id").to_string();

    let (_, _, artifact) = logged_as(&url, &id).expect("the submit's log row outlives the process that answered");
    assert!(artifact, "and its artifact");
}

/// A video submit's, likewise.
#[test]
fn a_video_submits_request_log_and_artifact_are_written_before_its_answer() {
    let dir = tempfile::tempdir().unwrap();
    let url = database(dir.path());
    let (status, body) = answer_then_stop(dir.path(), &url, &[], "/v1/videos/generations", json!({ "model": "mock/video-gen", "prompt": "a wave" }));
    assert!(status.is_success(), "{status} {body}");
    let id = body["id"].as_str().expect("an id").to_string();

    let (_, _, artifact) = logged_as(&url, &id).expect("the submit's log row outlives the process that answered");
    assert!(artifact, "and its artifact");
}

/// A cache hit is a request of its own: it answers with the earlier
/// generation, and the request log has a row for it, under its own id, at
/// what the hit was charged (the cached answer's price), with its artifact.
#[tokio::test]
async fn a_cache_hit_has_a_log_row_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let (app, db) = harness::app_with(dir.path(), |c| c.cache.enabled = true).await;
    let ask = || async {
        let body = json!({ "model": "mock/expensive-image", "prompt": "a red cube" });
        let req = Request::post("/v1/images/generations").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let cache = resp.headers().get("x-litegen-cache").map(|v| v.to_str().unwrap().to_string());
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
        (status, cache, serde_json::from_slice::<Value>(&bytes).unwrap())
    };
    let (status, cache, first) = ask().await;
    assert_eq!((status, cache.as_deref()), (StatusCode::OK, Some("miss")), "{first}");
    let (status, cache, second) = ask().await;
    assert_eq!((status, cache.as_deref()), (StatusCode::OK, Some("hit")), "{second}");
    assert_eq!(second["id"], first["id"], "a hit answers with the earlier generation");

    let (rows, _) = db.get_request_logs(1, 100).await.unwrap();
    let rows: Vec<_> = rows.into_iter().filter(|r| r.model == "mock/expensive-image").collect();
    assert_eq!(rows.len(), 2, "the hit has a row of its own: {rows:?}");
    assert_ne!(rows[0].id, rows[1].id);
    for row in &rows {
        assert_eq!(serde_json::to_value(row.status).unwrap(), "completed", "{row:?}");
        assert_eq!(row.cost_usd, 5.0, "what it was charged: {row:?}");
        assert!(db.get_request_artifact(&row.id).await.unwrap().is_some(), "and its own artifact: {row:?}");
    }
}

/// A hit's artifact holds no copy of the image: it names the generation
/// whose answer it was served (`output_kind: "generation"`, its id in
/// `output_value`), which holds the image. So the image is kept once, and
/// deleting that generation (or its files going by retention) leaves nothing
/// of it in the hits' artifacts.
#[tokio::test]
async fn a_cache_hits_artifact_points_at_its_generation() {
    let dir = tempfile::tempdir().unwrap();
    let (app, db) = harness::app_with(dir.path(), |c| c.cache.enabled = true).await;
    let ask = || async {
        let body = json!({ "model": "mock/image-gen", "prompt": "a green cube" });
        let req = Request::post("/v1/images/generations").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let cache = resp.headers().get("x-litegen-cache").map(|v| v.to_str().unwrap().to_string());
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
        (cache, serde_json::from_slice::<Value>(&bytes).unwrap())
    };
    let (cache, first) = ask().await;
    assert_eq!(cache.as_deref(), Some("miss"), "{first}");
    let (cache, second) = ask().await;
    assert_eq!(cache.as_deref(), Some("hit"), "{second}");
    let generation = first["id"].as_str().unwrap();

    let (rows, _) = db.get_request_logs(1, 100).await.unwrap();
    let hit = rows.iter().find(|r| r.model == "mock/image-gen" && r.id != generation).expect("the hit's own row");
    let artifact = db.get_request_artifact(&hit.id).await.unwrap().expect("the hit's artifact");
    assert_eq!(artifact.output_kind, "generation", "{artifact:?}");
    assert_eq!(artifact.output_value.as_deref(), Some(generation), "it names the generation it was served");
    assert!(!artifact.output_truncated, "{artifact:?}");
    assert_eq!(artifact.prompt.as_deref(), Some("a green cube"), "its own request is recorded");
    let original = db.get_request_artifact(generation).await.unwrap().expect("the generation's own artifact");
    assert_eq!(original.output_kind, "b64", "the one copy, the generation's own request's");
}

// ─── a cache hit's marker, and whether hits are charged ────────────────────

const MASTER: &str = "sk-cache-hits-master";

async fn send(app: &axum::Router, method: &str, uri: &str, bearer: &str, body: Option<Value>) -> (StatusCode, axum::http::HeaderMap, Value) {
    let req = Request::builder().method(method).uri(uri).header("authorization", format!("Bearer {bearer}"));
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// One image of `mock/expensive-image` ($5) asked with `bearer`: the answer's
/// id and its `X-Litegen-Cache`.
async fn expensive_image(app: &axum::Router, bearer: &str) -> (String, String) {
    let (status, headers, body) = send(app, "POST", "/v1/images/generations", bearer, Some(json!({ "model": "mock/expensive-image", "prompt": "a red cube" }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cache = headers.get("x-litegen-cache").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
    (body["id"].as_str().unwrap().to_string(), cache)
}

/// The rows of `mock/expensive-image` in `GET /v1/logs` and `GET
/// /v1/spend/logs`, each as (id, cost, cache_hit), sorted by id.
async fn logged_and_spent(app: &axum::Router) -> (Vec<(String, f64, Value)>, Vec<(String, f64, Value)>) {
    let rows = |body: &Value| {
        let mut rows: Vec<_> = body["data"]
            .as_array()
            .unwrap_or_else(|| panic!("{body}"))
            .iter()
            .filter(|r| r["model"] == "mock/expensive-image")
            .map(|r| (r["id"].as_str().unwrap().to_string(), r["cost_usd"].as_f64().unwrap(), r["cache_hit"].clone()))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    };
    let (status, _, logs) = send(app, "GET", "/v1/logs?per_page=100", MASTER, None).await;
    assert_eq!(status, StatusCode::OK, "{logs}");
    let (status, _, spend) = send(app, "GET", "/v1/spend/logs?per_page=100", MASTER, None).await;
    assert_eq!(status, StatusCode::OK, "{spend}");
    (rows(&logs), rows(&spend))
}

#[tokio::test]
async fn a_cache_hits_row_says_so_in_the_logs_and_the_spend_logs() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _db) = harness::app_with_hooks_tweaked(dir.path(), Some(MASTER), Vec::new(), |c| c.cache.enabled = true).await;
    let (first, cache) = expensive_image(&app, MASTER).await;
    assert_eq!(cache, "miss");
    let (_, cache) = expensive_image(&app, MASTER).await;
    assert_eq!(cache, "hit");

    let (logs, spend) = logged_and_spent(&app).await;
    for (route, rows) in [("logs", &logs), ("spend logs", &spend)] {
        assert_eq!(rows.len(), 2, "{route}: {rows:?}");
        for (id, cost, cache_hit) in rows {
            let hit = *id != first;
            assert_eq!((*cost, cache_hit.clone()), (5.0, Value::Bool(hit)), "{route}: {id}");
        }
    }
}

/// With `cache_charge_hits: false` a hit is free: its row costs 0 and says
/// `cache_hit`, and neither the key's quota nor its budget moves.
#[tokio::test]
async fn a_free_cache_hit_charges_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (app, db) = harness::app_with_hooks_tweaked(dir.path(), Some(MASTER), Vec::new(), |c| c.cache.enabled = true).await;
    let (status, _, view) = send(&app, "PUT", "/v1/admin/settings", MASTER, Some(json!({ "cache_enabled": true, "cache_charge_hits": false }))).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let (status, _, created) = send(&app, "POST", "/v1/keys", MASTER, Some(json!({ "name": "free-hits", "token_quota": 100.0 }))).await;
    assert!(status.is_success(), "{created}");
    let (key_id, secret) = (created["id"].as_str().unwrap().to_string(), created["key"].as_str().unwrap().to_string());
    let (status, _, limits) = send(&app, "PUT", &format!("/v1/keys/{key_id}/limits"), MASTER, Some(json!({ "max_budget_usd": 50.0 }))).await;
    assert_eq!(status, StatusCode::OK, "{limits}");
    let key_uuid: uuid::Uuid = key_id.parse().unwrap();
    let charged = || {
        let (db, key_id) = (db.clone(), key_id.clone());
        async move {
            let tokens = db.get_api_key(&key_uuid).await.unwrap().unwrap().tokens_used;
            let spend = db.management().unwrap().get_limits(litegen::management::types::EntityType::Key, &key_id).await.unwrap().unwrap().spend_usd;
            (tokens, spend)
        }
    };

    let (first, cache) = expensive_image(&app, &secret).await;
    assert_eq!(cache, "miss");
    let after_the_miss = charged().await;
    assert!(after_the_miss.0 > 0.0 && after_the_miss.1 > 0.0, "the miss is charged: {after_the_miss:?}");
    let (_, cache) = expensive_image(&app, &secret).await;
    assert_eq!(cache, "hit");
    assert_eq!(charged().await, after_the_miss, "the hit moved neither the key's quota nor its budget");

    let (logs, spend) = logged_and_spent(&app).await;
    for (route, rows) in [("logs", &logs), ("spend logs", &spend)] {
        let hit: Vec<_> = rows.iter().filter(|(id, _, _)| *id != first).collect();
        assert_eq!(hit.len(), 1, "{route}: {rows:?}");
        assert_eq!((hit[0].1, hit[0].2.clone()), (0.0, Value::Bool(true)), "{route}: the hit costs nothing and says so");
    }
}

/// `cache_charge_hits` round-trips through `PUT`/`GET /v1/admin/settings`,
/// with the value in force; left out, the config's (true) is.
#[tokio::test]
async fn the_charge_hits_setting_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _db) = harness::app_with_hooks_tweaked(dir.path(), Some(MASTER), Vec::new(), |_| {}).await;
    let (status, _, view) = send(&app, "GET", "/v1/admin/settings", MASTER, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["effective_cache"]["charge_hits"], true, "{view}");

    let (status, _, view) = send(&app, "PUT", "/v1/admin/settings", MASTER, Some(json!({ "cache_charge_hits": false }))).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!((view["settings"]["cache_charge_hits"].clone(), view["effective_cache"]["charge_hits"].clone()), (json!(false), json!(false)), "{view}");
    let (_, _, view) = send(&app, "GET", "/v1/admin/settings", MASTER, None).await;
    assert_eq!((view["settings"]["cache_charge_hits"].clone(), view["effective_cache"]["charge_hits"].clone()), (json!(false), json!(false)), "{view}");

    let (status, _, view) = send(&app, "PUT", "/v1/admin/settings", MASTER, Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert!(view["settings"].get("cache_charge_hits").is_none(), "left out: {view}");
    assert_eq!(view["effective_cache"]["charge_hits"], true, "{view}");
}
