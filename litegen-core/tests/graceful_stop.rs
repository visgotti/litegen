//! A graceful stop gives back what the billed requests still running hold
//! (src/shutdown.rs, docs/RUNBOOK.md "Graceful shutdown not happening").
//!
//! One test, in a binary of its own: the stop it runs is the process's, and it
//! cuts every billed request the process is running.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::DatabaseStore;
use litegen::management::types::EntityType;
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs: the real create_router over the given SQLite

const MASTER: &str = "sk-stop-master";

fn send(method: &str, uri: &str, bearer: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {bearer}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn json_of(resp: axum::response::Response) -> Value {
    serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap_or(Value::Null)
}

/// What key `key_id` holds now: its quota's `tokens_used`, and its limits' spend.
async fn held(db: &SqliteDatabase, key_id: &str) -> (f64, f64) {
    let tokens = db.get_api_key(&key_id.parse().unwrap()).await.unwrap().unwrap().tokens_used;
    let spend = db.management().unwrap().get_limits(EntityType::Key, key_id).await.unwrap().unwrap().spend_usd;
    (tokens, spend)
}

/// E12 x P11 (triage 2026-11-14, "Later"): a billed request still running when
/// the grace ends gives back what it reserved. Its connection is cut, but the
/// request runs in a task of its own, which the runtime dropped once `main`
/// returned: the releases its guards spawned as they dropped never ran, and
/// the key's quota and its budget stayed spent on nothing for good.
#[test]
fn a_billed_request_in_flight_at_the_stop_leaves_no_reservation_behind() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}?mode=rwc", dir.path().join("stop.db").display());

    // The server's runtime, dropped as main's is once `serve` has returned.
    let serving = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let key_id = serving.block_on(async {
        let db = Arc::new(SqliteDatabase::connect(&url).await.unwrap());
        let app = harness::app_over_with(db.clone(), dir.path(), |_| {}, Some(MASTER), litegen::config::Mode::SingleTenant).await;
        let created = json_of(app.clone().oneshot(send("POST", "/v1/keys", MASTER, json!({ "name": "stop", "token_quota": 10.0 }))).await.unwrap()).await;
        let (key_id, secret) = (created["id"].as_str().unwrap().to_string(), created["key"].as_str().unwrap().to_string());
        let limits = app.clone().oneshot(send("PUT", &format!("/v1/keys/{key_id}/limits"), MASTER, json!({ "max_budget_usd": 5.0 }))).await.unwrap();
        assert_eq!(limits.status(), StatusCode::OK);

        // A billed request whose vendor is still working at the stop: the
        // mock runs it as a job that takes 10 minutes.
        let _request = tokio::spawn(app.clone().oneshot(send(
            "POST",
            "/v1/images/generations",
            &secret,
            json!({ "model": "mock/priced-image", "prompt": "a lighthouse at the stop job_ms=600000" }),
        )));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut now_held = held(&db, &key_id).await;
        while (now_held.0 == 0.0 || now_held.1 == 0.0) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            now_held = held(&db, &key_id).await;
        }
        assert!(now_held.0 > 0.0 && now_held.1 > 0.0, "the request holds its price while the vendor works: {now_held:?}");

        // The stop, as main runs it once its grace is over.
        let finished = litegen::shutdown::finish(tokio::time::Instant::now(), Duration::from_secs(5)).await;
        println!("the stop: {finished:?}");
        key_id
    });
    drop(serving);

    let checking = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    checking.block_on(async {
        let db = SqliteDatabase::connect(&url).await.unwrap();
        let (tokens, spend) = held(&db, &key_id).await;
        assert_eq!(
            (tokens, spend),
            (0.0, 0.0),
            "a request in flight at the stop left {tokens} USD reserved on the key's quota and {spend} on its budget"
        );
    });
}
