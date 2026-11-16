//! What a billed request reserves, and its `Idempotency-Key`, are held with
//! leases its replica renews, and given back when the replica dies
//! (migration 030; docs/scaling.md "Reservations", docs/RUNBOOK.md).
//!
//! Each test runs over a SQLite file of its own, which a second pool reads
//! and moves on in time. None runs a stop (tests/graceful_stop.rs does).

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

const MASTER: &str = "sk-reservations-master";

fn send(method: &str, uri: &str, bearer: Option<&str>, body: Value) -> Request<Body> {
    let mut req = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(bearer) = bearer {
        req = req.header("authorization", format!("Bearer {bearer}"));
    }
    req.body(Body::from(body.to_string())).unwrap()
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

/// A second pool on the test's SQLite file, to read and move on what litegen wrote.
async fn side_pool(url: &str) -> sqlx::SqlitePool {
    sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect(url).await.unwrap()
}

/// Item 2 (triage 2026-11-14, "Later"): a billed request in flight when its
/// replica dies (here: its runtime goes away with no stop, so its guards'
/// releases never run) leaves what it reserved recorded. Once the records'
/// leases lapse, the next sweep (another replica's, or this one's after its
/// restart) gives the key's quota and its budget back. They used to stay
/// spent for good: the budget until its window rolled, the quota forever.
#[test]
fn a_request_in_flight_when_its_replica_dies_is_given_back_by_the_next_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}?mode=rwc", dir.path().join("crash.db").display());

    // The replica that dies with the request in flight.
    let dying = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let (key_id, price) = dying.block_on(async {
        let db = Arc::new(SqliteDatabase::connect(&url).await.unwrap());
        let app = harness::app_over_with(db.clone(), dir.path(), |_| {}, Some(MASTER), litegen::config::Mode::SingleTenant).await;
        let created = json_of(app.clone().oneshot(send("POST", "/v1/keys", Some(MASTER), json!({ "name": "crash", "token_quota": 10.0 }))).await.unwrap()).await;
        let (key_id, secret) = (created["id"].as_str().unwrap().to_string(), created["key"].as_str().unwrap().to_string());
        let limits = app.clone().oneshot(send("PUT", &format!("/v1/keys/{key_id}/limits"), Some(MASTER), json!({ "max_budget_usd": 5.0 }))).await.unwrap();
        assert_eq!(limits.status(), StatusCode::OK);
        let _request = tokio::spawn(app.clone().oneshot(send(
            "POST",
            "/v1/images/generations",
            Some(&secret),
            json!({ "model": "mock/priced-image", "prompt": "a lighthouse, then a crash job_ms=600000" }),
        )));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut now_held = held(&db, &key_id).await;
        while (now_held.0 == 0.0 || now_held.1 == 0.0) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            now_held = held(&db, &key_id).await;
        }
        assert!(now_held.0 > 0.0 && now_held.1 > 0.0, "the request holds its price while the vendor works: {now_held:?}");
        (key_id, now_held.0)
    });
    drop(dying);

    // The next replica to sweep, two minutes on.
    let next = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    next.block_on(async {
        let db = SqliteDatabase::connect(&url).await.unwrap();
        assert_eq!(held(&db, &key_id).await, (price, price), "the dead replica's request still holds its price");
        let side = side_pool(&url).await;
        sqlx::query("UPDATE reservations SET expires_at = strftime('%Y-%m-%d %H:%M:%f', 'now', '-60.000 seconds')")
            .execute(&side)
            .await
            .unwrap();
        let given_back = litegen::management::reservations::sweep_lapsed(&db).await.unwrap();
        assert_eq!(held(&db, &key_id).await, (0.0, 0.0), "the sweep gave back the dead request's reservations");
        assert_eq!(given_back, 2, "one on the key's quota, one on its limits row");
    });
}

/// Whether `Idempotency-Key` `key`'s record has a holder, and a lease that
/// hasn't lapsed.
async fn lease_of(side: &sqlx::SqlitePool, key: &str) -> (bool, bool) {
    sqlx::query_as::<_, (bool, bool)>(
        "SELECT held_by IS NOT NULL, COALESCE(held_until > strftime('%Y-%m-%d %H:%M:%f', 'now'), 0) \
         FROM idempotency_keys WHERE idem_key = ?",
    )
    .bind(key)
    .fetch_one(side)
    .await
    .unwrap()
}

/// Item 3: a keyed request holds its `Idempotency-Key` with a lease, which
/// its replica renews while the request runs: a request whose replica dies
/// stops renewing, and its retry runs once the lease lapses (2 minutes)
/// instead of answering 409 for 15. A live one keeps its key: renewed past a
/// lapse, it is held again.
#[tokio::test]
async fn a_keyed_request_holds_its_idempotency_key_with_a_lease_it_renews() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}?mode=rwc", dir.path().join("keys.db").display());
    let db = Arc::new(SqliteDatabase::connect(&url).await.unwrap());
    let app = harness::app_over(db.clone(), dir.path(), |_| {}).await;
    let side = side_pool(&url).await;
    let prompt = "a lighthouse that holds its key";

    let first = Request::post("/v1/images/generations")
        .header("content-type", "application/json")
        .header("idempotency-key", "rsv-held")
        .body(Body::from(json!({ "model": "mock/slow-image", "prompt": prompt }).to_string()))
        .unwrap();
    let first = tokio::spawn(app.clone().oneshot(first));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while litegen::providers::image::mock::slow_image_calls(prompt) == 0 {
        assert!(std::time::Instant::now() < deadline, "the vendor never got the call");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(lease_of(&side, "rsv-held").await, (true, true), "the request holds its key with a lease");

    // Two minutes on, the lease has lapsed; the replica's next renewal holds it again.
    sqlx::query("UPDATE idempotency_keys SET held_until = strftime('%Y-%m-%d %H:%M:%f', 'now', '-60.000 seconds') WHERE idem_key = 'rsv-held'")
        .execute(&side)
        .await
        .unwrap();
    assert_eq!(lease_of(&side, "rsv-held").await, (true, false));
    litegen::management::reservations::renew_live(&*db).await.unwrap();
    assert_eq!(lease_of(&side, "rsv-held").await, (true, true), "a live request's renewal holds its key again");

    litegen::providers::image::mock::release_slow_image(prompt);
    let answered = first.await.unwrap().unwrap();
    assert_eq!(answered.status(), StatusCode::OK);
}
