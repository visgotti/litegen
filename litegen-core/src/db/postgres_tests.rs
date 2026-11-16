//! Postgres-backed regression tests for the dashboard stats queries.
//!
//! These are gated on `LITEGEN_PG_TEST_URL` because they need a live Postgres:
//! SQLite can't reproduce Postgres's strict typing. In particular `AVG(<bigint>)`
//! returns `NUMERIC` in Postgres, which sqlx refuses to decode into `f64`
//! (SQLite's `AVG` returns `REAL`, so the SQLite suite is blind to this class of
//! bug). When the env var is unset these tests no-op, keeping the default
//! SQLite-only `cargo test` run green.
//!
//! Run against a throwaway database, e.g.:
//!   createdb litegen_stats_test
//!   LITEGEN_PG_TEST_URL=postgres://localhost:5432/litegen_stats_test \
//!       cargo test --lib db::postgres_tests
//!   dropdb litegen_stats_test

use super::{DatabaseStore, PostgresDatabase};
use crate::api::middleware::{DEFAULT_APP_ID, DEFAULT_ORG_ID};
use uuid::Uuid;

/// Regression for the Overview-page decode error seen right after login:
///   "error occurred while decoding column 4: mismatched types; Rust type `f64`
///    (as SQL type `FLOAT8`) is not compatible with SQL type `NUMERIC`"
///
/// `AVG(latency_ms)` over the `BIGINT` `latency_ms` column returns `NUMERIC` in
/// Postgres, and `COALESCE(AVG(...), 0.0)` stays `NUMERIC`. The stats queries
/// decode that column into `f64`, so without a `::double precision` cast in the
/// SQL, `get_stats` / `get_stats_for_tenant` return Err and the dashboard shows a
/// red banner. This test fails (decode error) on the unpatched queries and passes
/// once the casts are in place.
#[tokio::test]
async fn stats_queries_decode_avg_latency_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!(
            "skipping stats_queries_decode_avg_latency_against_postgres: \
             set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run"
        );
        return;
    };

    let db = PostgresDatabase::connect(&url)
        .await
        .expect("connect + migrate against the test Postgres");

    // One completed request with a real, non-zero latency so AVG(latency_ms)
    // produces an actual NUMERIC value (not merely NULL→0.0). org_id/app_id left
    // as None default to the migration-seeded Default org/app, satisfying the FKs.
    db.log_request(
        &Uuid::new_v4().to_string(),
        "test-model",
        "test-provider",
        "completed",
        "image",
        0.25,
        120,
        None,
        None,
        None,
        None,
    )
    .await
    .expect("insert request_log");

    // Global Overview stats — must decode, not 500. Exercises the totals, model,
    // and provider queries (all three carry an AVG(latency_ms) column).
    let global = db
        .get_stats()
        .await
        .expect("get_stats must decode AVG(latency_ms) NUMERIC into f64");
    assert!(global.total_requests >= 1, "inserted row should be counted");
    assert!(global.avg_latency_ms > 0.0, "avg latency should reflect the 120ms row");
    assert!(
        global.providers_used.iter().any(|p| p.provider == "test-provider"),
        "provider stats query must decode and surface the inserted provider",
    );
    assert!(
        global.models_used.iter().any(|m| m.model == "test-model"),
        "model stats query must decode and surface the inserted model",
    );

    // Tenant-scoped Overview stats — the exact path that errored after login.
    let by_org = db
        .get_stats_for_tenant(DEFAULT_ORG_ID, None, &[])
        .await
        .expect("get_stats_for_tenant (org) must decode AVG(latency_ms)");
    assert!(by_org.total_requests >= 1);
    assert!(by_org.avg_latency_ms > 0.0);

    let by_app = db
        .get_stats_for_tenant(DEFAULT_ORG_ID, Some(DEFAULT_APP_ID), &[])
        .await
        .expect("get_stats_for_tenant (org+app) must decode AVG(latency_ms)");
    assert!(by_app.total_requests >= 1);
    assert!(by_app.avg_latency_ms > 0.0);
}

/// Regression for a sibling of the stats bug, on the Generations tab.
///
/// `generations.cost_usd` was declared `REAL` (float4, 4-byte) in
/// `20240101000003_generations.sql`, while `GenerationRow.cost_usd` is `f64` —
/// which sqlx maps strictly to `FLOAT8` (8-byte). sqlx refuses to decode a
/// `FLOAT4` column into `f64` ("Rust type `f64` (as SQL type `FLOAT8`) is not
/// compatible with SQL type `FLOAT4`"), so every generation decode path
/// (get / list / cancel / tenant-list) 500s as soon as one generation row
/// exists. Migration `20240101000011_generations_cost_usd_to_float8.sql` widens
/// the column to `DOUBLE PRECISION` to match `request_logs.cost_usd` and `f64`.
///
/// This test fails (FLOAT4↛f64 decode error) on the unmigrated column and passes
/// once the widening migration is applied — including against an already-migrated
/// database, which is exactly the production scenario the ALTER must handle.
#[tokio::test]
async fn generations_decode_cost_usd_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!(
            "skipping generations_decode_cost_usd_against_postgres: \
             set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run"
        );
        return;
    };

    let db = PostgresDatabase::connect(&url)
        .await
        .expect("connect + migrate against the test Postgres");

    // org_id/app_id left None default to the seeded Default org/app; key_id None
    // is allowed (nullable FK to api_keys).
    let id = Uuid::new_v4().to_string();
    db.insert_generation(&id, None, "test-model", "test-provider", "image", None, 0.5, None, None)
        .await
        .expect("insert_generation");

    // The exact FromRow decode path the Generations dashboard tab / API hits.
    let got = db
        .get_generation(&id)
        .await
        .expect("get_generation must decode generations.cost_usd (REAL) into f64")
        .expect("the inserted generation row should exist");
    assert!((got.cost_usd - 0.5).abs() < 1e-6, "cost_usd should round-trip");

    // list_generations exercises the same decode over a Vec of rows.
    let listed = db
        .list_generations(None, 1, 20)
        .await
        .expect("list_generations must decode generations.cost_usd");
    assert!(listed.iter().any(|g| g.id == id), "inserted generation should be listed");
}

/// `org_provider_credentials.created_at`/`updated_at` were created by
/// `20240101000013` as `timestamp without time zone`, but `ProviderCredentialRow`
/// decodes `DateTime<Utc>` (sqlx → strict `timestamptz`). `list_org_provider_credentials`
/// 500s ("TIMESTAMPTZ … not compatible with SQL type TIMESTAMP") on the unmigrated
/// column — breaking the Providers dashboard tab. `20240101000014` re-sweeps the
/// remaining timestamp columns to timestamptz.
///
/// This test fails (TIMESTAMP↛DateTime<Utc> decode error) on the unmigrated column
/// and passes once the sweep is applied — including against an already-migrated
/// database, the production scenario the ALTER must handle idempotently.
#[tokio::test]
async fn org_provider_credentials_list_decodes_created_at_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!(
            "skipping org_provider_credentials_list_decodes_created_at_against_postgres: \
             set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run"
        );
        return;
    };

    let db = PostgresDatabase::connect(&url)
        .await
        .expect("connect + migrate against the test Postgres");

    // Store a credential under the seeded Default org (created on connect).
    let org_id = crate::api::middleware::DEFAULT_ORG_ID;
    db.upsert_org_provider_credential(org_id, "fal", "ciphertext", "nonce", Some("…1234"))
        .await
        .expect("upsert_org_provider_credential");

    // The exact FromRow decode path the Providers dashboard tab / API hits.
    let listed = db
        .list_org_provider_credentials(org_id)
        .await
        .expect("list_org_provider_credentials must decode created_at (timestamptz) into DateTime<Utc>");
    assert!(
        listed.iter().any(|c| c.provider == "fal"),
        "the stored fal credential should be listed"
    );
}

/// `update_request_log_status` is the only UPDATE on `request_logs`, with its
/// own `$n` placeholder list — a Postgres-only mistake there is invisible to
/// the SQLite suite, and would leave every async request `pending` again.
#[tokio::test]
async fn request_log_status_update_round_trips_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!(
            "skipping request_log_status_update_round_trips_against_postgres: \
             set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run"
        );
        return;
    };

    let db = PostgresDatabase::connect(&url)
        .await
        .expect("connect + migrate against the test Postgres");

    let id = Uuid::new_v4().to_string();
    // 999_999 ms (~16 min) is a submit-time latency no test run can reach, so
    // the async re-stamp below shows up without pinning a wall-clock number.
    let started = std::time::Instant::now();
    db.log_request(&id, "test-model", "test-provider", "pending", "model3d", 0.0, 999_999, None, None, None, None)
        .await
        .expect("insert request_log");
    // A sync row (never `pending`) must come out of the same UPDATE untouched.
    let sync_id = Uuid::new_v4().to_string();
    db.log_request(&sync_id, "test-model", "test-provider", "completed", "image", 0.0, 1234, None, None, None, None)
        .await
        .expect("insert sync request_log");

    db.update_request_log_status(&id, "failed", Some("provider error"))
        .await
        .expect("update_request_log_status");
    db.update_request_log_status(&sync_id, "failed", Some("provider error"))
        .await
        .expect("update_request_log_status (sync row)");

    let filters = crate::types::LogFilters { status: Some("failed".to_string()), ..Default::default() };
    let (logs, _) = db
        .get_request_logs_filtered(&filters, 1, 1000)
        .await
        .expect("get_request_logs_filtered");
    let row = logs.iter().find(|l| l.id == id).expect("the updated row is listed as failed");
    assert_eq!(row.status, crate::types::GenerationStatus::Failed);
    assert_eq!(row.error.as_deref(), Some("provider error"));
    // Settling an ASYNC row re-stamps `latency_ms` with the end-to-end time
    // (NOW() − created_at, both full-precision here) so `/v1/stats` percentiles
    // keep one meaning; `created_at` is NOW() at insert, so the value can only
    // be the time this test itself took.
    let ceiling = started.elapsed().as_millis() as u64 + 2_000;
    assert!(
        row.latency_ms <= ceiling,
        "a settled pending row carries its end-to-end latency, not the 999_999ms submit stamp; got {}",
        row.latency_ms
    );
    let sync_row = logs.iter().find(|l| l.id == sync_id).expect("the sync row is listed as failed");
    assert_eq!(
        sync_row.latency_ms, 1234,
        "a row that was never `pending` keeps the latency its handler measured"
    );
}

/// The compare-and-swap metadata writes in their Postgres spelling
/// (`IS NOT DISTINCT FROM`, `$n` placeholders): merge keeps what submit
/// recorded, a terminal row is never touched, and racing claimants of one
/// stage produce exactly one winner.
#[tokio::test]
async fn metadata_merge_and_stage_claims_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!(
            "skipping metadata_merge_and_stage_claims_against_postgres: \
             set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run"
        );
        return;
    };
    let db = std::sync::Arc::new(
        PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"),
    );

    let id = format!("litegen-3d-{}", Uuid::new_v4());
    db.insert_generation(&id, None, "mock/mesh-3d", "mock", "model3d", Some("job"), 0.0, None, None)
        .await
        .expect("insert");
    db.merge_generation_metadata_if_active(&id, &serde_json::json!({ "request": { "prompt": "a chair" }, "keep": 1 }))
        .await
        .expect("merge");
    db.merge_generation_metadata_if_active(&id, &serde_json::json!({ "keep": null }))
        .await
        .expect("merge a removal");

    let mut tasks = Vec::new();
    for i in 0..8 {
        let db = db.clone();
        let id = id.clone();
        tasks.push(tokio::spawn(async move { db.claim_generation_stage(&id, "rig", &format!("o{i}")).await.unwrap() }));
    }
    let mut wins = 0;
    for t in tasks {
        if t.await.unwrap() {
            wins += 1;
        }
    }
    assert_eq!(wins, 1, "exactly one claimant starts the stage");

    let w = db
        .update_generation_if_active(
            &id, "completed", 100, Some("https://x/model.glb"), None, Some(chrono::Utc::now()),
            Some(&serde_json::json!({ "assets": [{ "kind": "mesh" }] })),
        )
        .await
        .expect("terminal write");
    assert_eq!(w, crate::db::GenerationWrite::Written);
    let m = db.get_generation(&id).await.unwrap().unwrap().metadata.unwrap();
    assert_eq!(m["request"]["prompt"], "a chair", "{m}");
    assert!(m.get("keep").is_none(), "{m}");
    assert!(m["stage_claims"]["rig"]["by"].is_string(), "{m}");
    assert_eq!(m["assets"][0]["kind"], "mesh", "{m}");

    let late = db.merge_generation_metadata_if_active(&id, &serde_json::json!({ "late": 1 })).await.unwrap();
    assert_eq!(late, crate::db::GenerationWrite::AlreadyTerminal);
    assert!(!db.claim_generation_stage(&id, "refine", "poller").await.unwrap());
}

/// The poller's skip warning (migration 035) in its Postgres spelling: each
/// 10-minute mark of a row's age is noted once, by one of several pollers
/// racing at it, a later mark again, never an earlier one, and a row no
/// longer in flight never.
#[tokio::test]
async fn a_skip_warning_is_noted_once_per_mark_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_skip_warning_is_noted_once_per_mark_against_postgres: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let db = std::sync::Arc::new(PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
    let id = format!("litegen-vid-{}", Uuid::new_v4());
    db.insert_generation(&id, None, "mock/video-gen", "mock", "video", Some("job"), 0.0, None, None).await.expect("insert");

    let mut racing = Vec::new();
    for _ in 0..8 {
        let (db, id) = (db.clone(), id.clone());
        racing.push(tokio::spawn(async move { db.note_poll_skip_warning(&id, 0).await.unwrap() }));
    }
    let mut warned = 0;
    for r in racing {
        warned += usize::from(r.await.unwrap());
    }
    assert_eq!(warned, 1, "one of the pollers racing at the first skip warns");
    assert!(!db.note_poll_skip_warning(&id, 0).await.unwrap(), "the same mark again");
    assert!(db.note_poll_skip_warning(&id, 2).await.unwrap(), "a later mark");
    assert!(!db.note_poll_skip_warning(&id, 1).await.unwrap(), "an earlier mark than the last warned");
    db.cancel_generation(&id).await.unwrap();
    assert!(!db.note_poll_skip_warning(&id, 3).await.unwrap(), "a row that ended");
}

/// Idempotency-Key records in their Postgres spelling (BYTEA body, `$n`
/// placeholders, TIMESTAMPTZ ages): claim, replay, conflict, release and
/// racing first requests.
/// The delivery lease in its Postgres spelling: one holder among racing
/// claimants, its own re-take, release by the holder only, a lapsed lease
/// taken over, and none on a terminal row.
#[tokio::test]
async fn work_leases_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!(
            "skipping work_leases_against_postgres: \
             set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run"
        );
        return;
    };
    let db = std::sync::Arc::new(
        PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"),
    );
    let lease = chrono::Duration::minutes(5);
    let id = format!("litegen-lease-{}", Uuid::new_v4());
    db.insert_generation(&id, None, "mock/mesh-3d", "mock", "model3d", Some("job"), 0.0, None, None)
        .await
        .expect("insert");

    let mut tasks = Vec::new();
    for i in 0..8 {
        let db = db.clone();
        let id = id.clone();
        tasks.push(tokio::spawn(async move { db.lease_generation_work(&id, "delivery", &format!("o{i}"), lease).await.unwrap() }));
    }
    let mut holders = 0;
    for t in tasks {
        if t.await.unwrap() {
            holders += 1;
        }
    }
    assert_eq!(holders, 1, "exactly one claimant holds the lease");
    let held_by = db.get_generation(&id).await.unwrap().unwrap().metadata.unwrap()["leases"]["delivery"]["by"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(db.lease_generation_work(&id, "delivery", &held_by, lease).await.unwrap(), "its holder may take it again");
    db.release_generation_work(&id, "delivery", "someone-else").await.unwrap();
    assert!(!db.lease_generation_work(&id, "delivery", "late", lease).await.unwrap(), "only the holder releases it");
    db.release_generation_work(&id, "delivery", &held_by).await.unwrap();
    assert!(db.lease_generation_work(&id, "delivery", "late", lease).await.unwrap(), "a released lease is free");
    assert!(db.lease_generation_work(&id, "delivery", "later", chrono::Duration::zero()).await.unwrap(), "a lapsed one too");

    db.update_generation_if_active(&id, "completed", 100, None, None, Some(chrono::Utc::now()), None)
        .await
        .expect("terminal write");
    assert!(!db.lease_generation_work(&id, "delivery", "after", lease).await.unwrap(), "a settled row's work is done");
}

#[tokio::test]
async fn idempotency_records_against_postgres() {
    use crate::db::{IdempotencyBegin, StoredResponse};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!(
            "skipping idempotency_records_against_postgres: \
             set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run"
        );
        return;
    };
    let db = std::sync::Arc::new(PostgresDatabase::connect(&url).await.expect("connect + migrate"));
    let scope = format!("org:{}", Uuid::new_v4());

    let lease = crate::management::reservations::LEASE;
    let mut tasks = Vec::new();
    for i in 0..8 {
        let (db, scope) = (db.clone(), scope.clone());
        tasks.push(tokio::spawn(async move {
            let held_by = format!("req-{i}");
            (db.idempotency_begin_held(&scope, "k", "h", &held_by, lease).await.unwrap(), held_by)
        }));
    }
    let mut started = Vec::new();
    for t in tasks {
        let (begun, held_by) = t.await.unwrap();
        if matches!(begun, IdempotencyBegin::Started) {
            started.push(held_by);
        }
    }
    assert_eq!(started.len(), 1);

    let stored = StoredResponse { status: 504, headers: vec![("content-type".into(), "application/json".into())], body: b"{\"x\":1}".to_vec() };
    db.idempotency_complete_held(&scope, "k", &started[0], &stored).await.unwrap();
    assert_eq!(db.idempotency_begin_held(&scope, "k", "h", "req-x", lease).await.unwrap(), IdempotencyBegin::Replay(stored));
    assert_eq!(db.idempotency_begin_held(&scope, "k", "other", "req-y", lease).await.unwrap(), IdempotencyBegin::Conflict);

    assert_eq!(db.idempotency_begin_held(&scope, "fresh", "h", "req-fresh", lease).await.unwrap(), IdempotencyBegin::Started);
    db.idempotency_release_held(&scope, "fresh", "req-fresh").await.unwrap();
    assert_eq!(db.idempotency_begin_held(&scope, "fresh", "h2", "req-again", lease).await.unwrap(), IdempotencyBegin::Started);
}

/// L17 and L18 against Postgres, where `request_logs.metadata` is TEXT: the
/// metadata filters cast it to `jsonb`, with their placeholders numbered after
/// the tenant's; a refund is claimed once, and only for a job that ended.
#[tokio::test]
async fn metadata_filters_and_refund_claims_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping metadata_filters_and_refund_claims_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let project = format!("castle-{}", Uuid::new_v4());
    let (castle, forest) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    let tags = serde_json::json!({ "project": project, "asset": "knight" });
    db.log_request(&castle, "m", "p", "completed", "video", 0.5, 10, None, Some(&tags), None, None).await.unwrap();
    db.log_request(&forest, "m", "p", "completed", "video", 0.25, 10, None, Some(&serde_json::json!({ "project": "forest" })), None, None)
        .await
        .unwrap();

    let pairs = vec![("project".to_string(), project.clone()), ("asset".to_string(), "knight".to_string())];
    let filter = crate::db::RequestLogFilter { metadata: &pairs, ..Default::default() };
    let (rows, total) = db.get_request_logs_for_tenant_filtered(DEFAULT_ORG_ID, None, &filter, 1, 50).await.expect("filtered logs");
    assert_eq!((total, rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>()), (1, vec![castle.as_str()]));
    for app in [None, Some(DEFAULT_APP_ID)] {
        let stats = db.get_stats_for_tenant(DEFAULT_ORG_ID, app, &pairs).await.expect("filtered stats");
        assert_eq!((stats.total_requests, stats.total_cost_usd), (1, 0.5), "app {app:?}");
    }

    let id = Uuid::new_v4().to_string();
    db.log_request(&id, "m", "p", "pending", "video", 0.5, 10, None, None, None, None).await.unwrap();
    db.insert_generation(&id, None, "m", "p", "video", None, 0.5, None, None).await.unwrap();
    assert!(!db.claim_generation_refund(&id, 0.5).await.unwrap(), "a job still running is not refunded");
    db.cancel_generation(&id).await.unwrap().expect("cancelled");
    assert!(db.claim_generation_refund(&id, 0.5).await.unwrap(), "a cancelled one is");
    assert!(!db.claim_generation_refund(&id, 0.5).await.unwrap(), "once");
    assert_eq!(db.get_generation(&id).await.unwrap().unwrap().cost_usd, 0.0);
    let filter = crate::types::LogFilters::default();
    let (logs, _) = db.get_request_logs_filtered(&filter, 1, 1000).await.unwrap();
    assert_eq!(logs.iter().find(|l| l.id == id).unwrap().cost_usd, 0.0, "its log too");
}

/// Per-app access settings round-trip, and go with their app and org: the
/// app_id column references `applications` without a cascade, so a missing
/// child delete would fail `delete_application` here (sqlite has no FKs).
#[tokio::test]
async fn app_file_access_round_trips_and_goes_with_its_app_against_postgres() {
    use crate::types::{AppFileAccessUpsert, Application, Organization};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping app_file_access_round_trips_and_goes_with_its_app_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let now = chrono::Utc::now();
    let org = Organization {
        id: Uuid::new_v4().to_string(),
        name: "files".into(),
        slug: format!("files-{}", Uuid::new_v4().simple()),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_organization(&org).await.unwrap();
    let app = |slug: &str| Application {
        id: Uuid::new_v4().to_string(),
        org_id: org.id.clone(),
        name: slug.into(),
        slug: slug.into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    let (a1, a2) = (app("web"), app("web2"));
    db.create_application(&a1).await.unwrap();
    db.create_application(&a2).await.unwrap();
    let put = |app_id: &str, mode: Option<&str>, ttl: Option<i64>| AppFileAccessUpsert {
        app_id: app_id.into(),
        mode: mode.map(str::to_string),
        owner_scope: None,
        signed_url_ttl_seconds: ttl,
        keep_outputs: None,
        retention_days: None,
        signed_links: None,
    };
    assert!(db.get_app_file_access(&a1.id).await.unwrap().is_none());
    db.put_app_file_access(&put(&a1.id, Some("signed"), Some(900))).await.unwrap();
    db.put_app_file_access(&put(&a1.id, Some("private"), None)).await.unwrap();
    let row = db.get_app_file_access(&a1.id).await.unwrap().expect("stored");
    assert_eq!((row.mode.as_deref(), row.signed_url_ttl_seconds), (Some("private"), None));
    // Keeping, retention and links round-trip, and are replaced with the rest.
    db.put_app_file_access(&AppFileAccessUpsert {
        keep_outputs: Some(false),
        retention_days: Some(30),
        signed_links: Some("litegen".into()),
        ..put(&a1.id, None, None)
    })
    .await
    .unwrap();
    let row = db.get_app_file_access(&a1.id).await.unwrap().expect("stored");
    assert_eq!((row.keep_outputs, row.retention_days, row.signed_links.as_deref()), (Some(false), Some(30), Some("litegen")));
    db.put_app_file_access(&put(&a1.id, Some("private"), None)).await.unwrap();
    let row = db.get_app_file_access(&a1.id).await.unwrap().expect("stored");
    assert_eq!((row.keep_outputs, row.retention_days, row.signed_links.as_deref()), (None, None, None));
    assert!(db.delete_application(&a1.id).await.unwrap(), "the app deletes with its access row");
    assert!(db.get_app_file_access(&a1.id).await.unwrap().is_none());
    db.put_app_file_access(&put(&a2.id, Some("signed"), None)).await.unwrap();
    assert!(db.delete_organization(&org.id).await.unwrap(), "the org deletes with its apps' access rows");
    assert!(db.get_app_file_access(&a2.id).await.unwrap().is_none());
}

#[tokio::test]
async fn link_revocations_take_the_latest_scope_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping link_revocations_take_the_latest_scope_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    // Fresh ids: the database outlives a run. The gateway's row is shared, so
    // it is left to the SQLite test.
    let (a, b, g, h) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());
    let t = |secs: i64| chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).unwrap();
    let gateway = db.links_revoked_at(None, &g).await.unwrap();
    let only = |v: Option<chrono::DateTime<chrono::Utc>>| v.max(gateway);
    assert_eq!(db.links_revoked_at(Some(&a), &g).await.unwrap(), gateway);
    db.revoke_links("app", &a, t(1_000)).await.unwrap();
    assert_eq!(db.links_revoked_at(Some(&a), &g).await.unwrap(), only(Some(t(1_000))));
    assert_eq!(db.links_revoked_at(Some(&b), &g).await.unwrap(), gateway, "another app's");
    db.revoke_links("generation", &g, t(2_000)).await.unwrap();
    assert_eq!(db.links_revoked_at(Some(&a), &g).await.unwrap(), only(Some(t(2_000))));
    assert_eq!(db.links_revoked_at(Some(&a), &h).await.unwrap(), only(Some(t(1_000))));
    db.revoke_links("app", &a, t(3_000)).await.unwrap();
    assert_eq!(db.links_revoked_at(Some(&a), &g).await.unwrap(), only(Some(t(3_000))));
    db.revoke_links("app", &a, t(500)).await.unwrap();
    assert_eq!(db.links_revoked_at(Some(&a), &h).await.unwrap(), only(Some(t(3_000))), "an earlier time never un-revokes");
}

#[tokio::test]
async fn generations_due_for_retention_respect_app_cutoffs_against_postgres() {
    use crate::types::{AppFileAccessUpsert, Application, Organization};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping generations_due_for_retention_respect_app_cutoffs_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let now = chrono::Utc::now();
    let tag = uuid::Uuid::new_v4().to_string();
    let org = Organization {
        id: uuid::Uuid::new_v4().to_string(),
        name: format!("keeps-{tag}"),
        slug: format!("keeps-{tag}"),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_organization(&org).await.unwrap();
    let app = |name: &str| Application {
        id: uuid::Uuid::new_v4().to_string(),
        org_id: org.id.clone(),
        name: name.into(),
        slug: format!("{name}-{tag}"),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    let (a, b) = (app("a"), app("b"));
    db.create_application(&a).await.unwrap();
    db.create_application(&b).await.unwrap();
    let (ga, gb) = (format!("g-a-{tag}"), format!("g-b-{tag}"));
    for (id, app_id) in [(&ga, &a.id), (&gb, &b.id)] {
        db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, Some(&org.id), Some(app_id)).await.unwrap();
        db.update_generation_if_active(id, "completed", 100, None, None, Some(now), None).await.unwrap();
    }
    db.put_app_file_access(&AppFileAccessUpsert {
        app_id: a.id.clone(), mode: None, owner_scope: None, signed_url_ttl_seconds: None,
        keep_outputs: None, retention_days: Some(1), signed_links: None,
    }).await.unwrap();
    assert!(db.apps_with_retention().await.unwrap().contains(&(a.id.clone(), 1)));
    let later = now + chrono::Duration::days(2);
    let ids = |rows: Vec<crate::types::Generation>| rows.into_iter().map(|g| g.id).collect::<Vec<_>>();
    assert_eq!(ids(db.generations_due_for_retention(Some(&a.id), later, 10).await.unwrap()), std::slice::from_ref(&ga));
    let rest = ids(db.generations_due_for_retention(None, later, 100_000).await.unwrap());
    assert!(rest.contains(&gb) && !rest.contains(&ga), "app b follows the default; a has its own");
    db.mark_files_deleted(&ga, now, "retention").await.unwrap();
    assert!(db.generations_due_for_retention(Some(&a.id), later, 10).await.unwrap().is_empty());
    let g = db.get_generation(&ga).await.unwrap().unwrap();
    assert_eq!(g.files_deleted_reason.as_deref(), Some("retention"));
}

/// A row whose files the sweep failed to delete goes behind the rows not yet
/// tried (P12), so rows that keep failing can't fill every batch.
#[tokio::test]
async fn a_row_whose_delete_failed_goes_behind_the_others_against_postgres() {
    use crate::types::{Application, Organization};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_row_whose_delete_failed_goes_behind_the_others_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let now = chrono::Utc::now();
    let tag = uuid::Uuid::new_v4().to_string();
    let org = Organization {
        id: uuid::Uuid::new_v4().to_string(),
        name: format!("fails-{tag}"),
        slug: format!("fails-{tag}"),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_organization(&org).await.unwrap();
    let app = Application {
        id: uuid::Uuid::new_v4().to_string(),
        org_id: org.id.clone(),
        name: "a".into(),
        slug: format!("a-{tag}"),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_application(&app).await.unwrap();
    let (old, new) = (format!("g-old-{tag}"), format!("g-new-{tag}"));
    for (id, days) in [(&old, 3_i32), (&new, 2)] {
        db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, Some(&org.id), Some(&app.id)).await.unwrap();
        db.update_generation_if_active(id, "completed", 100, None, None, Some(now), None).await.unwrap();
        sqlx::query("UPDATE generations SET created_at = now() - make_interval(days => $2) WHERE id = $1")
            .bind(id)
            .bind(days)
            .execute(db.pool())
            .await
            .unwrap();
    }
    let ids = |rows: Vec<crate::types::Generation>| rows.into_iter().map(|g| g.id).collect::<Vec<_>>();
    assert_eq!(ids(db.generations_due_for_retention(Some(&app.id), now, 1).await.unwrap()), std::slice::from_ref(&old), "oldest first");

    db.note_files_delete_failed(&old).await.unwrap();
    assert_eq!(ids(db.generations_due_for_retention(Some(&app.id), now, 1).await.unwrap()), std::slice::from_ref(&new), "the failed row goes behind");
    assert_eq!(ids(db.generations_due_for_retention(Some(&app.id), now, 10).await.unwrap()), [new.clone(), old.clone()], "and is still due");
}

/// Security fix 3 (2026-11-05): `transfer_deployment_owner` moves the
/// single-tenant Owner in both places it is recorded (`users.role` and the
/// default org's membership) in one transaction, or nowhere.
#[tokio::test]
async fn transfer_deployment_owner_moves_both_places_or_neither_against_postgres() {
    use crate::types::{Organization, Role, User};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping transfer_deployment_owner_moves_both_places_or_neither_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let _turn = DEPLOYMENT_OWNER_TESTS.lock().await;
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let now = chrono::Utc::now();
    let org = Organization {
        id: Uuid::new_v4().to_string(),
        name: "deploy".into(),
        slug: format!("deploy-{}", Uuid::new_v4().simple()),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_organization(&org).await.unwrap();
    let user = |role: Role| User {
        id: Uuid::new_v4().to_string(),
        email: format!("{}@deploy.test", Uuid::new_v4().simple()),
        password_hash: None,
        role,
        oauth_github_id: None,
        oauth_google_id: None,
        created_at: now,
        updated_at: now,
        last_login_at: None,
        is_active: true,
        sign_in_epoch: 0,
    };
    let (owner, admin, outsider) = (user(Role::Owner), user(Role::Admin), user(Role::Member));
    for u in [&owner, &admin, &outsider] {
        db.create_user(u).await.unwrap();
    }
    db.add_org_member(&org.id, &owner.id, Role::Owner).await.unwrap();
    db.add_org_member(&org.id, &admin.id, Role::Admin).await.unwrap();

    // Someone with no membership can't take it, and nothing moves.
    assert!(matches!(db.transfer_deployment_owner(&org.id, &outsider.id).await, Err(sqlx::Error::RowNotFound)));
    assert_eq!(db.get_membership(&org.id, &owner.id).await.unwrap(), Some(Role::Owner));
    assert_eq!(db.get_user_by_id(&owner.id).await.unwrap().unwrap().role, Role::Owner);
    assert_eq!(db.get_user_by_id(&outsider.id).await.unwrap().unwrap().role, Role::Member);

    // A member takes it, in both places.
    db.transfer_deployment_owner(&org.id, &admin.id).await.unwrap();
    assert_eq!(db.get_membership(&org.id, &admin.id).await.unwrap(), Some(Role::Owner));
    assert_eq!(db.get_membership(&org.id, &owner.id).await.unwrap(), Some(Role::Admin));
    assert_eq!(db.get_user_by_id(&admin.id).await.unwrap().unwrap().role, Role::Owner);
    assert_eq!(db.get_user_by_id(&owner.id).await.unwrap().unwrap().role, Role::Admin);
}

/// The tests that run `transfer_deployment_owner` take turns: it rewrites
/// `users.role` across the whole database, and each checks the roles it set up.
static DEPLOYMENT_OWNER_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A new org with an Owner and an Admin, as (org, owner, admin) ids.
async fn org_with_an_owner_and_an_admin(db: &PostgresDatabase, name: &str) -> (String, String, String) {
    use crate::types::{Organization, Role, User};
    let now = chrono::Utc::now();
    let org = Organization {
        id: Uuid::new_v4().to_string(),
        name: name.into(),
        slug: format!("{name}-{}", Uuid::new_v4().simple()),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_organization(&org).await.unwrap();
    let user = |role: Role| User {
        id: Uuid::new_v4().to_string(),
        email: format!("{}@{name}.test", Uuid::new_v4().simple()),
        password_hash: None,
        role,
        oauth_github_id: None,
        oauth_google_id: None,
        created_at: now,
        updated_at: now,
        last_login_at: None,
        is_active: true,
        sign_in_epoch: 0,
    };
    let (owner, admin) = (user(Role::Owner), user(Role::Admin));
    for u in [&owner, &admin] {
        db.create_user(u).await.unwrap();
    }
    db.add_org_member(&org.id, &owner.id, Role::Owner).await.unwrap();
    db.add_org_member(&org.id, &admin.id, Role::Admin).await.unwrap();
    (org.id, owner.id, admin.id)
}

/// Starts an Owner transfer of `org_id` to `to`: the deployment's when
/// `deployment`, else the org's.
fn start_transfer(
    db: &std::sync::Arc<PostgresDatabase>,
    deployment: bool,
    org_id: &str,
    to: &str,
) -> tokio::task::JoinHandle<Result<(), sqlx::Error>> {
    let (db, org_id, to) = (db.clone(), org_id.to_string(), to.to_string());
    tokio::spawn(async move {
        if deployment {
            db.transfer_deployment_owner(&org_id, &to).await
        } else {
            db.transfer_org_owner(&org_id, &to).await
        }
    })
}

/// Holds `org_id`'s row with `lock` on a connection of its own, starts a
/// transfer with `start`, and says whether the transfer waited for that
/// connection (`true`) or finished while the row was held (`false`). Postgres
/// itself says who waits for whom (`pg_blocking_pids`), so nothing depends on
/// timing. The transfer has finished, successfully, when this returns.
async fn waits_on_a_held_org_row(
    url: &str,
    org_id: &str,
    lock: &str,
    start: impl FnOnce() -> tokio::task::JoinHandle<Result<(), sqlx::Error>>,
) -> bool {
    use sqlx::Connection;
    let mut holder = sqlx::PgConnection::connect(url).await.expect("a connection to hold the row");
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut holder).await.unwrap();
    let mut held = holder.begin().await.unwrap();
    sqlx::query(&format!("SELECT id FROM organizations WHERE id = $1 {lock}"))
        .bind(org_id)
        .execute(&mut *held)
        .await
        .unwrap();
    let mut pending = start();
    let mut observer = sqlx::PgConnection::connect(url).await.expect("a connection to watch the locks");
    let waited = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            tokio::select! {
                done = &mut pending => {
                    done.expect("the transfer's task").expect("the transfer");
                    return false;
                }
                () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                    let waiting: i64 =
                        sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))")
                            .bind(holder_pid)
                            .fetch_one(&mut observer)
                            .await
                            .unwrap();
                    if waiting > 0 {
                        return true;
                    }
                }
            }
        }
    })
    .await
    .expect("the transfer neither finished nor waited on the held row");
    held.rollback().await.unwrap();
    if waited {
        pending.await.expect("the transfer's task").expect("the transfer, once the row is free");
    }
    waited
}

/// Security fix 3, review rounds (2026-11-05): both Owner transfers lock the
/// org's row before they move anything, so a second transfer of the same org
/// waits for the first. Without the lock, under READ COMMITTED, the second's
/// demotion misses the Owner the first promoted, and the org ends up with two.
#[tokio::test]
async fn a_second_owner_transfer_waits_for_the_first_against_postgres() {
    use crate::types::Role;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_second_owner_transfer_waits_for_the_first_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let _turn = DEPLOYMENT_OWNER_TESTS.lock().await;
    let db = std::sync::Arc::new(PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
    let (org, owner, admin) = org_with_an_owner_and_an_admin(&db, "turns").await;

    // The hosted transfer, as the SQLite suite checks it: a swap, or nothing
    // for someone who isn't a member.
    db.transfer_org_owner(&org, &admin).await.unwrap();
    assert_eq!(db.get_membership(&org, &admin).await.unwrap(), Some(Role::Owner));
    assert_eq!(db.get_membership(&org, &owner).await.unwrap(), Some(Role::Admin));
    assert!(matches!(db.transfer_org_owner(&org, "not-a-member").await, Err(sqlx::Error::RowNotFound)));
    assert_eq!(db.get_membership(&org, &admin).await.unwrap(), Some(Role::Owner));

    for (deployment, to) in [(false, &owner), (true, &admin)] {
        assert!(
            waits_on_a_held_org_row(&url, &org, "FOR NO KEY UPDATE", || start_transfer(&db, deployment, &org, to)).await,
            "deployment={deployment}: a transfer ran alongside another of the same org"
        );
    }
    assert_eq!(db.get_membership(&org, &admin).await.unwrap(), Some(Role::Owner));
    assert_eq!(db.get_membership(&org, &owner).await.unwrap(), Some(Role::Admin));
    assert_eq!(db.get_user_by_id(&admin).await.unwrap().unwrap().role, Role::Owner);
    assert_eq!(db.get_user_by_id(&owner).await.unwrap().unwrap().role, Role::Admin);
}

/// The transfers' lock is `FOR NO KEY UPDATE`, never `FOR UPDATE`: a
/// foreign-key check on the org (adding a member, an app, a key) takes
/// `FOR KEY SHARE`, so neither waits for the other.
#[tokio::test]
async fn an_owner_transfer_never_waits_for_a_foreign_key_check_against_postgres() {
    use crate::types::Role;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping an_owner_transfer_never_waits_for_a_foreign_key_check_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let _turn = DEPLOYMENT_OWNER_TESTS.lock().await;
    let db = std::sync::Arc::new(PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
    let (org, owner, admin) = org_with_an_owner_and_an_admin(&db, "keyshare").await;
    for (deployment, to) in [(false, &admin), (true, &owner)] {
        assert!(
            !waits_on_a_held_org_row(&url, &org, "FOR KEY SHARE", || start_transfer(&db, deployment, &org, to)).await,
            "deployment={deployment}: a transfer waited for a foreign-key check"
        );
    }
    assert_eq!(db.get_membership(&org, &owner).await.unwrap(), Some(Role::Owner));
    assert_eq!(db.get_user_by_id(&owner).await.unwrap().unwrap().role, Role::Owner);
    assert_eq!(db.get_membership(&org, &admin).await.unwrap(), Some(Role::Admin));
}

/// The tests that claim generations take turns: a claim takes the oldest free
/// rows of the whole table, so two of them at once would take each other's.
static POLL_CLAIM_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `n` new active generations, `litegen-claim-test-<tag>-<0..n>`, made the
/// oldest in the table so a claim of up to `n` rows takes only them (the shared
/// test database's other rows can't take its slots). Any a crashed earlier run
/// left active are cancelled first. Ids come back oldest first.
async fn the_oldest_active_rows(db: &PostgresDatabase, tag: Uuid, n: usize) -> Vec<String> {
    sqlx::query("UPDATE generations SET status = 'cancelled' WHERE id LIKE 'litegen-claim-test-%' AND status IN ('pending', 'processing')")
        .execute(db.pool())
        .await
        .unwrap();
    let mut ids = Vec::new();
    for i in 0..n {
        let id = format!("litegen-claim-test-{tag}-{i}");
        db.insert_generation(&id, None, "mock/mesh-3d", "mock", "model3d", Some("job"), 0.0, None, None)
            .await
            .unwrap();
        sqlx::query("UPDATE generations SET created_at = TIMESTAMP '1999-01-01 00:00:00' + make_interval(secs => $2) WHERE id = $1")
            .bind(&id)
            .bind(i as f64)
            .execute(db.pool())
            .await
            .unwrap();
        ids.push(id);
    }
    ids
}

/// Two replicas claiming at the same moment: each takes a share of the active
/// rows, no row goes to both, and none is left out; then a replica renews its
/// own rows and a lapsed claim passes on. The two claims share one pool and may
/// not overlap in time, so the proof that a claim skips rows another claimer
/// holds is `a_claim_skips_rows_another_transaction_has_locked`.
#[tokio::test]
async fn concurrent_claims_never_overlap() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping concurrent_claims_never_overlap: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let _turn = POLL_CLAIM_TESTS.lock().await;
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4();
    let mut mine = the_oldest_active_rows(&db, tag, 20).await;

    let hold = chrono::Duration::seconds(30);
    let (replica_a, replica_b) = (format!("a-{tag}"), format!("b-{tag}"));
    let (a, b) = tokio::join!(
        db.claim_active_generations(10, &replica_a, hold),
        db.claim_active_generations(10, &replica_b, hold),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    let (a, b): (Vec<_>, Vec<_>) = (a.into_iter().map(|g| g.id).collect(), b.into_iter().map(|g| g.id).collect());
    assert!(a.iter().all(|id| !b.contains(id)), "no row is claimed twice: a={a:?} b={b:?}");
    assert_eq!(a.len() + b.len(), 20, "and none is left out: a={a:?} b={b:?}");
    let mut claimed: Vec<_> = a.iter().chain(&b).cloned().collect();
    claimed.sort();
    mine.sort();
    assert_eq!(claimed, mine, "the claims are the twenty rows");

    // A replica keeps its rows from tick to tick, and a claim that lapses on
    // the database's clock frees them for another replica.
    let ids = |rows: Vec<crate::types::Generation>| {
        let mut ids: Vec<String> = rows.into_iter().map(|g| g.id).collect();
        ids.sort();
        ids
    };
    let (mut a, mut b) = (a, b);
    a.sort();
    b.sort();
    // The shared database's other active rows may sort before rows just
    // claimed, so these claims have room for every active row, and only the
    // test's rows are judged.
    let all = 100_000;
    let of_mine = |rows: Vec<crate::types::Generation>| {
        let mut ids: Vec<String> = ids(rows).into_iter().filter(|id| mine.contains(id)).collect();
        ids.sort();
        ids
    };
    assert_eq!(of_mine(db.claim_active_generations(all, &replica_a, hold).await.unwrap()), a, "a renews its own rows, and none of b's");
    assert_eq!(of_mine(db.claim_active_generations(all, &replica_b, chrono::Duration::milliseconds(50)).await.unwrap()), b);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let replica_c = format!("c-{tag}");
    assert_eq!(of_mine(db.claim_active_generations(all, &replica_c, hold).await.unwrap()), b, "b's lapsed rows pass to c, a's stay a's");

    for id in &mine {
        db.cancel_generation(id).await.unwrap();
    }
    // Hand back the other tests' rows these claims took.
    for replica in [&replica_a, &replica_b, &replica_c] {
        db.release_poll_claims(replica).await.unwrap();
    }
}

/// `FOR UPDATE SKIP LOCKED`, made to matter: another transaction holds five of
/// twenty active rows, and a claim must return the other fifteen without
/// waiting for the five, which a later claim then gets. `FOR UPDATE` without
/// `SKIP LOCKED` would wait on the first locked row until the timeout, and so
/// would a claim with no lock clause (its UPDATE needs the same row locks).
#[tokio::test]
async fn a_claim_skips_rows_another_transaction_has_locked() {
    use sqlx::Connection;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_claim_skips_rows_another_transaction_has_locked: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let _turn = POLL_CLAIM_TESTS.lock().await;
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4();
    let mine = the_oldest_active_rows(&db, tag, 20).await;
    let (locked, free) = mine.split_at(5);
    // The rows other claims may meet are the shared database's; only these
    // twenty are the test's to judge.
    let of_mine = |rows: Vec<crate::types::Generation>| {
        let mut ids: Vec<String> = rows.into_iter().map(|g| g.id).filter(|id| mine.contains(id)).collect();
        ids.sort();
        ids
    };
    let sorted = |ids: &[String]| {
        let mut ids = ids.to_vec();
        ids.sort();
        ids
    };

    // A claimer that is partway through its own claim: it has the five rows locked.
    let mut holder = sqlx::PgConnection::connect(&url).await.expect("a connection to hold the rows");
    let mut held = holder.begin().await.unwrap();
    sqlx::query("SELECT id FROM generations WHERE id = ANY($1) FOR UPDATE")
        .bind(locked.to_vec())
        .execute(&mut *held)
        .await
        .unwrap();

    let hold = chrono::Duration::seconds(30);
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        db.claim_active_generations(20, &format!("a-{tag}"), hold),
    )
    .await;
    held.rollback().await.unwrap();
    let first = first.expect("the claim waited for rows another transaction had locked").unwrap();
    assert_eq!(of_mine(first), sorted(free), "the claim took the fifteen free rows and none of the five locked ones");

    let later = db.claim_active_generations(20, &format!("b-{tag}"), hold).await.unwrap();
    assert_eq!(of_mine(later), sorted(locked), "once released, the five are claimable, and a's fifteen stay a's");

    for id in &mine {
        db.cancel_generation(id).await.unwrap();
    }
}

/// A replica that stops gracefully hands its poll claims back: another replica
/// takes those rows at once, while rows a third replica holds stay held.
#[tokio::test]
async fn releasing_poll_claims_frees_only_that_claimants_rows() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping releasing_poll_claims_frees_only_that_claimants_rows: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let _turn = POLL_CLAIM_TESTS.lock().await;
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4();
    let mine = the_oldest_active_rows(&db, tag, 6).await;
    let ids = |rows: Vec<crate::types::Generation>| {
        let mut ids: Vec<String> = rows.into_iter().map(|g| g.id).collect();
        ids.sort();
        ids
    };

    let hold = chrono::Duration::seconds(30);
    let (replica_a, replica_b, replica_c) = (format!("a-{tag}"), format!("b-{tag}"), format!("c-{tag}"));
    let a = ids(db.claim_active_generations(2, &replica_a, hold).await.unwrap());
    let c = ids(db.claim_active_generations(2, &replica_c, hold).await.unwrap());
    assert_eq!((a.len(), c.len()), (2, 2));

    // A claim is never cleared when its row turns terminal. So one of a's
    // rows finishes first: the release must neither rewrite nor count it
    // (without the status clause it would also scan every finished row ever
    // polled).
    let finished = a[0].clone();
    db.update_generation_status(&finished, "completed", 100, None, None, Some(chrono::Utc::now())).await.unwrap();
    let claimed_by = |id: String| {
        let pool = db.pool().clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>("SELECT poll_claimed_by FROM generations WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(claimed_by(finished.clone()).await.as_deref(), Some(replica_a.as_str()), "the finished row still names a");

    assert_eq!(db.release_poll_claims(&replica_a).await.unwrap(), 1, "a held one active row (the finished one isn't counted)");
    assert_eq!(claimed_by(finished.clone()).await.as_deref(), Some(replica_a.as_str()), "and the finished row was left as it was");
    // Three rows of the six are free now: a's active one and the two nobody
    // held. A claim of three takes the oldest free ones, and the six are the
    // oldest in the table.
    let b = ids(db.claim_active_generations(3, &replica_b, hold).await.unwrap());
    assert!(b.contains(&a[1]), "b took a's active row at once: a={a:?} b={b:?}");
    assert!(!b.contains(&finished), "a finished row is never claimed: {b:?}");
    assert!(c.iter().all(|id| !b.contains(id)), "and none of c's: c={c:?} b={b:?}");
    assert!(b.iter().all(|id| mine.contains(id)), "all three are the test's: {b:?}");

    assert_eq!(db.release_poll_claims(&replica_a).await.unwrap(), 0, "a has nothing left to release");
    // Room for every active row (the shared database's other rows may sort
    // before c's just-claimed ones), and only the test's rows judged.
    let c_again: Vec<String> = ids(db.claim_active_generations(100_000, &replica_c, hold).await.unwrap())
        .into_iter()
        .filter(|id| mine.contains(id))
        .collect();
    assert_eq!(c_again, c, "c still holds its own");

    for id in &mine {
        db.cancel_generation(id).await.unwrap();
    }
    for replica in [&replica_b, &replica_c] {
        db.release_poll_claims(replica).await.unwrap();
    }
}

/// A replica with more active rows than one claim takes (P2): three claims of
/// two take all five of the test's rows in turn, not the oldest two each time.
/// The test's rows date from 1999, so they sort before the shared database's.
#[tokio::test]
async fn a_claimant_takes_its_active_rows_in_turn_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_claimant_takes_its_active_rows_in_turn_against_postgres: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let _turn = POLL_CLAIM_TESTS.lock().await;
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4();
    let mine = the_oldest_active_rows(&db, tag, 5).await;
    let replica = format!("a-{tag}");
    let hold = chrono::Duration::seconds(30);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..3 {
        let claimed = db.claim_active_generations(2, &replica, hold).await.unwrap();
        seen.extend(claimed.into_iter().map(|g| g.id).filter(|id| mine.contains(id)));
    }
    let all = seen.len();
    for id in &mine {
        db.cancel_generation(id).await.unwrap();
    }
    db.release_poll_claims(&replica).await.unwrap();
    assert_eq!(all, 5, "three claims of two take all five active rows in turn: {seen:?}");
}

/// Leases and `Idempotency-Key` records are stamped and judged on the
/// database's clock (R8). `with_clock_offset` sets "the database's clock" apart
/// from this process's, which is how a replica whose own clock is off looks:
/// one running 6 min ahead must not take over a delivery lease taken just now,
/// and one running 16 min ahead must not judge an in-flight key stale.
#[tokio::test]
async fn leases_and_idempotency_keys_are_judged_on_the_databases_clock_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping leases_and_idempotency_keys_are_judged_on_the_databases_clock_against_postgres: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    // Guard: with no offset, the store's clock is the database's `now()`.
    let theirs: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT now()").fetch_one(db.pool()).await.unwrap();
    let ours = db.clock_now().await.unwrap();
    assert!((ours - theirs).num_milliseconds().abs() < 2000, "clock_now {ours} is the database's now() {theirs}");

    let db = db.with_clock_offset(-chrono::Duration::minutes(6));
    let id = format!("litegen-clock-test-{}", Uuid::new_v4());
    db.insert_generation(&id, None, "mock/mesh-3d", "mock", "model3d", Some("job"), 0.0, None, None).await.unwrap();
    let just_now_on_the_database: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT now() - interval '6 minutes'").fetch_one(db.pool()).await.unwrap();
    db.merge_generation_metadata_if_active(&id, &serde_json::json!({
        "leases": { "delivery": { "by": "elsewhere", "at": just_now_on_the_database.to_rfc3339() } },
    })).await.unwrap();
    let took = db.lease_generation_work(&id, "delivery", "here", crate::proxy::delivery_lease::DELIVERY_LEASE).await.unwrap();

    let db = db.with_clock_offset(-chrono::Duration::minutes(16));
    let scope = format!("clock-test-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO idempotency_keys (scope, idem_key, request_hash, state, created_at) \
         VALUES ($1, 'k', 'h', 'in_progress', now() - interval '16 minutes')",
    )
    .bind(&scope)
    .execute(db.pool())
    .await
    .unwrap();
    let begun = db.idempotency_begin_held(&scope, "k", "h", "req-1", crate::management::reservations::LEASE).await.unwrap();

    db.cancel_generation(&id).await.unwrap();
    sqlx::query("DELETE FROM idempotency_keys WHERE scope = $1").bind(&scope).execute(db.pool()).await.unwrap();
    assert_eq!(
        (took, begun),
        (false, crate::db::IdempotencyBegin::InProgress),
        "a lease taken just now on the database's clock is not 6 min old, and the other replica's run is in progress, not stale"
    );
}

/// A long tick keeps the rows it is still polling (R7): renewing extends only
/// the claimant's own active rows, so past its first hold they are still its
/// own, and nothing of the test's is free for another claimant.
#[tokio::test]
async fn renewing_poll_claims_extends_only_the_claimants_active_rows_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping renewing_poll_claims_extends_only_the_claimants_active_rows_against_postgres: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let _turn = POLL_CLAIM_TESTS.lock().await;
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4();
    let mine = the_oldest_active_rows(&db, tag, 3).await;
    let of_mine = |rows: Vec<crate::types::Generation>| {
        let mut ids: Vec<String> = rows.into_iter().map(|g| g.id).filter(|id| mine.contains(id)).collect();
        ids.sort();
        ids
    };
    let (replica_a, replica_b, replica_c) = (format!("a-{tag}"), format!("b-{tag}"), format!("c-{tag}"));
    let hold = chrono::Duration::seconds(30);
    let a = of_mine(db.claim_active_generations(2, &replica_a, chrono::Duration::seconds(1)).await.unwrap());
    let b = of_mine(db.claim_active_generations(1, &replica_b, hold).await.unwrap());
    assert_eq!((a.len(), b.len()), (2, 1), "the test's rows are the oldest: a={a:?} b={b:?}");
    db.update_generation_status(&a[0], "completed", 100, None, None, Some(chrono::Utc::now())).await.unwrap();

    let renewed = db.renew_poll_claims(&replica_a, &mine, hold).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    // Room for every active row; only the test's are judged.
    let c = of_mine(db.claim_active_generations(100_000, &replica_c, hold).await.unwrap());

    for id in &mine {
        db.cancel_generation(id).await.unwrap();
    }
    for replica in [&replica_a, &replica_b, &replica_c] {
        db.release_poll_claims(replica).await.unwrap();
    }
    assert_eq!(renewed, 1, "only a's active row is a's to renew (not its finished one, not b's)");
    assert!(c.is_empty(), "a's renewed row is still a's and the third is b's: {c:?}");
}

/// Replicas that boot together and each offer their own new secret: the first
/// write wins, and every caller reads that one back (`ON CONFLICT DO NOTHING`
/// then `SELECT`; a plain insert-or-select would let a loser read nothing, or a
/// last-write-wins upsert would let the two disagree). Many rounds, so the two
/// calls overlap on the server in some of them.
#[tokio::test]
async fn server_secret_is_created_once() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping server_secret_is_created_once: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4();
    for round in 0..20 {
        let name = format!("litegen-test-{tag}-{round}");
        let (a, b) = tokio::join!(
            db.server_secret(&name, &|| "from-a".to_string()),
            db.server_secret(&name, &|| "from-b".to_string()),
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a, b, "round {round}: both callers got the one stored value");
        assert!(a == "from-a" || a == "from-b", "round {round}: it is one of the two offered: {a}");
        let later = db.server_secret(&name, &|| "from-c".to_string()).await.unwrap();
        assert_eq!(later, a, "round {round}: a later caller reads the stored value and stores nothing new");
    }
    sqlx::query("DELETE FROM server_secrets WHERE name LIKE $1")
        .bind(format!("litegen-test-{tag}-%"))
        .execute(db.pool())
        .await
        .unwrap();
}

/// Postgres twin of the SQLite test: a rotation sets the new owner in the
/// same statement, and keeps the old one when there's none.
#[tokio::test]
async fn rotate_api_key_takes_a_new_owner_or_keeps_the_old_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping rotate_api_key_takes_a_new_owner_or_keeps_the_old_against_postgres: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4().simple().to_string();
    let (first, second) = (format!("u-first-{tag}"), format!("u-second-{tag}"));
    for id in [&first, &second] {
        sqlx::query("INSERT INTO users (id, email, role) VALUES ($1, $2, 'admin')")
            .bind(id)
            .bind(format!("{id}@example.com"))
            .execute(db.pool())
            .await
            .unwrap();
    }
    let key = db.create_api_key("k", &format!("hash-0-{tag}"), "lg-0", None, None, "generate", None).await.unwrap();
    db.set_api_key_owner(&key.id, &first).await.unwrap();

    let kept = db.rotate_api_key(&key.id, &format!("pk_1_{tag}"), &format!("hash-1-{tag}"), "lg-1", None).await.unwrap().expect("rotated");
    assert_eq!(kept.owner_user_id.as_deref(), Some(first.as_str()));
    assert_eq!(kept.key_hash, format!("hash-1-{tag}"));
    let taken = db.rotate_api_key(&key.id, &format!("pk_2_{tag}"), &format!("hash-2-{tag}"), "lg-2", Some(&second)).await.unwrap().expect("rotated");
    assert_eq!(taken.owner_user_id.as_deref(), Some(second.as_str()));
    assert!(db.rotate_api_key(&Uuid::new_v4(), &format!("pk_3_{tag}"), &format!("hash-3-{tag}"), "lg-3", Some(&second)).await.unwrap().is_none());
}

/// Postgres twin of `migration_0025_gives_keys_made_by_keys_their_makers_owner`,
/// on a database of its own: every migration before 025, the same keys and
/// audit rows, then 025.
#[tokio::test]
async fn migration_0025_backfills_key_owners_against_postgres() {
    use sqlx::postgres::PgPoolOptions;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping migration_0025_backfills_key_owners_against_postgres: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let server = PgPoolOptions::new().max_connections(1).connect(&url).await.expect("connect to the test Postgres");
    let name = format!("litegen_mig0025_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&server).await.expect("create a scratch database");
    let mut scratch_url = reqwest::Url::parse(&url).expect("LITEGEN_PG_TEST_URL is a URL");
    scratch_url.set_path(&format!("/{name}"));
    let pool = PgPoolOptions::new().max_connections(1).connect(scratch_url.as_str()).await.expect("connect to the scratch database");

    let mut before = sqlx::migrate!("./migrations/postgres");
    before.migrations =
        std::borrow::Cow::Owned(before.migrations.iter().filter(|m| m.version < 20240101000025).cloned().collect());
    before.run(&pool).await.expect("migrations before 025");
    for user in ["u-maker", "u-other"] {
        sqlx::query("INSERT INTO users (id, email, role) VALUES ($1, $2, 'admin')")
            .bind(user)
            .bind(format!("{user}@example.com"))
            .execute(&pool)
            .await
            .unwrap();
    }
    for (key, owner) in [
        ("k-parent", Some("u-maker")),
        ("k-child", None),
        ("k-grandchild", None),
        ("k-service", None),
        ("k-service-child", None),
        ("k-owned", Some("u-other")),
        ("k-orphan", None),
        ("k-loop-a", None),
        ("k-loop-b", None),
    ] {
        sqlx::query("INSERT INTO api_keys (id, name, key_hash, key_prefix, owner_user_id) VALUES ($1, $2, $3, 'lg-x', $4)")
            .bind(key)
            .bind(key)
            .bind(format!("hash-{key}"))
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap();
    }
    for (n, (actor, target)) in [
        (Some("k-parent"), "k-child"),
        (Some("k-child"), "k-grandchild"),
        (None, "k-service"),
        (Some("k-service"), "k-service-child"),
        (Some("k-parent"), "k-owned"),
        (Some("k-gone"), "k-orphan"),
        (Some("k-loop-b"), "k-loop-a"),
        (Some("k-loop-a"), "k-loop-b"),
    ]
    .into_iter()
    .enumerate()
    {
        sqlx::query(
            "INSERT INTO audit_log (id, actor_key_id, actor_label, action, target_type, target_id) \
             VALUES ($1, $2, $3, 'key.create', 'api_key', $4)",
        )
        .bind(format!("audit-{n}"))
        .bind(actor)
        .bind(actor.unwrap_or("master-key"))
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO audit_log (id, actor_key_id, actor_label, action, target_type, target_id) \
         VALUES ('audit-rotate', 'k-parent', 'k-parent', 'key.rotate', 'api_key', 'k-orphan')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let applied = sqlx::migrate!("./migrations/postgres").run(&pool).await;
    let owners: std::collections::HashMap<String, Option<String>> =
        sqlx::query_as::<_, (String, Option<String>)>("SELECT id, owner_user_id FROM api_keys")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .collect();
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {name}")).execute(&server).await.expect("drop the scratch database");
    applied.expect("migration 025 applies");

    let owner = |key: &str| owners[key].clone();
    assert_eq!(owner("k-child").as_deref(), Some("u-maker"), "made by the maker's key");
    assert_eq!(owner("k-grandchild").as_deref(), Some("u-maker"), "made by a key the maker's key made");
    assert_eq!(owner("k-service"), None, "made with the master key: the org's");
    assert_eq!(owner("k-service-child"), None, "made by the org's key: the org's");
    assert_eq!(owner("k-owned").as_deref(), Some("u-other"), "an owner already set is kept");
    assert_eq!(owner("k-orphan"), None, "its maker's row is gone, and a rotation names no maker");
    assert_eq!((owner("k-loop-a"), owner("k-loop-b")), (None, None), "a cycle ends with no owner");
}

/// The audit rows a migration test reads back: id → (before_json, after_json).
async fn audit_json_columns(pool: &sqlx::PgPool) -> Result<std::collections::HashMap<String, (Option<String>, Option<String>)>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>)>("SELECT id, before_json, after_json FROM audit_log")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|(id, before, after)| (id, (before, after))).collect())
}

/// Postgres twin of `migration_0027_redacts_provider_header_values_in_the_audit_log`,
/// on a database of its own: every migration before 027, the same audit rows,
/// then 027, then 027's SQL once more.
#[tokio::test]
async fn migration_0027_redacts_provider_header_values_against_postgres() {
    use sqlx::postgres::PgPoolOptions;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping migration_0027_redacts_provider_header_values_against_postgres: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let server = PgPoolOptions::new().max_connections(1).connect(&url).await.expect("connect to the test Postgres");
    let name = format!("litegen_mig0027_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&server).await.expect("create a scratch database");
    let mut scratch_url = reqwest::Url::parse(&url).expect("LITEGEN_PG_TEST_URL is a URL");
    scratch_url.set_path(&format!("/{name}"));
    let pool = PgPoolOptions::new().max_connections(1).connect(scratch_url.as_str()).await.expect("connect to the scratch database");

    let mut before = sqlx::migrate!("./migrations/postgres");
    before.migrations =
        std::borrow::Cow::Owned(before.migrations.iter().filter(|m| m.version < 20240101000027).cloned().collect());
    before.run(&pool).await.expect("migrations before 027");
    let rows = [
        ("a-headers", "provider.configure", None, Some(r#"{"api_base":"https://gateway.example/v1","extra_headers":{"cf-aig-authorization":"Bearer cf-token-1","x-team":"growth"}}"#)),
        ("a-before", "provider.configure", Some(r#"{"extra_headers":{"Helicone-Auth":"Bearer hk-1"}}"#), Some(r#"{"enabled":false}"#)),
        ("a-no-headers", "provider.configure", None, Some(r#"{"api_base":"https://other.example/v1"}"#)),
        ("a-empty", "provider.configure", None, Some(r#"{"extra_headers":{}}"#)),
        ("a-redacted", "provider.configure", None, Some(r#"{"extra_headers":{"x-team":"[redacted]"}}"#)),
        ("a-nothing", "provider.configure", None, None),
        ("a-not-json", "provider.configure", None, Some("not json {")),
        ("a-other-action", "settings.update", None, Some(r#"{"extra_headers":{"x-kept":"kept"}}"#)),
    ];
    for (id, action, before_json, after_json) in rows {
        sqlx::query(
            "INSERT INTO audit_log (id, actor_label, action, target_type, target_id, before_json, after_json) \
             VALUES ($1, 'master-key', $2, 'provider', 'openai', $3, $4)",
        )
        .bind(id)
        .bind(action)
        .bind(before_json)
        .bind(after_json)
        .execute(&pool)
        .await
        .unwrap();
    }

    let migrator = sqlx::migrate!("./migrations/postgres");
    let applied = migrator.run(&pool).await;
    let stored = audit_json_columns(&pool).await;
    let again = match migrator.migrations.iter().find(|m| m.version == 20240101000027) {
        Some(m) => Some(sqlx::raw_sql(&m.sql).execute(&pool).await.map(|_| ())),
        None => None,
    };
    let stored_again = audit_json_columns(&pool).await;
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {name}")).execute(&server).await.expect("drop the scratch database");
    applied.expect("migration 027 applies");
    let stored = stored.expect("read the audit rows");

    let parsed = |text: &Option<String>| text.as_deref().map(|t| serde_json::from_str::<serde_json::Value>(t).expect("still JSON"));
    assert_eq!(
        parsed(&stored["a-headers"].1),
        Some(serde_json::json!({
            "api_base": "https://gateway.example/v1",
            "extra_headers": { "cf-aig-authorization": "[redacted]", "x-team": "[redacted]" },
        })),
        "after_json keeps the names and the rest of the settings"
    );
    assert_eq!(parsed(&stored["a-before"].0), Some(serde_json::json!({ "extra_headers": { "Helicone-Auth": "[redacted]" } })), "before_json too");
    for (id, _, before_json, after_json) in rows {
        let as_written = (before_json.map(str::to_string), after_json.map(str::to_string));
        match id {
            "a-headers" => {}
            "a-before" => assert_eq!(stored[id].1, as_written.1, "{id}: its after_json holds no headers"),
            _ => assert_eq!(stored[id], as_written, "{id} is left as it was"),
        }
    }
    again.expect("migration 027").expect("027 runs a second time");
    assert_eq!(stored_again.expect("read the audit rows again"), stored, "running 027 again changes nothing");
}

/// Postgres twin of `migration_0028_redacts_alert_webhook_urls_in_the_audit_log`,
/// on a database of its own: every migration before 028, the same audit rows,
/// then 028, then 028's SQL once more.
#[tokio::test]
async fn migration_0028_redacts_alert_webhook_urls_against_postgres() {
    use sqlx::postgres::PgPoolOptions;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping migration_0028_redacts_alert_webhook_urls_against_postgres: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let server = PgPoolOptions::new().max_connections(1).connect(&url).await.expect("connect to the test Postgres");
    let name = format!("litegen_mig0028_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&server).await.expect("create a scratch database");
    let mut scratch_url = reqwest::Url::parse(&url).expect("LITEGEN_PG_TEST_URL is a URL");
    scratch_url.set_path(&format!("/{name}"));
    let pool = PgPoolOptions::new().max_connections(1).connect(scratch_url.as_str()).await.expect("connect to the scratch database");

    let mut before = sqlx::migrate!("./migrations/postgres");
    before.migrations =
        std::borrow::Cow::Owned(before.migrations.iter().filter(|m| m.version < 20240101000028).cloned().collect());
    before.run(&pool).await.expect("migrations before 028");
    let rows = [
        ("s-slack", "settings.update", None, Some(r#"{"alert_webhook_url":"https://hooks.slack.com/services/T0001/B0002/secret-1","cost_markup_percent":12.5}"#)),
        ("s-odd-case", "settings.update", None, Some(r#"{"alert_webhook_url":"HTTPS://User:Pass@Hooks.Example.COM:443/alerts?token=abc#frag"}"#)),
        ("s-port", "settings.update", None, Some(r#"{"alert_webhook_url":"http://alerts.internal.example:8080/hook?key=k"}"#)),
        ("s-default-port", "settings.update", None, Some(r#"{"alert_webhook_url":"http://10.0.0.7:80/x"}"#)),
        ("s-backslash", "settings.update", None, Some(r#"{"alert_webhook_url":"https://hooks.example.com\\services\\secret-3"}"#)),
        ("s-ipv6", "settings.update", None, Some(r#"{"alert_webhook_url":"https://[::1]:9000/secret-4"}"#)),
        ("s-host-secret", "settings.update", None, Some(r#"{"alert_webhook_url":"https://eo3secret77.m.pipedream.net/hook"}"#)),
        ("s-before", "settings.update", Some(r#"{"alert_webhook_url":"https://discord.com/api/webhooks/1/secret-2"}"#), Some(r#"{"alert_webhook_url":null}"#)),
        ("s-done", "settings.update", None, Some(r#"{"alert_webhook_url":"[redacted]"}"#)),
        ("s-empty", "settings.update", None, Some(r#"{"alert_webhook_url":""}"#)),
        ("s-no-url", "settings.update", None, Some(r#"{"cost_markup_percent":3}"#)),
        ("s-nothing", "settings.update", None, None),
        ("s-not-json", "settings.update", None, Some("not json {")),
        ("s-other-action", "key.update", None, Some(r#"{"alert_webhook_url":"https://hooks.slack.com/services/kept"}"#)),
    ];
    for (id, action, before_json, after_json) in rows {
        sqlx::query(
            "INSERT INTO audit_log (id, actor_label, action, target_type, target_id, before_json, after_json) \
             VALUES ($1, 'master-key', $2, 'settings', 'gateway', $3, $4)",
        )
        .bind(id)
        .bind(action)
        .bind(before_json)
        .bind(after_json)
        .execute(&pool)
        .await
        .unwrap();
    }

    let migrator = sqlx::migrate!("./migrations/postgres");
    let applied = migrator.run(&pool).await;
    let stored = audit_json_columns(&pool).await;
    let again = match migrator.migrations.iter().find(|m| m.version == 20240101000028) {
        Some(m) => Some(sqlx::raw_sql(&m.sql).execute(&pool).await.map(|_| ())),
        None => None,
    };
    let stored_again = audit_json_columns(&pool).await;
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {name}")).execute(&server).await.expect("drop the scratch database");
    applied.expect("migration 028 applies");
    let stored = stored.expect("read the audit rows");

    let parsed = |text: &Option<String>| text.as_deref().map(|t| serde_json::from_str::<serde_json::Value>(t).expect("still JSON"));
    let rewritten = ["s-slack", "s-odd-case", "s-port", "s-default-port", "s-backslash", "s-ipv6", "s-host-secret"];
    for id in rewritten {
        assert_eq!(parsed(&stored[id].1).expect("after_json")["alert_webhook_url"], "[redacted]", "{id}");
    }
    assert_eq!(parsed(&stored["s-slack"].1).expect("after_json")["cost_markup_percent"], 12.5, "the rest of the settings stay");
    assert_eq!(parsed(&stored["s-before"].0), Some(serde_json::json!({ "alert_webhook_url": "[redacted]" })), "before_json too");
    for (id, _, before_json, after_json) in rows {
        let as_written = (before_json.map(str::to_string), after_json.map(str::to_string));
        if rewritten.contains(&id) {
            assert_eq!(stored[id].0, as_written.0, "{id}: its before_json holds no URL");
        } else if id == "s-before" {
            assert_eq!(stored[id].1, as_written.1, "{id}: its after_json holds no URL");
        } else {
            assert_eq!(stored[id], as_written, "{id} is left as it was");
        }
    }
    again.expect("migration 028").expect("028 runs a second time");
    assert_eq!(stored_again.expect("read the audit rows again"), stored, "running 028 again changes nothing");
}

/// Postgres twin of `an_audit_entry_without_an_org_is_stored_under_none`, on
/// the shared test database: rows of their own, found by an action no other
/// test writes.
#[tokio::test]
async fn an_audit_entry_without_an_org_is_stored_under_none_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping an_audit_entry_without_an_org_is_stored_under_none_against_postgres: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4().simple().to_string();
    let action = format!("test.{tag}");
    let entry = |id: &str, org_id: Option<&str>| crate::types::AuditLogEntry {
        id: format!("{id}-{tag}"),
        actor_key_id: None,
        actor_label: "master-key".to_string(),
        action: action.clone(),
        target_type: "settings".to_string(),
        target_id: "gateway".to_string(),
        before_json: None,
        after_json: None,
        created_at: chrono::Utc::now(),
        org_id: org_id.map(str::to_string),
    };
    db.insert_audit_log(&entry("a-none", None)).await.unwrap();
    db.insert_audit_log(&entry("a-default", Some(DEFAULT_ORG_ID))).await.unwrap();

    let filter = crate::types::AuditLogFilter { action: Some(action.clone()), ..Default::default() };
    let (entries, _) = db.list_audit_log(&filter, 1, 50).await.unwrap();
    let org_of = |id: &str| entries.iter().find(|e| e.id == format!("{id}-{tag}")).expect("stored").org_id.clone();
    assert_eq!(org_of("a-none"), None, "an entry that names no org");
    assert_eq!(org_of("a-default").as_deref(), Some(DEFAULT_ORG_ID));
}

/// One audit entry, as the item 11 tests write them.
fn audit_row(id: &str, actor_key_id: Option<&str>, actor_label: &str, action: &str, target_type: &str, org_id: Option<&str>) -> crate::types::AuditLogEntry {
    crate::types::AuditLogEntry {
        id: id.to_string(),
        actor_key_id: actor_key_id.map(str::to_string),
        actor_label: actor_label.to_string(),
        action: action.to_string(),
        target_type: target_type.to_string(),
        target_id: format!("target-of-{id}"),
        before_json: None,
        after_json: None,
        created_at: chrono::Utc::now(),
        org_id: org_id.map(str::to_string),
    }
}

/// Postgres twin of `the_audit_log_lists_one_orgs_entries_or_those_filed_under_none`,
/// on the shared test database: rows of their own, found by actions no other
/// test writes.
#[tokio::test]
async fn the_audit_log_lists_one_orgs_entries_or_those_filed_under_none_against_postgres() {
    use crate::db::{AuditLogOrg, TenantAuditFilter};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping the_audit_log_lists_one_orgs_entries_or_those_filed_under_none_against_postgres: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4().simple().to_string();
    let (settings, routes) = (format!("test.{tag}.settings"), format!("test.{tag}.routes"));
    for entry in [
        audit_row(&format!("a-none-{tag}"), None, "master-key", &settings, "settings", None),
        audit_row(&format!("a-none-routes-{tag}"), None, "master-key", &routes, "routes", None),
        audit_row(&format!("a-default-{tag}"), None, "master-key", &settings, "settings", Some(DEFAULT_ORG_ID)),
    ] {
        db.insert_audit_log(&entry).await.unwrap();
    }
    let ids = |entries: Vec<crate::types::AuditLogEntry>| {
        let mut ids: Vec<String> = entries.into_iter().map(|e| e.id.trim_end_matches(&format!("-{tag}")).to_string()).collect();
        ids.sort();
        ids
    };
    let settings_only = TenantAuditFilter { action: Some(&settings), ..Default::default() };
    let routes_only = TenantAuditFilter { action: Some(&routes), ..Default::default() };

    let (none, none_total) = db.list_audit_log_for_tenant_filtered(AuditLogOrg::NoOrg, &settings_only, 1, 50).await.unwrap();
    let (default, default_total) = db.list_audit_log_for_tenant_filtered(AuditLogOrg::Org(DEFAULT_ORG_ID), &settings_only, 1, 50).await.unwrap();
    let (none_routes, routes_total) = db.list_audit_log_for_tenant_filtered(AuditLogOrg::NoOrg, &routes_only, 1, 50).await.unwrap();
    assert_eq!((ids(none), none_total), (vec!["a-none".to_string()], 1), "filed under no org");
    assert_eq!((ids(default), default_total), (vec!["a-default".to_string()], 1), "the default org's");
    assert_eq!((ids(none_routes), routes_total), (vec!["a-none-routes".to_string()], 1), "the filters apply");
}

/// Postgres twin of `the_master_keys_entries_for_the_named_actions_move_to_no_org`,
/// on a database of its own: the step moves every matching row it finds.
/// Postgres keeps an entry's org to an org that exists, so the other org is
/// a real one.
#[tokio::test]
async fn the_master_keys_entries_for_the_named_actions_move_to_no_org_against_postgres() {
    use sqlx::postgres::PgPoolOptions;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping the_master_keys_entries_for_the_named_actions_move_to_no_org_against_postgres: set LITEGEN_PG_TEST_URL to a throwaway Postgres database to run");
        return;
    };
    let server = PgPoolOptions::new().max_connections(1).connect(&url).await.expect("connect to the test Postgres");
    let name = format!("litegen_audit_move_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&server).await.expect("create a scratch database");
    let mut scratch_url = reqwest::Url::parse(&url).expect("LITEGEN_PG_TEST_URL is a URL");
    scratch_url.set_path(&format!("/{name}"));
    let db = PostgresDatabase::connect(scratch_url.as_str()).await.expect("connect + migrate the scratch database");

    let now = chrono::Utc::now();
    db.create_organization(&crate::types::Organization {
        id: "org-b".into(),
        name: "B".into(),
        slug: format!("b-{}", Uuid::new_v4().simple()),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    })
    .await
    .expect("another org");
    let rows = [
        audit_row("m-provider", None, "master-key", "provider.configure", "provider", Some(DEFAULT_ORG_ID)),
        audit_row("m-settings", None, "master-key", "settings.update", "settings", Some(DEFAULT_ORG_ID)),
        audit_row("m-user", None, "master-key", "user.update", "user", Some(DEFAULT_ORG_ID)),
        audit_row("m-org-limits", None, "master-key", "limits.update", "org_limits", Some(DEFAULT_ORG_ID)),
        audit_row("person", None, "owner@deploy.test", "settings.update", "settings", Some(DEFAULT_ORG_ID)),
        audit_row("key", Some("key-1"), "key-1", "settings.update", "settings", Some(DEFAULT_ORG_ID)),
        audit_row("system", None, "system", "budget.soft_limit_reached", "org_limits", Some(DEFAULT_ORG_ID)),
        audit_row("m-other-org", None, "master-key", "settings.update", "settings", Some("org-b")),
        audit_row("m-no-org", None, "master-key", "settings.update", "settings", None),
    ];
    let mut written = Ok(());
    for entry in &rows {
        written = written.and(db.insert_audit_log(entry).await);
    }
    let actions = ["provider.configure", "settings.update", "user.update"];
    let moved = db.move_master_key_audit_entries_to_no_org(DEFAULT_ORG_ID, &actions).await;
    let stored = db.list_audit_log(&crate::types::AuditLogFilter::default(), 1, 50).await;
    let again = db.move_master_key_audit_entries_to_no_org(DEFAULT_ORG_ID, &actions).await;
    db.pool().close().await;
    sqlx::query(&format!("DROP DATABASE {name}")).execute(&server).await.expect("drop the scratch database");

    written.expect("the rows are written");
    let (entries, _) = stored.expect("read the rows back");
    let org_of = |id: &str| entries.iter().find(|e| e.id == id).expect("stored").org_id.clone();
    for entry in &rows {
        let expected = match entry.id.as_str() {
            "m-provider" | "m-settings" | "m-user" => None,
            _ => entry.org_id.clone(),
        };
        assert_eq!(org_of(&entry.id), expected, "{}", entry.id);
    }
    assert_eq!(moved.expect("the step runs"), 3);
    assert_eq!(again.expect("the step runs again"), 0, "a second run moves none");
}

/// Review 2026-11-11 P14: the one-statement auth reads answer as the lookups
/// they replace did, against Postgres.
#[tokio::test]
async fn a_key_lookup_reports_its_owners_standing_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_key_lookup_reports_its_owners_standing_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let pool = db.pool().clone();
    let tag = Uuid::new_v4().simple().to_string();
    crate::db::trait_def::one_statement_reads_answer_as_the_lookups(&db, &tag, async |key_id: &str| {
        sqlx::query("UPDATE api_keys SET org_id = NULL WHERE id = $1").bind(key_id).execute(&pool).await.unwrap();
    })
    .await;
}

/// Postgres twin of `sessions_are_kept_by_their_tokens_hash`: sessions are
/// kept by their token's hash (migration 044).
#[tokio::test]
async fn sessions_are_kept_by_their_tokens_hash_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping sessions_are_kept_by_their_tokens_hash_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let pool = db.pool().clone();
    let tag = Uuid::new_v4().simple().to_string();
    crate::db::trait_def::sessions_are_kept_by_their_tokens_hash(&db, &tag, async |user_id: &str| {
        let mut ids: Vec<String> = sqlx::query_scalar("SELECT id FROM sessions WHERE user_id = $1").bind(user_id).fetch_all(&pool).await.unwrap();
        ids.sort();
        ids
    })
    .await;
}

/// SEC-INMEM (review 2026-11-11): deleting an org or an app first lists the
/// jobs it leaves running, to stop them at their vendors: the org's (or one
/// app's) generations still `pending` or `processing`, and no other org's.
#[tokio::test]
async fn active_generations_of_an_org_are_listed_against_postgres() {
    use crate::types::{Application, Organization};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping active_generations_of_an_org_are_listed_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let now = chrono::Utc::now();
    let org = |name: &str| Organization {
        id: Uuid::new_v4().to_string(),
        name: name.into(),
        slug: format!("{name}-{}", Uuid::new_v4().simple()),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    let (mine, other) = (org("running"), org("other"));
    db.create_organization(&mine).await.unwrap();
    db.create_organization(&other).await.unwrap();
    let app = |org_id: &str, slug: &str| Application {
        id: Uuid::new_v4().to_string(),
        org_id: org_id.into(),
        name: slug.into(),
        slug: slug.into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    let (web, batch, theirs) = (app(&mine.id, "web"), app(&mine.id, "batch"), app(&other.id, "web"));
    for a in [&web, &batch, &theirs] {
        db.create_application(a).await.unwrap();
    }
    let job = |org: &Organization, app: &Application, media: &str| {
        let id = format!("litegen-test-{}", Uuid::new_v4());
        (id, org.id.clone(), app.id.clone(), media.to_string())
    };
    let pending = job(&mine, &web, "video");
    let processing = job(&mine, &web, "model3d");
    let in_batch = job(&mine, &batch, "video");
    let finished = job(&mine, &web, "video");
    let elsewhere = job(&other, &theirs, "video");
    for (id, org_id, app_id, media) in [&pending, &processing, &in_batch, &finished, &elsewhere] {
        db.insert_generation(id, None, "mock/held-video", "mock", media, Some("job"), 0.0, Some(org_id.as_str()), Some(app_id.as_str())).await.unwrap();
    }
    db.update_generation_if_active(&processing.0, "processing", 50, None, None, None, None).await.unwrap();
    db.update_generation_if_active(&finished.0, "completed", 100, Some("https://x/v.mp4"), None, Some(now), None).await.unwrap();

    let ids = |gens: Vec<crate::types::Generation>| {
        let mut ids: Vec<String> = gens.into_iter().map(|g| g.id).collect();
        ids.sort();
        ids
    };
    let mut want = vec![pending.0.clone(), processing.0.clone(), in_batch.0.clone()];
    want.sort();
    assert_eq!(ids(db.list_active_generations_for_tenant(&mine.id, None).await.unwrap()), want, "the org's running jobs");
    let mut in_web = vec![pending.0.clone(), processing.0.clone()];
    in_web.sort();
    assert_eq!(ids(db.list_active_generations_for_tenant(&mine.id, Some(&web.id)).await.unwrap()), in_web, "one app's");
    assert_eq!(ids(db.list_active_generations_for_tenant(&mine.id, Some(&theirs.id)).await.unwrap()), Vec::<String>::new(), "another org's app");
    db.delete_organization(&mine.id).await.unwrap();
    db.delete_organization(&other.id).await.unwrap();
}

/// SEC-INVITES (review 2026-11-11): a person who leaves an org, or can no
/// longer invite, takes their pending invitations with them: those to their
/// email in that org, and those they sent there (or anywhere, when the
/// account itself is deactivated). Used ones and other people's stay.
#[tokio::test]
async fn pending_invitations_are_revoked_against_postgres() {
    use crate::types::{Invitation, Organization, Role, User};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping pending_invitations_are_revoked_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let now = chrono::Utc::now();
    let org = |name: &str| Organization {
        id: Uuid::new_v4().to_string(),
        name: name.into(),
        slug: format!("{name}-{}", Uuid::new_v4().simple()),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    let (mine, other) = (org("invites"), org("elsewhere"));
    db.create_organization(&mine).await.unwrap();
    db.create_organization(&other).await.unwrap();
    let user = || User {
        id: Uuid::new_v4().to_string(),
        email: format!("{}@invites.test", Uuid::new_v4().simple()),
        password_hash: None,
        role: Role::Admin,
        oauth_github_id: None,
        oauth_google_id: None,
        created_at: now,
        updated_at: now,
        last_login_at: None,
        is_active: true,
        sign_in_epoch: 0,
    };
    let (leaver, stayer) = (user(), user());
    db.create_user(&leaver).await.unwrap();
    db.create_user(&stayer).await.unwrap();
    let invite = |org: &Organization, email: &str, by: &User| Invitation {
        id: Uuid::new_v4().to_string(),
        email: email.to_string(),
        role: Role::Member,
        token: Uuid::new_v4().simple().to_string(),
        invited_by: Some(by.id.clone()),
        org_id: org.id.clone(),
        expires_at: now + chrono::Duration::days(7),
        used_at: None,
        created_at: now,
    };
    let to_leaver = invite(&mine, &leaver.email, &stayer);
    let sent_here = invite(&mine, "friend@invites.test", &leaver);
    let sent_there = invite(&other, "friend@invites.test", &leaver);
    let used = invite(&mine, "used@invites.test", &leaver);
    let others = invite(&mine, "someone@invites.test", &stayer);
    for inv in [&to_leaver, &sent_here, &sent_there, &used, &others] {
        db.create_invitation(inv).await.unwrap();
    }
    assert!(db.mark_invitation_used(&used.token).await.unwrap());
    let alive = |token: String| {
        let db = &db;
        async move { db.get_invitation(&token).await.unwrap().is_some_and(|i| i.used_at.is_none()) }
    };

    // Leaving one org: the invitations to them there, and those they sent there.
    let gone = db.revoke_pending_invitations(Some(&mine.id), Some(&leaver.email), Some(&leaver.id)).await.unwrap();
    assert_eq!(gone.len(), 2, "the one to them and the one they sent, in that org");
    assert!(!alive(to_leaver.token.clone()).await && !alive(sent_here.token.clone()).await);
    assert!(alive(sent_there.token.clone()).await, "another org's stays");
    assert!(alive(others.token.clone()).await, "someone else's stays");
    assert!(db.get_invitation(&used.token).await.unwrap().is_some(), "a used one stays");
    // Deactivated: what they sent, everywhere.
    assert_eq!(db.revoke_pending_invitations(None, None, Some(&leaver.id)).await.unwrap().len(), 1);
    assert!(!alive(sent_there.token.clone()).await);
    assert!(alive(others.token).await);
}

/// Postgres twin of `an_orgs_unused_invitations_are_listed_and_revoked`.
#[tokio::test]
async fn an_orgs_unused_invitations_are_listed_and_revoked_against_postgres() {
    use crate::types::{Invitation, Organization, Role};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping an_orgs_unused_invitations_are_listed_and_revoked_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let now = chrono::Utc::now();
    let org = |name: &str| Organization {
        id: Uuid::new_v4().to_string(),
        name: name.into(),
        slug: format!("{name}-{}", Uuid::new_v4().simple()),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    let (mine, other) = (org("listed"), org("unlisted"));
    db.create_organization(&mine).await.unwrap();
    db.create_organization(&other).await.unwrap();
    let invite = |org: &Organization, email: &str, age_hours: i64| Invitation {
        id: Uuid::new_v4().to_string(),
        email: email.to_string(),
        role: Role::Member,
        token: Uuid::new_v4().simple().to_string(),
        invited_by: None,
        org_id: org.id.clone(),
        expires_at: now + chrono::Duration::days(7),
        used_at: None,
        created_at: now - chrono::Duration::hours(age_hours),
    };
    let older = invite(&mine, "older@listed.test", 2);
    let newer = invite(&mine, "newer@listed.test", 1);
    let used = invite(&mine, "used@listed.test", 3);
    let theirs = invite(&other, "theirs@listed.test", 1);
    for inv in [&older, &newer, &used, &theirs] {
        db.create_invitation(inv).await.unwrap();
    }
    assert!(db.mark_invitation_used(&used.token).await.unwrap());

    let listed: Vec<String> = db.list_unused_invitations(&mine.id).await.unwrap().into_iter().map(|i| i.id).collect();
    assert_eq!(listed, [newer.id.clone(), older.id.clone()]);
    assert!(db.revoke_invitation(&mine.id, &theirs.id).await.unwrap().is_none(), "another org's");
    assert!(db.revoke_invitation(&mine.id, &used.id).await.unwrap().is_none(), "an accepted one");
    let gone = db.revoke_invitation(&mine.id, &older.id).await.unwrap().expect("revoked");
    assert_eq!(gone.email, "older@listed.test");
    assert!(db.get_invitation(&older.token).await.unwrap().is_none(), "its token is gone");
    assert!(db.revoke_invitation(&mine.id, &older.id).await.unwrap().is_none(), "once");
    assert!(db.get_invitation(&theirs.token).await.unwrap().is_some(), "another org's stays");
}

/// Postgres twin of round 4's sign-in epoch and deactivate-again tests: a
/// session begun before a reset, a deactivation or a password change is
/// refused once it lands; deactivating an inactive account again deletes its
/// links.
#[tokio::test]
async fn sign_in_epochs_and_deactivating_again_against_postgres() {
    use crate::config::Mode;
    use crate::db::ResetConfirm;
    use crate::types::{PasswordReset, Session};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping sign_in_epochs_and_deactivating_again_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let user = crate::types::User { email: format!("epoch-{}@example.com", Uuid::new_v4().simple()), role: crate::types::Role::Member, ..first_user(0) };
    db.create_user(&user).await.unwrap();
    let tag = Uuid::new_v4().simple().to_string();
    let session = |id: &str| {
        let now = chrono::Utc::now();
        Session { id: format!("{id}-{tag}"), user_id: user.id.clone(), created_at: now, expires_at: now + chrono::Duration::days(7), ip: None, user_agent: None, csrf_token: "csrf".into() }
    };
    let link = |token: &str| PasswordReset {
        token: format!("{token}-{tag}"),
        user_id: user.id.clone(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        used_at: None,
        created_at: chrono::Utc::now(),
        issued_by: None,
    };

    let begun = db.sign_in_epoch(&user.id).await.unwrap().expect("the account");
    db.create_password_reset(&link("rescue")).await.unwrap();
    assert!(matches!(db.confirm_password_reset(&format!("rescue-{tag}"), "h", Mode::SingleTenant).await.unwrap(), ResetConfirm::Done { .. }));
    db.create_session_at_epoch(&session("late"), begun).await.unwrap();
    assert!(db.get_session_with_user(&format!("late-{tag}")).await.unwrap().is_none(), "a login begun before the reset");
    let now = db.sign_in_epoch(&user.id).await.unwrap().unwrap();
    db.create_session_at_epoch(&session("fresh"), now).await.unwrap();
    assert!(db.get_session_with_user(&format!("fresh-{tag}")).await.unwrap().is_some());

    let begun = db.sign_in_epoch(&user.id).await.unwrap().unwrap();
    db.create_session(&session("kept")).await.unwrap();
    let kept = crate::auth::tokens::token_hash(&format!("kept-{tag}"));
    assert_eq!(db.change_own_password(&user.id, "newer", "h", &kept).await.unwrap(), crate::db::PasswordChange::Done);
    db.create_session_at_epoch(&session("across"), begun).await.unwrap();
    assert!(db.get_session_with_user(&format!("across-{tag}")).await.unwrap().is_none(), "a login begun before a password change");
    assert!(db.get_session_with_user(&format!("kept-{tag}")).await.unwrap().is_some(), "the session that changed it stays valid");

    db.update_user(&user.id, None, Some(false), None).await.unwrap();
    db.create_password_reset(&link("reactivate")).await.unwrap();
    db.update_user(&user.id, None, Some(false), None).await.unwrap();
    assert!(db.get_password_reset(&format!("reactivate-{tag}")).await.unwrap().is_none(), "deactivating again deletes it");
}

/// Postgres twin of round 4's issuer tests: confirm re-checks the issuer
/// (a late insert after a promotion, a demoted or deactivated issuer, a
/// hosted person reactivating), and an issuer's unused links go when the
/// issuer is demoted, removed or deactivated.
#[tokio::test]
async fn link_issuers_against_postgres() {
    use crate::config::Mode;
    use crate::db::ResetConfirm;
    use crate::types::{PasswordReset, Role};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping link_issuers_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4().simple().to_string();
    let mut n: usize = 0;
    let mut member_of = async |role: Role| {
        n += 1;
        let user = crate::types::User { email: format!("issuer-{tag}-{n}@example.com"), role, ..first_user(n) };
        db.create_user(&user).await.unwrap();
        db.add_org_member(DEFAULT_ORG_ID, &user.id, role).await.unwrap();
        user
    };
    let admin = member_of(Role::Admin).await;
    let member = member_of(Role::Member).await;
    let promoted = member_of(Role::Member).await;
    let demoted = member_of(Role::Admin).await;
    let gone = member_of(Role::Admin).await;
    let inactive = member_of(Role::Member).await;
    let issuers: Vec<crate::types::User> = vec![member_of(Role::Admin).await, member_of(Role::Admin).await, member_of(Role::Admin).await];
    let link = |target: &str, token: &str, issuer: Option<&str>| PasswordReset {
        token: format!("{token}-{tag}"),
        user_id: target.to_string(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        used_at: None,
        created_at: chrono::Utc::now(),
        issued_by: issuer.map(str::to_string),
    };
    let confirm = |token: &str, mode: Mode| {
        let token = format!("{token}-{tag}");
        let db = &db;
        async move { db.confirm_password_reset(&token, "h", mode).await.unwrap() }
    };

    db.update_member_role(DEFAULT_ORG_ID, &promoted.id, Role::Admin).await.unwrap();
    db.create_password_reset(&link(&promoted.id, "late", Some(&admin.id))).await.unwrap();
    assert_eq!(confirm("late", Mode::SingleTenant).await, ResetConfirm::IssuerRevoked, "after a promotion");
    db.update_member_role(DEFAULT_ORG_ID, &demoted.id, Role::Member).await.unwrap();
    db.create_password_reset(&link(&member.id, "demoted", Some(&demoted.id))).await.unwrap();
    assert_eq!(confirm("demoted", Mode::SingleTenant).await, ResetConfirm::IssuerRevoked, "a demoted issuer");
    db.update_user(&gone.id, None, Some(false), None).await.unwrap();
    db.create_password_reset(&link(&member.id, "gone", Some(&gone.id))).await.unwrap();
    assert_eq!(confirm("gone", Mode::SingleTenant).await, ResetConfirm::IssuerRevoked, "a deactivated issuer");
    db.update_user(&inactive.id, None, Some(false), None).await.unwrap();
    db.create_password_reset(&link(&inactive.id, "hosted", Some(&admin.id))).await.unwrap();
    assert_eq!(confirm("hosted", Mode::Hosted).await, ResetConfirm::IssuerRevoked, "hosted: a person can't reactivate");
    db.create_password_reset(&link(&inactive.id, "reactivate", Some(&admin.id))).await.unwrap();
    assert_eq!(confirm("reactivate", Mode::SingleTenant).await, ResetConfirm::Done { user_id: inactive.id.clone(), reactivated: true, revoked_invitations: vec![] });
    db.create_password_reset(&link(&promoted.id, "master", None)).await.unwrap();
    assert!(matches!(confirm("master", Mode::SingleTenant).await, ResetConfirm::Done { .. }), "the master key");

    for (i, issuer) in issuers.iter().enumerate() {
        let token = format!("follows-{i}");
        db.create_password_reset(&link(&member.id, &token, Some(&issuer.id))).await.unwrap();
        match i {
            0 => db.update_member_role(DEFAULT_ORG_ID, &issuer.id, Role::Member).await.unwrap(),
            1 => db.remove_org_member(DEFAULT_ORG_ID, &issuer.id).await.unwrap(),
            _ => assert!(db.update_user(&issuer.id, None, Some(false), None).await.unwrap().is_some()),
        }
        assert!(db.get_password_reset(&format!("{token}-{tag}")).await.unwrap().is_none(), "issuer {i}: its link goes");
    }
}

/// Postgres twin of round 5's store tests: a password change checked before
/// a reset writes nothing; reactivating moves the epoch on; a reset deletes
/// the links the account issued and revokes its invitations; a stale grant
/// claim revokes nothing; the session list leaves out an old epoch.
#[tokio::test]
async fn round_5_store_rules_against_postgres() {
    use crate::config::Mode;
    use crate::db::{PasswordChange, ResetConfirm};
    use crate::mcp::oauth::store::Grant;
    use crate::mcp::Surface;
    use crate::types::{Invitation, PasswordReset, Role, Session};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping round_5_store_rules_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4().simple().to_string();
    let user = crate::types::User { email: format!("r5-{tag}@example.com"), role: Role::Admin, password_hash: Some("verified".into()), ..first_user(0) };
    db.create_user(&user).await.unwrap();
    db.add_org_member(DEFAULT_ORG_ID, &user.id, Role::Admin).await.unwrap();
    let member = crate::types::User { email: format!("r5m-{tag}@example.com"), role: Role::Member, ..first_user(1) };
    db.create_user(&member).await.unwrap();
    db.add_org_member(DEFAULT_ORG_ID, &member.id, Role::Member).await.unwrap();
    let session = |id: &str| {
        let now = chrono::Utc::now();
        Session { id: format!("{id}-{tag}"), user_id: user.id.clone(), created_at: now, expires_at: now + chrono::Duration::days(7), ip: None, user_agent: None, csrf_token: "csrf".into() }
    };
    let link = |target: &str, token: &str, issuer: Option<&str>| PasswordReset {
        token: format!("{token}-{tag}"),
        user_id: target.to_string(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        used_at: None,
        created_at: chrono::Utc::now(),
        issued_by: issuer.map(str::to_string),
    };

    db.create_session(&session("thief")).await.unwrap();
    db.create_password_reset(&link(&member.id, "issued", Some(&user.id))).await.unwrap();
    let invitation = Invitation {
        id: Uuid::new_v4().to_string(),
        email: format!("stand-in-{tag}@example.com"),
        role: Role::Member,
        token: format!("invite-{tag}"),
        invited_by: Some(user.id.clone()),
        org_id: DEFAULT_ORG_ID.to_string(),
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        used_at: None,
        created_at: chrono::Utc::now(),
    };
    db.create_invitation(&invitation).await.unwrap();
    db.create_password_reset(&link(&user.id, "rescue", None)).await.unwrap();
    let ResetConfirm::Done { revoked_invitations, .. } = db.confirm_password_reset(&format!("rescue-{tag}"), "reset-hash", Mode::SingleTenant).await.unwrap() else {
        panic!("the reset went through");
    };
    assert_eq!(revoked_invitations.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), [invitation.id.as_str()], "its invitations revoked");
    assert!(db.get_password_reset(&format!("issued-{tag}")).await.unwrap().is_none(), "the link it issued is gone");
    let thief = crate::auth::tokens::token_hash(&format!("thief-{tag}"));
    assert_eq!(db.change_own_password(&user.id, "thief-hash", "verified", &thief).await.unwrap(), PasswordChange::SignInEnded);
    assert_eq!(db.get_user_by_id(&user.id).await.unwrap().unwrap().password_hash.as_deref(), Some("reset-hash"), "the reset's password stays");

    db.update_user(&user.id, None, Some(false), None).await.unwrap();
    let while_inactive = db.sign_in_epoch(&user.id).await.unwrap().unwrap();
    db.create_session_at_epoch(&session("while-inactive"), while_inactive).await.unwrap();
    db.update_user(&user.id, None, Some(true), None).await.unwrap();
    assert!(db.get_session_with_user(&format!("while-inactive-{tag}")).await.unwrap().is_none(), "reactivating moves the epoch on");
    let listed: Vec<String> = db.list_user_sessions(&user.id).await.unwrap().into_iter().map(|s| s.id).collect();
    assert!(listed.is_empty(), "the old-epoch session isn't listed: {listed:?}");

    let current = db.sign_in_epoch(&user.id).await.unwrap().unwrap();
    let oauth = db.mcp_oauth().expect("an OAuth store");
    let now = chrono::Utc::now();
    let grant = |id: &str, epoch: i64| Grant {
        id: format!("{id}-{tag}"),
        client_id: format!("lgc-{tag}"),
        user_id: user.id.clone(),
        org_id: DEFAULT_ORG_ID.into(),
        app_id: None,
        surface: Surface::Ai,
        scopes: vec!["ai:read".into()],
        created_at: now,
        last_used_at: None,
        revoked_at: None,
        sign_in_epoch: epoch,
    };
    assert!(oauth.claim_grant(&grant("newer", current)).await.unwrap().is_some());
    assert_eq!(oauth.claim_grant(&grant("stale", current - 1)).await.unwrap(), None, "a stale claim is refused");
    assert!(oauth.get_grant(&format!("newer-{tag}")).await.unwrap().is_some_and(|g| g.revoked_at.is_none()), "the newer grant stays");
}

/// Runs `start` while a connection of its own holds `user_id`'s row with
/// the uncommitted `hold` (an `UPDATE`, as a reset or a deactivation holds
/// it until it commits), then commits the hold. Says whether `start` waited
/// for that connection (Postgres itself says who waits for whom, so nothing
/// depends on timing), and what it returned.
async fn runs_while_an_update_of_the_row_commits(
    url: &str,
    hold: &str,
    user_id: &str,
    start: impl FnOnce() -> tokio::task::JoinHandle<Result<bool, sqlx::Error>>,
) -> (bool, bool) {
    use sqlx::Connection;
    let mut holder = sqlx::PgConnection::connect(url).await.expect("a connection to hold the row");
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut holder).await.unwrap();
    let mut held = holder.begin().await.unwrap();
    sqlx::query(hold).bind(user_id).execute(&mut *held).await.unwrap();
    let mut pending = start();
    let mut observer = sqlx::PgConnection::connect(url).await.expect("a connection to watch the locks");
    let finished_first = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            tokio::select! {
                done = &mut pending => return Some(done.expect("the task").expect("the write")),
                () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                    let waiting: i64 =
                        sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))")
                            .bind(holder_pid)
                            .fetch_one(&mut observer)
                            .await
                            .unwrap();
                    if waiting > 0 {
                        return None;
                    }
                }
            }
        }
    })
    .await
    .expect("the write neither finished nor waited on the held row");
    held.commit().await.unwrap();
    match finished_first {
        Some(stored) => (false, stored),
        None => (true, pending.await.expect("the task").expect("the write, once the row is free")),
    }
}

/// Review round 6, finding 2, on Postgres: a link or an invitation is stored
/// only while the sign-in sending it is live, checked in the statement that
/// stores it. One sent while the sender's reset or deactivation commits
/// waits for that commit (the check holds a share lock on the sender's
/// row), then stores nothing. A deactivation revokes the account's pending
/// invitations in its own transaction and hands them back.
#[tokio::test]
async fn round_6_store_rules_against_postgres() {
    use crate::config::Mode;
    use crate::db::{ResetConfirm, SignIn};
    use crate::types::{Invitation, PasswordReset, Role, Session};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping round_6_store_rules_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = std::sync::Arc::new(PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
    let tag = Uuid::new_v4().simple().to_string();
    let admin = crate::types::User { email: format!("r6-{tag}@example.com"), role: Role::Admin, ..first_user(0) };
    db.create_user(&admin).await.unwrap();
    db.add_org_member(DEFAULT_ORG_ID, &admin.id, Role::Admin).await.unwrap();
    let member = crate::types::User { email: format!("r6m-{tag}@example.com"), role: Role::Member, ..first_user(1) };
    db.create_user(&member).await.unwrap();
    db.add_org_member(DEFAULT_ORG_ID, &member.id, Role::Member).await.unwrap();
    let session = |id: &str| {
        let now = chrono::Utc::now();
        Session { id: format!("{id}-{tag}"), user_id: admin.id.clone(), created_at: now, expires_at: now + chrono::Duration::days(7), ip: None, user_agent: None, csrf_token: "csrf".into() }
    };
    let link = |token: &str, issuer: Option<&str>| PasswordReset {
        token: format!("{token}-{tag}"),
        user_id: member.id.clone(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        used_at: None,
        created_at: chrono::Utc::now(),
        issued_by: issuer.map(str::to_string),
    };
    let invitation = |token: &str| Invitation {
        id: Uuid::new_v4().to_string(),
        email: format!("{token}-{tag}@example.com"),
        role: Role::Member,
        token: format!("{token}-{tag}"),
        invited_by: Some(admin.id.clone()),
        org_id: DEFAULT_ORG_ID.to_string(),
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        used_at: None,
        created_at: chrono::Utc::now(),
    };
    let stored = |token: &str| {
        let (db, token) = (db.clone(), format!("{token}-{tag}"));
        async move { (db.get_invitation(&token).await.unwrap().is_some(), db.get_password_reset(&token).await.unwrap().is_some()) }
    };

    // A session's sign-in goes by its stored id, its token's hash (migration 044).
    let stored_id = |token: &str| crate::auth::tokens::token_hash(&format!("{token}-{tag}"));
    let live = stored_id("live");
    db.create_session(&session("live")).await.unwrap();
    assert!(db.create_invitation_from(&invitation("sent-live"), SignIn::Session(&live)).await.unwrap());
    assert!(db.create_password_reset_from(&link("sent-live", Some(&admin.id)), SignIn::Session(&live)).await.unwrap());
    assert_eq!(stored("sent-live").await, (true, true), "both stored from a live session");

    // Sent while the sender's reset (an epoch move) or deactivation commits:
    // each waits for the commit, then stores nothing.
    for (ending, hold) in [
        ("reset", "UPDATE users SET sign_in_epoch = sign_in_epoch + 1 WHERE id = $1"),
        ("deactivation", "UPDATE users SET is_active = FALSE WHERE id = $1"),
    ] {
        let sid = stored_id(ending);
        db.create_session(&session(ending)).await.unwrap();
        let (inv, sid_for_inv) = (invitation(&format!("racing-{ending}")), sid.clone());
        let racer = db.clone();
        let (waited, landed) = runs_while_an_update_of_the_row_commits(&url, hold, &admin.id, move || {
            tokio::spawn(async move { racer.create_invitation_from(&inv, SignIn::Session(&sid_for_inv)).await })
        })
        .await;
        assert!(waited, "{ending}: the invitation's insert waited for the sender's row");
        assert!(!landed, "{ending}: and stored nothing once the {ending} committed");
        db.update_user(&admin.id, None, Some(true), None).await.unwrap();
        let sid = stored_id(&format!("{ending}-link"));
        db.create_session(&session(&format!("{ending}-link"))).await.unwrap();
        let (reset, racer) = (link(&format!("racing-{ending}"), Some(&admin.id)), db.clone());
        let (waited, landed) = runs_while_an_update_of_the_row_commits(&url, hold, &admin.id, move || {
            tokio::spawn(async move { racer.create_password_reset_from(&reset, SignIn::Session(&sid)).await })
        })
        .await;
        assert!(waited, "{ending}: the link's insert waited for the sender's row");
        assert!(!landed, "{ending}: and stored nothing once the {ending} committed");
        assert_eq!(stored(&format!("racing-{ending}")).await, (false, false), "{ending}: nothing stored");
        db.update_user(&admin.id, None, Some(true), None).await.unwrap();
    }

    // Ended before: a reset's confirm, a session that is gone, an inactive
    // account's session at its current epoch. The master key's land anyway.
    db.create_session(&session("before-reset")).await.unwrap();
    db.create_password_reset(&PasswordReset { user_id: admin.id.clone(), ..link("rescue", None) }).await.unwrap();
    assert!(matches!(db.confirm_password_reset(&format!("rescue-{tag}"), "reset-hash", Mode::SingleTenant).await.unwrap(), ResetConfirm::Done { .. }));
    let before_reset = stored_id("before-reset");
    assert!(!db.create_invitation_from(&invitation("after-reset"), SignIn::Session(&before_reset)).await.unwrap());
    assert!(!db.create_password_reset_from(&link("after-reset", Some(&admin.id)), SignIn::Session(&before_reset)).await.unwrap());
    db.update_user(&admin.id, None, Some(false), None).await.unwrap();
    let while_inactive = db.sign_in_epoch(&admin.id).await.unwrap().unwrap();
    db.create_session_at_epoch(&session("inactive"), while_inactive).await.unwrap();
    let inactive = stored_id("inactive");
    assert!(!db.create_invitation_from(&invitation("while-inactive"), SignIn::Session(&inactive)).await.unwrap());
    assert!(!db.create_password_reset_from(&link("while-inactive", Some(&admin.id)), SignIn::Session(&inactive)).await.unwrap());
    assert_eq!(stored("after-reset").await, (false, false));
    assert_eq!(stored("while-inactive").await, (false, false));
    assert!(db.create_invitation_from(&invitation("master"), SignIn::MasterKey).await.unwrap());
    assert!(db.create_password_reset_from(&link("master", None), SignIn::MasterKey).await.unwrap());
    assert_eq!(stored("master").await, (true, true));

    // An app's OAuth grant sends while it is live, at the person's epoch;
    // once the epoch moves on, it sends nothing.
    db.update_user(&admin.id, None, Some(true), None).await.unwrap();
    let epoch = db.sign_in_epoch(&admin.id).await.unwrap().unwrap();
    let oauth = db.mcp_oauth().expect("an OAuth store");
    let grant = crate::mcp::oauth::store::Grant {
        id: format!("grant-{tag}"),
        client_id: format!("lgc-{tag}"),
        user_id: admin.id.clone(),
        org_id: DEFAULT_ORG_ID.into(),
        app_id: None,
        surface: crate::mcp::Surface::Admin,
        scopes: vec!["admin:write".into()],
        created_at: chrono::Utc::now(),
        last_used_at: None,
        revoked_at: None,
        sign_in_epoch: epoch,
    };
    assert!(oauth.claim_grant(&grant).await.unwrap().is_some());
    assert!(db.create_invitation_from(&invitation("from-the-app"), SignIn::Grant(&grant.id)).await.unwrap(), "a live grant");
    assert!(db.create_password_reset_from(&link("from-the-app", Some(&admin.id)), SignIn::Grant(&grant.id)).await.unwrap(), "a live grant");
    sqlx::query("UPDATE users SET sign_in_epoch = sign_in_epoch + 1 WHERE id = $1").bind(&admin.id).execute(db.pool()).await.unwrap();
    assert!(!db.create_invitation_from(&invitation("from-an-old-grant"), SignIn::Grant(&grant.id)).await.unwrap(), "a grant of an old epoch");
    assert!(!db.create_password_reset_from(&link("from-an-old-grant", Some(&admin.id)), SignIn::Grant(&grant.id)).await.unwrap(), "a grant of an old epoch");
    assert_eq!(stored("from-the-app").await, (true, true));
    assert_eq!(stored("from-an-old-grant").await, (false, false));

    // A deactivation revokes the account's pending invitations, in its
    // transaction, and hands them back.
    db.update_user(&admin.id, None, Some(true), None).await.unwrap();
    let pending = invitation("pending-at-deactivation");
    db.create_invitation(&pending).await.unwrap();
    let (user, revoked) = db.update_user_with_revoked(&admin.id, None, Some(false), None, None).await.unwrap().expect("the account");
    assert!(!user.is_active);
    assert!(revoked.iter().any(|i| i.id == pending.id), "{revoked:?}");
    assert!(revoked.iter().all(|i| i.invited_by.as_deref() == Some(admin.id.as_str())), "{revoked:?}");
    assert!(!stored("pending-at-deactivation").await.0, "revoked");
}

/// Waits until a statement on another connection waits for the backend
/// `pid` (Postgres itself says who waits for whom, so nothing depends on
/// timing), failing if `task` finishes first.
async fn waits_for_backend<T>(url: &str, pid: i32, task: &tokio::task::JoinHandle<T>, what: &str) {
    use sqlx::Connection;
    let mut observer = sqlx::PgConnection::connect(url).await.expect("a connection to watch the locks");
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))")
                .bind(pid)
                .fetch_one(&mut observer)
                .await
                .unwrap();
            if waiting > 0 {
                return;
            }
            assert!(!task.is_finished(), "{what} finished without waiting");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what} neither waited nor finished"));
}

/// Review round 7, findings 4 and 5, on Postgres.
///
/// 4: issuing a link takes the account's older unused links before anything
/// that locks the account's row (the new link's foreign key takes a
/// key-share lock on it): the order a confirm takes them in, its link and
/// then the row. So an issue and a confirm of the older link don't deadlock.
///
/// 5: an invitation or a link sent from the very session that is changing
/// its own password is stored. The change moves the account's epoch (its
/// first write, which locks the account's row), re-stamps that session, and
/// commits. The send waits for it without holding the session's row, then
/// reads both rows anew.
#[tokio::test]
async fn round_8_store_rules_against_postgres() {
    use crate::db::SignIn;
    use crate::types::{Invitation, PasswordReset, Role, Session};
    use sqlx::Connection;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping round_8_store_rules_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = std::sync::Arc::new(PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
    let tag = Uuid::new_v4().simple().to_string();
    let admin = crate::types::User { email: format!("r8-{tag}@example.com"), role: Role::Admin, ..first_user(0) };
    db.create_user(&admin).await.unwrap();
    db.add_org_member(DEFAULT_ORG_ID, &admin.id, Role::Admin).await.unwrap();
    let member = crate::types::User { email: format!("r8m-{tag}@example.com"), role: Role::Member, ..first_user(1) };
    db.create_user(&member).await.unwrap();
    db.add_org_member(DEFAULT_ORG_ID, &member.id, Role::Member).await.unwrap();
    let cookie = format!("r8-{tag}");
    let now = chrono::Utc::now();
    db.create_session(&Session { id: cookie.clone(), user_id: admin.id.clone(), created_at: now, expires_at: now + chrono::Duration::days(7), ip: None, user_agent: None, csrf_token: "csrf".into() })
        .await
        .unwrap();
    // The session's stored id, its token's hash (migration 044): what its
    // sign-in and the re-stamp below name.
    let sid = crate::auth::tokens::token_hash(&cookie);
    let link = |token: &str, issuer: Option<&str>| PasswordReset {
        token: format!("{token}-{tag}"),
        user_id: member.id.clone(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        used_at: None,
        created_at: chrono::Utc::now(),
        issued_by: issuer.map(str::to_string),
    };

    // 4: a confirm has claimed the older link when a new one is issued...
    db.create_password_reset(&link("older", None)).await.unwrap();
    let mut confirm = sqlx::PgConnection::connect(&url).await.expect("a connection for the confirm");
    let confirm_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut confirm).await.unwrap();
    let mut claimed = confirm.begin().await.unwrap();
    let claim = sqlx::query("UPDATE password_resets SET used_at = NOW() WHERE token = $1 AND used_at IS NULL")
        .bind(crate::auth::tokens::token_hash(&format!("older-{tag}")))
        .execute(&mut *claimed)
        .await
        .unwrap();
    assert_eq!(claim.rows_affected(), 1, "the confirm's claim");
    let (racer, s, newer) = (db.clone(), sid.clone(), link("newer", Some(&admin.id)));
    let issue = tokio::spawn(async move { racer.create_password_reset_from(&newer, SignIn::Session(&s)).await });
    // ...the issue waits for the claimed link...
    waits_for_backend(&url, confirm_pid, &issue, "the issue").await;
    // ...and the confirm locks the account's row, as it does next, then commits.
    let locked = sqlx::query_scalar::<_, bool>("SELECT is_active FROM users WHERE id = $1 FOR UPDATE").bind(&member.id).fetch_one(&mut *claimed).await;
    assert!(locked.is_ok(), "the confirm locks the account's row, no deadlock: {locked:?}");
    claimed.commit().await.unwrap();
    assert!(issue.await.expect("the task").expect("the issue, once the confirm committed"), "the new link is stored");
    assert!(db.get_password_reset(&format!("newer-{tag}")).await.unwrap().is_some_and(|r| r.used_at.is_none()), "and usable");

    // 5: sent while the session's own password change runs: stored.
    for what in ["invitation", "link"] {
        let mut changer = sqlx::PgConnection::connect(&url).await.expect("a connection for the password change");
        let changer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut changer).await.unwrap();
        let mut change = changer.begin().await.unwrap();
        sqlx::query("UPDATE users SET sign_in_epoch = sign_in_epoch + 1 WHERE id = $1").bind(&admin.id).execute(&mut *change).await.unwrap();
        let (racer, s) = (db.clone(), sid.clone());
        let send = if what == "invitation" {
            let inv = Invitation {
                id: Uuid::new_v4().to_string(),
                email: format!("own-change-{tag}@example.com"),
                role: Role::Member,
                token: format!("own-change-{tag}"),
                invited_by: Some(admin.id.clone()),
                org_id: DEFAULT_ORG_ID.to_string(),
                expires_at: chrono::Utc::now() + chrono::Duration::days(7),
                used_at: None,
                created_at: chrono::Utc::now(),
            };
            tokio::spawn(async move { racer.create_invitation_from(&inv, SignIn::Session(&s)).await })
        } else {
            let reset = link("own-change", Some(&admin.id));
            tokio::spawn(async move { racer.create_password_reset_from(&reset, SignIn::Session(&s)).await })
        };
        waits_for_backend(&url, changer_pid, &send, what).await;
        let restamp = sqlx::query("UPDATE sessions SET sign_in_epoch = (SELECT sign_in_epoch FROM users WHERE id = $1) WHERE id = $2")
            .bind(&admin.id)
            .bind(&sid)
            .execute(&mut *change)
            .await;
        assert!(restamp.is_ok(), "{what}: the change re-stamps its session without waiting for the send: {restamp:?}");
        change.commit().await.unwrap();
        assert!(send.await.expect("the task").expect("the send"), "{what}: stored, from the session the change kept");
    }

    // Review of round 8: a role change with its membership never changes an
    // Owner's role: it fails and writes nothing. (The order its locks are
    // taken in: account_rows_are_locked_before_links_and_memberships_….)
    let owner = crate::types::User { email: format!("r8o-{tag}@example.com"), role: Role::Owner, ..first_user(2) };
    db.create_user(&owner).await.unwrap();
    db.add_org_member(DEFAULT_ORG_ID, &owner.id, Role::Owner).await.unwrap();
    assert!(db.update_user_with_revoked(&owner.id, Some(Role::Member), None, None, Some(DEFAULT_ORG_ID)).await.is_err());
    let after = db.get_user_by_id(&owner.id).await.unwrap().unwrap();
    assert_eq!((after.role, db.get_membership(DEFAULT_ORG_ID, &owner.id).await.unwrap()), (Role::Owner, Some(Role::Owner)));
}

/// Review of round 8 (the security reviewer): every change that touches an
/// account's memberships or links locks the account's row first: a confirm,
/// a role change with its membership, and a transfer of ownership. A link's
/// issue locks its issuer's row first too. With one order, no two of them
/// deadlock (an issue against a transfer away from its issuer did). Each
/// change here runs while another connection holds the account's row: it
/// waits there holding nothing else, so the account's link and membership
/// stay free (a NOWAIT lock on them succeeds). A database of its own: a
/// transfer demotes every Owner.
#[tokio::test]
async fn account_rows_are_locked_before_links_and_memberships_against_postgres() {
    use crate::config::Mode;
    use crate::db::ResetConfirm;
    use crate::types::{PasswordReset, Role};
    use sqlx::Connection;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping account_rows_are_locked_before_links_and_memberships_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let (server, name, db) = own_database(&url, "litegen_lock_order").await;
    let db = std::sync::Arc::new(db);
    let mut scratch_url = reqwest::Url::parse(&url).unwrap();
    scratch_url.set_path(&format!("/{name}"));
    let scratch = scratch_url.to_string();
    let owner = crate::types::User { email: "lo-owner@example.com".into(), role: Role::Owner, ..first_user(0) };
    db.create_user(&owner).await.unwrap();
    db.add_org_member(DEFAULT_ORG_ID, &owner.id, Role::Owner).await.unwrap();
    let member = crate::types::User { email: "lo-member@example.com".into(), role: Role::Member, ..first_user(1) };
    db.create_user(&member).await.unwrap();
    db.add_org_member(DEFAULT_ORG_ID, &member.id, Role::Member).await.unwrap();
    let now = chrono::Utc::now();
    db.create_password_reset(&PasswordReset { token: "lo-link".into(), user_id: member.id.clone(), expires_at: now + chrono::Duration::hours(1), used_at: None, created_at: now, issued_by: None })
        .await
        .unwrap();

    // Holds the member's row while `start` runs; once `start` waits for it,
    // `free` must lock without waiting. Then lets go.
    async fn while_the_row_is_held<T: Send + 'static>(
        url: &str,
        user_id: &str,
        free: &str,
        what: &str,
        start: impl FnOnce() -> tokio::task::JoinHandle<T>,
    ) -> T {
        let mut holder = sqlx::PgConnection::connect(url).await.expect("a connection to hold the row");
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut holder).await.unwrap();
        let mut held = holder.begin().await.unwrap();
        sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR NO KEY UPDATE").bind(user_id).execute(&mut *held).await.unwrap();
        let task = start();
        waits_for_backend(url, holder_pid, &task, what).await;
        let mut other = sqlx::PgConnection::connect(url).await.expect("a connection to try the lock");
        let tried = sqlx::query(free).execute(&mut other).await;
        assert!(tried.is_ok(), "{what} holds a lock it took before the account's row: {tried:?}");
        held.commit().await.unwrap();
        task.await.expect("the task")
    }

    let link_free = format!("SELECT 1 FROM password_resets WHERE token = '{}' FOR UPDATE NOWAIT", crate::auth::tokens::token_hash("lo-link"));
    let racer = db.clone();
    let confirmed = while_the_row_is_held(&scratch, &member.id, &link_free, "the confirm", move || {
        tokio::spawn(async move { racer.confirm_password_reset("lo-link", "new-hash", Mode::SingleTenant).await })
    })
    .await;
    assert!(matches!(confirmed.expect("the confirm"), ResetConfirm::Done { .. }));

    let membership_free = format!("SELECT 1 FROM organization_members WHERE org_id = '{DEFAULT_ORG_ID}' AND user_id = '{}' FOR UPDATE NOWAIT", member.id);
    let (racer, id) = (db.clone(), member.id.clone());
    let changed = while_the_row_is_held(&scratch, &member.id, &membership_free, "the role change", move || {
        tokio::spawn(async move { racer.update_user_with_revoked(&id, Some(Role::Viewer), None, None, Some(DEFAULT_ORG_ID)).await })
    })
    .await;
    assert_eq!(changed.expect("the role change").expect("the account").0.role, Role::Viewer);

    let (racer, id) = (db.clone(), member.id.clone());
    let transferred = while_the_row_is_held(&scratch, &member.id, &membership_free, "the transfer", move || {
        tokio::spawn(async move { racer.transfer_deployment_owner(DEFAULT_ORG_ID, &id).await })
    })
    .await;
    transferred.expect("the transfer");
    assert_eq!(db.get_membership(DEFAULT_ORG_ID, &member.id).await.unwrap(), Some(Role::Owner));

    db.pool().close().await;
    drop_own_database(&server, &name).await;
}

/// A database of its own on the test server, migrated, for a test that
/// needs one empty (the first signup counts every account): the server
/// connection and the name, to drop it after.
async fn own_database(url: &str, prefix: &str) -> (sqlx::PgPool, String, PostgresDatabase) {
    use sqlx::postgres::PgPoolOptions;
    let server = PgPoolOptions::new().max_connections(1).connect(url).await.expect("connect to the test Postgres");
    let name = format!("{prefix}_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&server).await.expect("create a scratch database");
    let mut scratch_url = reqwest::Url::parse(url).expect("LITEGEN_PG_TEST_URL is a URL");
    scratch_url.set_path(&format!("/{name}"));
    let db = PostgresDatabase::connect(scratch_url.as_str()).await.expect("connect + migrate the scratch database");
    (server, name, db)
}

/// Drops a database [`own_database`] made, ending any session still on it
/// (a closed pool's server sessions can outlive it for a moment).
async fn drop_own_database(server: &sqlx::PgPool, name: &str) {
    sqlx::query(&format!("DROP DATABASE {name} WITH (FORCE)")).execute(server).await.expect("drop the scratch database");
}

/// The `i`th would-be first Owner of a first-owner test.
fn first_user(i: usize) -> crate::types::User {
    crate::types::User {
        id: Uuid::new_v4().to_string(),
        email: format!("first{i}@example.com"),
        password_hash: None,
        role: crate::types::Role::Owner,
        oauth_github_id: None,
        oauth_google_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        last_login_at: None,
        is_active: true,
        sign_in_epoch: 0,
    }
}

/// Postgres twin of `a_first_owner_whose_membership_fails_leaves_no_account`.
#[tokio::test]
async fn a_first_owner_whose_membership_fails_leaves_no_account_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_first_owner_whose_membership_fails_leaves_no_account_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let (server, name, db) = own_database(&url, "litegen_first_owner_fails").await;
    let first = first_user(0);
    let failed = db.create_first_owner(&first, "no-such-org").await;
    let after_failure = db.count_users().await;
    let retried = db.create_first_owner(&first, DEFAULT_ORG_ID).await;
    db.pool().close().await;
    drop_own_database(&server, &name).await;

    assert!(failed.is_err(), "the membership's org must exist");
    assert_eq!(after_failure.unwrap(), 0, "no account left behind");
    assert!(retried.unwrap(), "the first signup is still open");
}

/// Postgres twin of `first_owners_at_once_make_one`: under READ COMMITTED
/// each of them would count no account, so the claim decides. A later one
/// finds signup closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_owners_at_once_make_one_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping first_owners_at_once_make_one_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let (server, name, db) = own_database(&url, "litegen_first_owners").await;
    let db = std::sync::Arc::new(db);
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(8));
    let tries: Vec<_> = (1..=8)
        .map(|i| {
            let (db, start, user) = (db.clone(), start.clone(), first_user(i));
            tokio::spawn(async move {
                start.wait().await;
                db.create_first_owner(&user, DEFAULT_ORG_ID).await
            })
        })
        .collect();
    let mut outcomes = Vec::new();
    for t in tries {
        outcomes.push(t.await.unwrap());
    }
    let later = db.create_first_owner(&first_user(9), DEFAULT_ORG_ID).await;
    let accounts = db.count_users().await;
    let owners = db.list_org_members(DEFAULT_ORG_ID).await;
    db.pool().close().await;
    drop_own_database(&server, &name).await;

    let won = outcomes.into_iter().map(|o| o.expect("a first signup that loses hears signup is closed")).filter(|w| *w).count();
    assert_eq!(won, 1, "one first Owner");
    assert!(!later.unwrap(), "a later first signup finds it closed");
    assert_eq!(accounts.unwrap(), 1);
    assert_eq!(owners.unwrap().into_iter().filter(|m| m.role == crate::types::Role::Owner).count(), 1);
}

/// Postgres twin of the SQLite reset tests: stored hashed, a new link voids
/// the older, a used one writes nothing, deactivating deletes the unused
/// link, and one issued after that reactivates the account (round 3b, S1).
#[tokio::test]
async fn reset_tokens_are_hashed_single_use_and_reactivate_against_postgres() {
    use crate::db::ResetConfirm;
    use crate::types::PasswordReset;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping reset_tokens_are_hashed_single_use_and_reactivate_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let user = crate::types::User { email: format!("reset-{}@example.com", Uuid::new_v4().simple()), password_hash: Some("old-hash".into()), ..first_user(0) };
    db.create_user(&user).await.unwrap();
    let reset = |token: &str| PasswordReset {
        token: token.to_string(),
        user_id: user.id.clone(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        used_at: None,
        created_at: chrono::Utc::now(),
        issued_by: None,
    };
    let tag = Uuid::new_v4().simple().to_string();
    let (first, second, before, after) = (format!("first-{tag}"), format!("second-{tag}"), format!("before-{tag}"), format!("after-{tag}"));

    db.create_password_reset(&reset(&first)).await.unwrap();
    let stored: Vec<String> = sqlx::query_scalar("SELECT token FROM password_resets WHERE user_id = $1").bind(&user.id).fetch_all(db.pool()).await.unwrap();
    assert_eq!(stored, [crate::auth::tokens::token_hash(&first)], "only the hash is kept");
    db.create_password_reset(&reset(&second)).await.unwrap();
    assert!(db.get_password_reset(&first).await.unwrap().is_none(), "the older one is void");

    assert!(db.mark_password_reset_used(&second).await.unwrap());
    assert_eq!(db.confirm_password_reset(&second, "new-hash", crate::config::Mode::SingleTenant).await.unwrap(), ResetConfirm::Unusable);
    assert_eq!(db.get_user_by_id(&user.id).await.unwrap().unwrap().password_hash.as_deref(), Some("old-hash"), "a used token writes no password");

    db.create_password_reset(&reset(&before)).await.unwrap();
    db.update_user(&user.id, None, Some(false), None).await.unwrap();
    assert_eq!(db.confirm_password_reset(&before, "new-hash", crate::config::Mode::SingleTenant).await.unwrap(), ResetConfirm::Unusable, "deactivating deleted it");
    db.create_password_reset(&reset(&after)).await.unwrap();
    assert_eq!(db.confirm_password_reset(&after, "new-hash", crate::config::Mode::SingleTenant).await.unwrap(), ResetConfirm::Done { user_id: user.id.clone(), reactivated: true, revoked_invitations: vec![] });
    let now = db.get_user_by_id(&user.id).await.unwrap().unwrap();
    assert_eq!((now.is_active, now.password_hash.as_deref()), (true, Some("new-hash")), "reactivated, with the new password");
}

/// A session, an OAuth grant with an access token, and an approved code not
/// yet redeemed, all `user_id`'s, on `db`: (session's stored id, grant id,
/// token hash, code hash).
async fn pg_sign_ins_for(db: &PostgresDatabase, user_id: &str) -> (String, String, String, String) {
    use crate::mcp::oauth::store::{AuthRequest, Decision, Grant, TokenKind, TokenRow};
    use crate::mcp::Surface;
    let now = chrono::Utc::now();
    let tag = Uuid::new_v4().simple().to_string();
    let session = format!("sess-{tag}");
    db.create_session(&crate::types::Session { id: session.clone(), user_id: user_id.into(), created_at: now, expires_at: now + chrono::Duration::days(7), ip: None, user_agent: None, csrf_token: "csrf".into() })
        .await
        .unwrap();
    let oauth = db.mcp_oauth().expect("an OAuth store");
    let (grant, token, request, code) = (format!("grant-{tag}"), format!("tok-{tag}"), format!("req-{tag}"), format!("code-{tag}"));
    let scopes = vec!["admin:read".to_string()];
    oauth
        .insert_grant(&Grant { id: grant.clone(), client_id: format!("lgc-{tag}"), user_id: user_id.into(), org_id: DEFAULT_ORG_ID.into(), app_id: None, surface: Surface::Admin, scopes: scopes.clone(), created_at: now, last_used_at: None, revoked_at: None, sign_in_epoch: 0 })
        .await
        .unwrap();
    oauth
        .insert_token(&TokenRow { token_hash: token.clone(), grant_id: grant.clone(), kind: TokenKind::Access, created_at: now, expires_at: now + chrono::Duration::hours(1), rotated_at: None, revoked_at: None })
        .await
        .unwrap();
    oauth
        .insert_request(&AuthRequest {
            id: request.clone(),
            client_id: format!("lgc-{tag}"),
            redirect_uri: "http://127.0.0.1/callback".into(),
            state: None,
            code_challenge: "challenge".into(),
            scopes: scopes.clone(),
            surface: Surface::Admin,
            created_at: now,
            expires_at: now + chrono::Duration::minutes(5),
            user_id: None,
            org_id: None,
            app_id: None,
            granted_scopes: None,
            code_hash: None,
            decided_at: None,
            consumed_at: None,
            sign_in_epoch: None,
        })
        .await
        .unwrap();
    let approve = Decision::Approve { user_id: user_id.into(), org_id: DEFAULT_ORG_ID.into(), app_id: None, scopes, code_hash: code.clone(), code_expires_at: now + chrono::Duration::seconds(60), at: now, sign_in_epoch: 0 };
    assert!(oauth.decide_request(&request, approve).await.unwrap(), "approved");
    (crate::auth::tokens::token_hash(&session), grant, token, code)
}

/// Whether each of [`pg_sign_ins_for`]'s sign-ins still works.
async fn pg_sign_ins_alive(db: &PostgresDatabase, (session, grant, token, code): &(String, String, String, String)) -> [bool; 4] {
    let oauth = db.mcp_oauth().expect("an OAuth store");
    let code_alive: Option<i32> = sqlx::query_scalar("SELECT 1 FROM mcp_oauth_requests WHERE code_hash = $1 AND consumed_at IS NULL")
        .bind(code)
        .fetch_optional(db.pool())
        .await
        .unwrap();
    [
        db.get_session(session).await.unwrap().is_some(),
        oauth.get_grant(grant).await.unwrap().is_some_and(|g| g.revoked_at.is_none()),
        oauth.get_token(token).await.unwrap().is_some_and(|t| t.revoked_at.is_none()),
        code_alive.is_some(),
    ]
}

/// Postgres twin of round 3b's S1 and S2 store tests, on a database of its
/// own (a table is renamed to make a step fail): deactivating, and confirming
/// a reset, each end the sessions, grants, tokens and codes in one
/// transaction, or change nothing.
#[tokio::test]
async fn deactivating_and_resetting_end_every_sign_in_at_once_against_postgres() {
    use crate::db::ResetConfirm;
    use crate::types::PasswordReset;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping deactivating_and_resetting_end_every_sign_in_at_once_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let (server, name, db) = own_database(&url, "litegen_sign_ins").await;
    let reset = |user_id: &str, token: &str| PasswordReset {
        token: token.to_string(),
        user_id: user_id.to_string(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        used_at: None,
        created_at: chrono::Utc::now(),
        issued_by: None,
    };
    let leaving = crate::types::User { password_hash: Some("old-hash".into()), ..first_user(1) };
    let stolen = crate::types::User { password_hash: Some("old-hash".into()), ..first_user(2) };
    for user in [&leaving, &stolen] {
        db.create_user(user).await.unwrap();
    }
    let (leaving_sign_ins, stolen_sign_ins) = (pg_sign_ins_for(&db, &leaving.id).await, pg_sign_ins_for(&db, &stolen.id).await);
    db.create_password_reset(&reset(&leaving.id, "pending")).await.unwrap();
    db.create_password_reset(&reset(&stolen.id, "rescue")).await.unwrap();

    sqlx::query("ALTER TABLE mcp_oauth_requests RENAME TO mcp_oauth_requests_away").execute(db.pool()).await.unwrap();
    let deactivated = db.update_user(&leaving.id, None, Some(false), None).await;
    let confirmed = db.confirm_password_reset("rescue", "new-hash", crate::config::Mode::SingleTenant).await;
    sqlx::query("ALTER TABLE mcp_oauth_requests_away RENAME TO mcp_oauth_requests").execute(db.pool()).await.unwrap();
    assert!(deactivated.is_err() && confirmed.is_err(), "the codes couldn't be deleted");
    assert!(db.get_user_by_id(&leaving.id).await.unwrap().unwrap().is_active, "still active");
    assert_eq!(pg_sign_ins_alive(&db, &leaving_sign_ins).await, [true; 4]);
    assert!(db.get_password_reset("pending").await.unwrap().is_some());
    assert_eq!(db.get_user_by_id(&stolen.id).await.unwrap().unwrap().password_hash.as_deref(), Some("old-hash"), "no password written");
    assert_eq!(pg_sign_ins_alive(&db, &stolen_sign_ins).await, [true; 4]);
    assert!(db.get_password_reset("rescue").await.unwrap().is_some_and(|r| r.used_at.is_none()), "the link stays usable");

    db.update_user(&leaving.id, None, Some(false), None).await.unwrap().expect("deactivated");
    assert_eq!(pg_sign_ins_alive(&db, &leaving_sign_ins).await, [false; 4], "deactivating: session, grant, token, code");
    assert!(db.get_password_reset("pending").await.unwrap().is_none(), "its unused link is gone");
    assert_eq!(db.confirm_password_reset("rescue", "new-hash", crate::config::Mode::SingleTenant).await.unwrap(), ResetConfirm::Done { user_id: stolen.id.clone(), reactivated: false, revoked_invitations: vec![] });
    assert_eq!(pg_sign_ins_alive(&db, &stolen_sign_ins).await, [false; 4], "a reset: session, grant, token, code");
    db.pool().close().await;
    drop_own_database(&server, &name).await;
}

/// Postgres twin of round 3b's S3 test (migration 042's triggers), on a
/// database of its own (an owner transfer moves every account's role): any
/// role, ownership or membership change deletes the account's unused links,
/// whichever code makes it; signing in doesn't, and a used link stays.
#[tokio::test]
async fn unused_reset_links_follow_roles_against_postgres() {
    use crate::types::{Invitation, PasswordReset, Role};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping unused_reset_links_follow_roles_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let (server, name, db) = own_database(&url, "litegen_links_follow_roles").await;
    let user = crate::types::User { role: Role::Member, ..first_user(3) };
    db.create_user(&user).await.unwrap();
    db.add_org_member(DEFAULT_ORG_ID, &user.id, Role::Member).await.unwrap();
    let other = crate::types::Organization {
        id: format!("org-{}", Uuid::new_v4().simple()),
        name: "Other".into(),
        slug: format!("other-{}", Uuid::new_v4().simple()),
        plan: "free".into(),
        status: "active".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    db.create_organization(&other).await.unwrap();
    db.create_invitation(&Invitation {
        id: Uuid::new_v4().to_string(),
        email: user.email.clone(),
        role: Role::Member,
        token: "join-other".into(),
        invited_by: None,
        org_id: other.id.clone(),
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        used_at: None,
        created_at: chrono::Utc::now(),
    })
    .await
    .unwrap();
    let link = |token: &str| PasswordReset {
        token: token.to_string(),
        user_id: user.id.clone(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        used_at: None,
        created_at: chrono::Utc::now(),
        issued_by: None,
    };
    db.create_password_reset(&link("used-link")).await.unwrap();
    assert!(db.mark_password_reset_used("used-link").await.unwrap());
    for (n, what) in ["a role change in an org", "an account role change", "accepting an invitation to another org", "becoming that org's Owner", "becoming the deployment's Owner", "leaving an org", "joining an org", "being deactivated"].into_iter().enumerate() {
        let token = format!("link-{n}");
        db.create_password_reset(&link(&token)).await.unwrap();
        db.touch_last_login(&user.id).await.unwrap();
        assert!(db.get_password_reset(&token).await.unwrap().is_some(), "{what}: signing in keeps the link");
        match n {
            0 => db.update_member_role(DEFAULT_ORG_ID, &user.id, Role::Viewer).await.unwrap(),
            1 => assert!(db.update_user(&user.id, Some(Role::Admin), None, None).await.unwrap().is_some()),
            2 => assert!(db.accept_invitation("join-other", None, &user.id, &other.id, Role::Member).await.unwrap()),
            3 => db.transfer_org_owner(&other.id, &user.id).await.unwrap(),
            4 => db.transfer_owner(&user.id).await.unwrap(),
            5 => db.remove_org_member(DEFAULT_ORG_ID, &user.id).await.unwrap(),
            6 => db.add_org_member(DEFAULT_ORG_ID, &user.id, Role::Member).await.unwrap(),
            _ => assert!(db.update_user(&user.id, None, Some(false), None).await.unwrap().is_some()),
        }
        assert!(db.get_password_reset(&token).await.unwrap().is_none(), "{what} deletes the unused link");
    }
    assert!(db.get_password_reset("used-link").await.unwrap().is_some(), "a used link stays");
    db.pool().close().await;
    drop_own_database(&server, &name).await;
}

/// Postgres twin of the SQLite invitation-token and acceptance tests:
/// stored hashed, a pre-041 plain row still found, one revoked meanwhile
/// accepted with nothing written, and acceptance racing revocation to one
/// outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invitations_are_hashed_and_accepted_atomically_against_postgres() {
    use crate::types::{Invitation, Role};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping invitations_are_hashed_and_accepted_atomically_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = std::sync::Arc::new(PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
    let invitation = |token: &str| Invitation {
        id: Uuid::new_v4().to_string(),
        email: format!("{}@example.com", Uuid::new_v4().simple()),
        role: Role::Member,
        token: token.to_string(),
        invited_by: None,
        org_id: DEFAULT_ORG_ID.to_string(),
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        used_at: None,
        created_at: chrono::Utc::now(),
    };
    let newcomer = |n: usize| crate::types::User { email: format!("racer-{}-{n}@example.com", Uuid::new_v4().simple()), ..first_user(n) };
    let tag = Uuid::new_v4().simple().to_string();

    let inv = invitation(&format!("plain-{tag}"));
    db.create_invitation(&inv).await.unwrap();
    let stored: String = sqlx::query_scalar("SELECT token FROM invitations WHERE id = $1").bind(&inv.id).fetch_one(db.pool()).await.unwrap();
    assert_eq!(stored, crate::auth::tokens::token_hash(&format!("plain-{tag}")), "only the hash is kept");
    assert!(db.get_invitation(&stored).await.unwrap().is_none(), "what is stored is no token");
    let legacy = format!("legacy-{tag}");
    sqlx::query("INSERT INTO invitations (id, email, role, token, org_id, expires_at, created_at) VALUES ($1, $2, 'member', $3, $4, $5, $6)")
        .bind(format!("old-{tag}"))
        .bind(format!("old-{tag}@example.com"))
        .bind(&legacy)
        .bind(DEFAULT_ORG_ID)
        .bind(chrono::Utc::now() + chrono::Duration::days(7))
        .bind(chrono::Utc::now())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(db.get_invitation(&legacy).await.unwrap().is_some(), "a pre-041 row");

    let revoked = invitation(&format!("revoked-{tag}"));
    db.create_invitation(&revoked).await.unwrap();
    assert!(db.revoke_invitation(DEFAULT_ORG_ID, &revoked.id).await.unwrap().is_some());
    let late = newcomer(100);
    assert!(!db.accept_invitation(&format!("revoked-{tag}"), Some(&late), &late.id, DEFAULT_ORG_ID, Role::Member).await.unwrap());
    assert!(db.get_user_by_id(&late.id).await.unwrap().is_none() && db.get_membership(DEFAULT_ORG_ID, &late.id).await.unwrap().is_none(), "nothing written");

    for round in 0..20 {
        let token = format!("race-{tag}-{round}");
        let inv = invitation(&token);
        db.create_invitation(&inv).await.unwrap();
        let racer = newcomer(round);
        let start = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let accept = {
            let (db, start, racer) = (db.clone(), start.clone(), racer.clone());
            tokio::spawn(async move {
                start.wait().await;
                db.accept_invitation(&token, Some(&racer), &racer.id, DEFAULT_ORG_ID, Role::Member).await
            })
        };
        let revoke = {
            let (db, start, id) = (db.clone(), start.clone(), inv.id.clone());
            tokio::spawn(async move {
                start.wait().await;
                db.revoke_invitation(DEFAULT_ORG_ID, &id).await
            })
        };
        let accepted = accept.await.unwrap().expect("accept");
        let revoked = revoke.await.unwrap().expect("revoke").is_some();
        let member = db.get_membership(DEFAULT_ORG_ID, &racer.id).await.unwrap().is_some();
        let account = db.get_user_by_id(&racer.id).await.unwrap().is_some();
        assert!(accepted != revoked, "round {round}: exactly one wins (accepted {accepted}, revoked {revoked})");
        assert_eq!((member, account), (accepted, accepted), "round {round}: the account and membership go with acceptance");
    }
}

/// Postgres twin of `an_org_and_its_owner_are_made_together_or_not_at_all`.
#[tokio::test]
async fn an_org_and_its_owner_are_made_together_or_not_at_all_against_postgres() {
    use crate::db::OrgOwner;
    use crate::types::{Application, Organization, Role};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping an_org_and_its_owner_are_made_together_or_not_at_all_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let now = chrono::Utc::now();
    let org_and_app = |name: &str, app_org: Option<&str>| {
        let org = Organization {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            slug: format!("{}-{}", name.to_lowercase(), Uuid::new_v4().simple()),
            plan: "free".into(),
            status: "active".into(),
            created_at: now,
            updated_at: now,
        };
        let app = Application {
            id: Uuid::new_v4().to_string(),
            org_id: app_org.map_or_else(|| org.id.clone(), str::to_string),
            name: "Default".into(),
            slug: "default".into(),
            status: "active".into(),
            created_at: now,
            updated_at: now,
        };
        (org, app)
    };
    let signer = crate::types::User { email: format!("signer-{}@example.com", Uuid::new_v4().simple()), ..first_user(0) };

    let (org, app) = org_and_app("Signer", Some("no-such-org"));
    assert!(db.create_org_with_owner(&org, &app, OrgOwner::New(&signer)).await.is_err(), "the app's org must exist");
    assert!(db.get_user_by_id(&signer.id).await.unwrap().is_none(), "no account left behind");
    assert!(db.get_organization(&org.id).await.unwrap().is_none(), "no org left behind");

    let (org, app) = org_and_app("Orphan", None);
    assert!(db.create_org_with_owner(&org, &app, OrgOwner::Existing("no-such-user")).await.is_err(), "the owner must exist");
    assert!(db.get_organization(&org.id).await.unwrap().is_none(), "no org left behind");

    let (org, app) = org_and_app("Signer", None);
    db.create_org_with_owner(&org, &app, OrgOwner::New(&signer)).await.unwrap();
    assert!(db.get_user_by_id(&signer.id).await.unwrap().is_some());
    assert_eq!(db.get_membership(&org.id, &signer.id).await.unwrap(), Some(Role::Owner));
    assert_eq!(db.list_apps_for_org(&org.id).await.unwrap().into_iter().map(|a| a.id).collect::<Vec<_>>(), [app.id]);
}

// ─── Statement and lock timeouts (follow-ups 2026-11-15) ─────────────────────

/// The SQLSTATE of `e`, when the database raised it.
fn sqlstate(e: &sqlx::Error) -> Option<String> {
    e.as_database_error().and_then(|d| d.code()).map(|c| c.into_owned())
}

/// `url` with one more query parameter.
fn with_param(url: &str, param: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}{param}")
}

/// A statement of litegen's that waits on a row another transaction holds
/// gives up at the lock timeout. Without one it waited for as long as that
/// transaction ran, holding a pool connection and, in the poller, a slot.
#[tokio::test]
async fn a_statement_waiting_on_a_held_row_gives_up_at_the_lock_timeout_against_postgres() {
    use sqlx::Connection;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_statement_waiting_on_a_held_row_gives_up_at_the_lock_timeout_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let limits = super::postgres::StatementLimits {
        statement: std::time::Duration::from_secs(60),
        lock: std::time::Duration::from_millis(200),
    };
    let db = PostgresDatabase::connect_with_limits(&url, limits).await.expect("connect + migrate against the test Postgres");
    let id = format!("litegen-lock-timeout-{}", Uuid::new_v4());
    db.insert_generation(&id, None, "mock/video-gen", "mock", "video", Some("job"), 0.0, None, None).await.unwrap();

    let mut holder = sqlx::PgConnection::connect(&url).await.expect("a connection to hold the row");
    let mut held = holder.begin().await.unwrap();
    sqlx::query("SELECT id FROM generations WHERE id = $1 FOR UPDATE").bind(&id).execute(&mut *held).await.unwrap();
    let update = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        db.update_generation_if_active(&id, "processing", 10, None, None, None, None),
    )
    .await;
    held.rollback().await.unwrap();

    let update = update.expect("the update still waited on the held row after 10 s");
    let err = update.expect_err("the update went through a row another transaction held");
    assert_eq!(sqlstate(&err).as_deref(), Some("55P03"), "not a lock timeout: {err}");
}

/// No statement of litegen's runs past the statement timeout.
#[tokio::test]
async fn a_statement_stops_at_the_statement_timeout_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_statement_stops_at_the_statement_timeout_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let limits = super::postgres::StatementLimits {
        statement: std::time::Duration::from_millis(200),
        lock: std::time::Duration::from_secs(60),
    };
    let db = PostgresDatabase::connect_with_limits(&url, limits).await.expect("connect + migrate against the test Postgres");
    let slow = tokio::time::timeout(std::time::Duration::from_secs(10), sqlx::query("SELECT pg_sleep(5)").execute(db.pool())).await;
    let slow = slow.expect("the statement still ran after 10 s");
    let err = slow.expect_err("a 5 s statement ran to its end past a 200 ms statement timeout");
    assert_eq!(sqlstate(&err).as_deref(), Some("57014"), "not a statement timeout: {err}");
}

/// litegen's limits apply only where the connection has none: one the
/// operator set (here in the URL's `options`; `PGOPTIONS`, the role, the
/// database and the server's configuration count the same) stays, and so
/// does a `0` set that way, which turns that limit off.
#[tokio::test]
async fn an_operators_own_timeout_wins_over_litegens_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping an_operators_own_timeout_wins_over_litegens_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let limits = super::postgres::StatementLimits {
        statement: std::time::Duration::from_secs(60),
        lock: std::time::Duration::from_millis(250),
    };
    let settings = |options: &'static str| {
        let url = with_param(&url, options);
        async move {
            let db = PostgresDatabase::connect_with_limits(&url, limits).await.expect("connect + migrate against the test Postgres");
            let (statement, lock): (String, String) =
                sqlx::query_as("SELECT current_setting('statement_timeout'), current_setting('lock_timeout')")
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            (statement, lock)
        }
    };
    let (statement, lock) = settings("options=-c%20statement_timeout%3D7s").await;
    assert_eq!((statement.as_str(), lock.as_str()), ("7s", "250ms"), "the operator's statement timeout and litegen's lock timeout");
    let (statement, lock) = settings("options=-c%20lock_timeout%3D0").await;
    assert_eq!((statement.as_str(), lock.as_str()), ("1min", "0"), "litegen's statement timeout and the operator's 0");
}

/// The limits never cut a migration: migrations run first, on a connection
/// of their own without them. Here another transaction holds a lock the
/// migrator needs (`_sqlx_migrations`, which it reads first) for five times
/// the lock timeout, and the migrator waits it out.
#[tokio::test]
async fn migrations_wait_out_a_lock_past_the_lock_timeout_against_postgres() {
    use sqlx::Connection;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping migrations_wait_out_a_lock_past_the_lock_timeout_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let mut holder = sqlx::PgConnection::connect(&url).await.expect("a connection to hold the lock");
    let mut held = holder.begin().await.unwrap();
    sqlx::query("LOCK TABLE _sqlx_migrations IN ACCESS EXCLUSIVE MODE").execute(&mut *held).await.unwrap();

    // Its connections carry a name of their own, so Postgres can say which
    // of them wait for a lock. Other tests migrate the same database, so the
    // migrator may wait for theirs (the migrations' advisory lock, held by a
    // migrator that waits for this one): a wait all the same.
    let tagged = with_param(&url, "application_name=litegen-migrations-lock-test");
    let limits = super::postgres::StatementLimits {
        statement: std::time::Duration::from_secs(60),
        lock: std::time::Duration::from_millis(100),
    };
    let connecting = tokio::spawn(async move { PostgresDatabase::connect_with_limits(&tagged, limits).await.map(|_| ()) });
    let mut observer = sqlx::PgConnection::connect(&url).await.expect("a connection to watch the locks");
    let waiting = || {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE application_name = 'litegen-migrations-lock-test' AND wait_event_type = 'Lock'",
        )
    };
    let started_waiting = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while waiting().fetch_one(&mut observer).await.unwrap() == 0 && !connecting.is_finished() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(started_waiting.is_ok(), "the migrations never waited on the held lock");
    tokio::time::sleep(5 * limits.lock).await;
    let still_waiting = waiting().fetch_one(&mut observer).await.unwrap();
    let finished = connecting.is_finished();
    held.rollback().await.unwrap();
    let connected = tokio::time::timeout(std::time::Duration::from_secs(30), connecting).await;

    assert!(!finished && still_waiting >= 1, "the migrations stopped waiting for the lock: {connected:?}");
    connected.expect("connect, once the lock was freed").expect("the connect's task").expect("migrations, once the lock was freed");
}

// ─── the webhook outbox (migration 029) ─────────────────────────────────────

/// The outbox tests take turns: a claim takes any due delivery in the
/// shared database, so two of these at once would take each other's. Each
/// leaves none of its own due (deleted with its app, or failed).
static OUTBOX_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A fresh org with one app, for the outbox tests: (org id, app id).
async fn outbox_tenant(db: &PostgresDatabase, tag: &str) -> (String, String) {
    use crate::types::{Application, Organization};
    let now = chrono::Utc::now();
    let org = Organization {
        id: Uuid::new_v4().to_string(),
        name: format!("outbox-{tag}"),
        slug: format!("outbox-{tag}-{}", Uuid::new_v4().simple()),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_organization(&org).await.unwrap();
    let app = Application {
        id: Uuid::new_v4().to_string(),
        org_id: org.id.clone(),
        name: "a".into(),
        slug: format!("a-{}", Uuid::new_v4().simple()),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_application(&app).await.unwrap();
    (org.id, app.id)
}

/// A key of `org`/`app` with `webhook` as its webhook_url: its id.
async fn outbox_key(db: &PostgresDatabase, org: &str, app: &str, webhook: Option<&str>) -> Uuid {
    let tag = Uuid::new_v4().simple().to_string();
    db.create_api_key_scoped(org, app, &format!("pk_{tag}"), "k", &format!("hash-{tag}"), "lg-k", None, None, "generate,read", webhook, None)
        .await
        .unwrap()
        .id
}

/// The outbox's trigger on Postgres (029, replaced by 032) records a
/// delivery exactly where a terminal webhook is sent: a completion, a
/// failure, a cancel, a late image and an image row that fails, each once,
/// pending and due after its first-attempt grace (30 s); and none for a key
/// with no webhook_url or a cleared one (`""`), a job with no key, a sync
/// image result, a write that isn't terminal, or a second terminal write.
#[tokio::test]
async fn terminal_writes_record_webhook_deliveries_where_webhooks_are_sent_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping terminal_writes_record_webhook_deliveries_where_webhooks_are_sent_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let _turn = OUTBOX_TESTS.lock().await;
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let (org, app) = outbox_tenant(&db, "trigger").await;
    let hooked = outbox_key(&db, &org, &app, Some("https://hooks.example/h")).await;
    let plain = outbox_key(&db, &org, &app, None).await;
    let cleared = outbox_key(&db, &org, &app, Some("https://hooks.example/h")).await;
    sqlx::query("UPDATE api_keys SET webhook_url = '' WHERE id = $1").bind(cleared.to_string()).execute(db.pool()).await.unwrap();
    let t = Uuid::new_v4().simple().to_string();
    let id = |name: &str| format!("{name}-{t}");
    let job = |name: &str, key: Option<Uuid>, media: &'static str, vendor_job: Option<&'static str>| {
        let (db, org, app, id) = (&db, org.clone(), app.clone(), id(name));
        async move { db.insert_generation(&id, key.as_ref(), "mock/x", "mock", media, vendor_job, 0.0, Some(&org), Some(&app)).await.unwrap() }
    };
    let now = || Some(chrono::Utc::now());

    job("done", Some(hooked), "video", Some("j")).await;
    db.update_generation_if_active(&id("done"), "completed", 100, None, None, now(), None).await.unwrap();
    job("failed", Some(hooked), "model3d", Some("j")).await;
    db.update_generation_if_active(&id("failed"), "processing", 50, None, None, None, None).await.unwrap();
    db.update_generation_if_active(&id("failed"), "failed", 50, None, Some("x"), now(), Some(&serde_json::json!({ "x": 1 }))).await.unwrap();
    job("cancelled", Some(hooked), "video", Some("j")).await;
    db.cancel_generation(&id("cancelled")).await.unwrap().expect("cancelled");
    job("late-image", Some(hooked), "image", Some("j")).await;
    db.update_generation_if_active(&id("late-image"), "completed", 100, None, None, now(), None).await.unwrap();
    job("image-reaped", Some(hooked), "image", None).await;
    db.update_generation_if_active(&id("image-reaped"), "failed", 0, None, Some("timed out"), now(), None).await.unwrap();
    for name in ["done", "failed", "cancelled", "late-image", "image-reaped"] {
        let d = db.webhook_outbox_of(&id(name)).await.unwrap().unwrap_or_else(|| panic!("{name}: no delivery recorded"));
        assert_eq!((d.status.as_str(), d.attempts, d.key_id.clone()), ("pending", 0, hooked.to_string()), "{name}: {d:?}");
        assert!(d.id.strip_prefix("whd-").is_some_and(|u| Uuid::parse_str(u).is_ok()), "{name}: {}", d.id);
        let grace = chrono::Duration::from_std(crate::proxy::webhook::OUTBOX_FIRST_ATTEMPT_GRACE).unwrap();
        let wait = d.next_attempt_at - chrono::Utc::now();
        assert!(wait > grace - chrono::Duration::seconds(5) && wait <= grace + chrono::Duration::seconds(1), "{name}: due after the grace, in {wait:?}: {d:?}");
        assert_eq!((d.org_id.as_deref(), d.app_id.as_deref()), (Some(org.as_str()), Some(app.as_str())), "{name}");
    }

    job("plain", Some(plain), "video", Some("j")).await;
    db.update_generation_if_active(&id("plain"), "completed", 100, None, None, now(), None).await.unwrap();
    job("cleared", Some(cleared), "video", Some("j")).await;
    db.update_generation_if_active(&id("cleared"), "completed", 100, None, None, now(), None).await.unwrap();
    job("keyless", None, "video", Some("j")).await;
    db.update_generation_if_active(&id("keyless"), "failed", 0, None, Some("x"), now(), None).await.unwrap();
    job("sync-image", Some(hooked), "image", None).await;
    db.update_generation_if_active(&id("sync-image"), "completed", 100, None, None, now(), Some(&serde_json::json!({ "assets": [] }))).await.unwrap();
    job("running", Some(hooked), "video", Some("j")).await;
    db.update_generation_if_active(&id("running"), "processing", 40, None, None, None, None).await.unwrap();
    for name in ["plain", "cleared", "keyless", "sync-image", "running"] {
        assert_eq!(db.webhook_outbox_of(&id(name)).await.unwrap(), None, "{name}");
    }
    let first = db.webhook_outbox_of(&id("done")).await.unwrap();
    db.update_generation_status(&id("done"), "failed", 100, None, Some("again"), now()).await.unwrap();
    assert_eq!(db.webhook_outbox_of(&id("done")).await.unwrap(), first, "a row already terminal records no second delivery");

    assert!(db.delete_application(&app).await.unwrap());
    assert_eq!(db.webhook_outbox_of(&id("cancelled")).await.unwrap(), None, "deleting the app took its deliveries");
}

/// The claims on Postgres: two claimants at the same moment never take one
/// delivery, a claimant holds at most its share of one org's, a lease is
/// renewed by its holder only and passes on once it lapses, an attempt is
/// recorded only by the lease's holder and reschedules the delivery on the
/// database's clock, and a stop gives the leases back.
#[tokio::test]
async fn webhook_delivery_claims_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping webhook_delivery_claims_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let _turn = OUTBOX_TESTS.lock().await;
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let ((org_a, app_a), (org_b, app_b)) = (outbox_tenant(&db, "claims-a").await, outbox_tenant(&db, "claims-b").await);
    let key_a = outbox_key(&db, &org_a, &app_a, Some("https://hooks.example/a")).await;
    let key_b = outbox_key(&db, &org_b, &app_b, Some("https://hooks.example/b")).await;
    let t = Uuid::new_v4().simple().to_string();
    let mut mine = Vec::new();
    for (i, (key, org, app)) in [(key_a, &org_a, &app_a), (key_a, &org_a, &app_a), (key_a, &org_a, &app_a), (key_b, &org_b, &app_b)].into_iter().enumerate() {
        let id = format!("claims-{i}-{t}");
        db.insert_generation(&id, Some(&key), "mock/x", "mock", "video", Some("j"), 0.0, Some(org.as_str()), Some(app.as_str())).await.unwrap();
        db.update_generation_if_active(&id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
        mine.push(db.webhook_outbox_of(&id).await.unwrap().unwrap().id);
    }
    // Their first-attempt grace is over.
    sqlx::query("UPDATE webhook_outbox SET next_attempt_at = now() - interval '1 second' WHERE id = ANY($1)").bind(mine.clone()).execute(db.pool()).await.unwrap();
    let hold = chrono::Duration::seconds(60);
    let (a, b, c) = (format!("a-{t}"), format!("b-{t}"), format!("c-{t}"));
    // Other tests' due deliveries may be claimed too: only these four are judged.
    let of_mine = |claimed: Vec<crate::db::OutboxDelivery>| {
        let mut ids: Vec<String> = claimed.into_iter().map(|d| d.id).filter(|id| mine.contains(id)).collect();
        ids.sort();
        ids
    };

    // Two claimants at once, a share of 2 for one org: no delivery goes to both.
    let (by_a, by_b) = tokio::join!(db.claim_due_webhook_deliveries(&a, 100, 2, hold), db.claim_due_webhook_deliveries(&b, 100, 2, hold));
    let (by_a, by_b) = (of_mine(by_a.unwrap()), of_mine(by_b.unwrap()));
    assert!(by_a.iter().all(|id| !by_b.contains(id)), "no delivery is claimed twice: a={by_a:?} b={by_b:?}");
    assert_eq!(by_a.len() + by_b.len(), 4, "all four claimed, at most 2 of org a each: a={by_a:?} b={by_b:?}");

    // A claimant holding its share of org a's claims none of the rest.
    db.release_webhook_claims(&a, &[]).await.unwrap();
    db.release_webhook_claims(&b, &[]).await.unwrap();
    let first = of_mine(db.claim_due_webhook_deliveries(&a, 100, 2, hold).await.unwrap());
    assert_eq!(first.len(), 3, "2 of org a's and org b's: {first:?}");
    assert!(of_mine(db.claim_due_webhook_deliveries(&a, 100, 2, hold).await.unwrap()).is_empty(), "a holds its share already");
    let third = of_mine(db.claim_due_webhook_deliveries(&b, 100, 2, hold).await.unwrap());
    assert_eq!(third.len(), 1, "b takes org a's third: {third:?}");

    // Renewal is the holder's only; a lapsed lease passes on.
    assert_eq!(db.renew_webhook_claims(&c, &first, hold).await.unwrap(), 0, "c holds none of them");
    assert_eq!(db.renew_webhook_claims(&a, &first, hold).await.unwrap(), 3);
    sqlx::query("UPDATE webhook_outbox SET claimed_until = now() - interval '1 second' WHERE id = $1").bind(&third[0]).execute(db.pool()).await.unwrap();
    assert_eq!(of_mine(db.claim_due_webhook_deliveries(&c, 100, 2, hold).await.unwrap()), third, "b's lapsed lease passes to c");

    // An attempt is recorded by the lease's holder only, and reschedules.
    let retry = crate::db::OutboxAttempt {
        attempts: 1,
        outcome: crate::db::OutboxOutcome::Retry { after: chrono::Duration::seconds(30) },
        status_code: Some(503),
        error: None,
        payload_json: Some("{\"id\":\"p\"}"),
    };
    assert!(!db.finish_webhook_attempt(&third[0], &b, &retry).await.unwrap(), "b's lease lapsed: its attempt is not recorded");
    assert!(db.finish_webhook_attempt(&third[0], &c, &retry).await.unwrap());
    let rec = sqlx::query_scalar::<_, String>("SELECT generation_id FROM webhook_outbox WHERE id = $1").bind(&third[0]).fetch_one(db.pool()).await.unwrap();
    let waiting = db.webhook_outbox_of(&rec).await.unwrap().unwrap();
    assert_eq!((waiting.status.as_str(), waiting.attempts, waiting.claimed_by.as_deref(), waiting.payload_json.as_deref()), ("pending", 1, None, Some("{\"id\":\"p\"}")));
    let wait = waiting.next_attempt_at - chrono::Utc::now();
    assert!(wait > chrono::Duration::seconds(25) && wait <= chrono::Duration::seconds(31), "due in 30 s: {wait:?}");
    let delivered = crate::db::OutboxAttempt { attempts: 2, outcome: crate::db::OutboxOutcome::Delivered, status_code: Some(200), error: None, payload_json: None };
    assert!(db.finish_webhook_attempt(&first[0], &a, &delivered).await.unwrap());
    let done = sqlx::query_as::<_, (String, Option<String>, bool)>("SELECT status, claimed_by, finished_at IS NOT NULL FROM webhook_outbox WHERE id = $1")
        .bind(&first[0])
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(done, ("delivered".to_string(), None, true));

    // A stop gives the rest back; a finished delivery is never claimed again.
    assert!(db.release_webhook_claims(&a, &[]).await.unwrap() >= 2, "a held two pending ones");
    let still_held: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM webhook_outbox WHERE id = ANY($1) AND claimed_by IS NOT NULL")
        .bind(mine.clone())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(still_held, 0, "none of them is held any more");
    sqlx::query("UPDATE webhook_outbox SET next_attempt_at = now() - interval '1 second' WHERE id = ANY($1)").bind(mine.clone()).execute(db.pool()).await.unwrap();
    let again = of_mine(db.claim_due_webhook_deliveries(&b, 100, 100, hold).await.unwrap());
    assert!(!again.contains(&first[0]) && again.len() == 3, "the delivered one stays done: {again:?}");
    for claimant in [&a, &b, &c] {
        db.release_webhook_claims(claimant, &[]).await.unwrap();
    }
    sqlx::query("UPDATE webhook_outbox SET status = 'failed' WHERE id = ANY($1) AND status = 'pending'").bind(mine).execute(db.pool()).await.unwrap();
}

/// On Postgres too, a delivery whose key's webhook_url was cleared after the
/// job ended goes nowhere: its first attempt ends it failed, naming why, and
/// nothing is sent.
#[tokio::test]
async fn a_delivery_whose_webhook_was_cleared_ends_failed_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_delivery_whose_webhook_was_cleared_ends_failed_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let _turn = OUTBOX_TESTS.lock().await;
    let db = std::sync::Arc::new(PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
    let (org, app) = outbox_tenant(&db, "cleared").await;
    let key = outbox_key(&db, &org, &app, Some("https://hooks.example/cleared")).await;
    let id = format!("cleared-{}", Uuid::new_v4().simple());
    db.insert_generation(&id, Some(&key), "mock/x", "mock", "video", Some("j"), 0.0, Some(&org), Some(&app)).await.unwrap();
    db.update_generation_if_active(&id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
    sqlx::query("UPDATE api_keys SET webhook_url = NULL WHERE id = $1").bind(key.to_string()).execute(db.pool()).await.unwrap();

    let store: std::sync::Arc<dyn DatabaseStore> = db.clone();
    let outbox = std::sync::Arc::new(crate::proxy::webhook::Outbox::new(
        format!("pg-{}", Uuid::new_v4().simple()),
        std::sync::Arc::new(crate::proxy::webhook::WebhookDeliveries::new(4, 2)),
        vec![std::time::Duration::from_secs(10)],
    ));
    let ctx = crate::proxy::webhook::SendContext::new(crate::config::Mode::SingleTenant, None, crate::util::ssrf::no_redirect_client());
    assert_eq!(outbox.send_first(&store, &ctx, &id).await, crate::proxy::webhook::FirstAttempt::Sent, "claimed and tried");
    let d = db.webhook_outbox_of(&id).await.unwrap().unwrap();
    assert_eq!((d.status.as_str(), d.attempts), ("failed", 0), "{d:?}");
    assert!(d.last_error.as_deref().is_some_and(|e| e.contains("webhook_url")), "{d:?}");
}

/// Postgres twin of `migration_0032_stores_a_cleared_webhook_as_none`, on a
/// database of its own: every migration before 032, three keys, then 032.
#[tokio::test]
async fn migration_0032_stores_a_cleared_webhook_as_none_against_postgres() {
    use sqlx::postgres::PgPoolOptions;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping migration_0032_stores_a_cleared_webhook_as_none_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let server = PgPoolOptions::new().max_connections(1).connect(&url).await.expect("connect to the test Postgres");
    let name = format!("litegen_mig0032_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&server).await.expect("create a scratch database");
    let mut scratch_url = reqwest::Url::parse(&url).expect("LITEGEN_PG_TEST_URL is a URL");
    scratch_url.set_path(&format!("/{name}"));
    let pool = PgPoolOptions::new().max_connections(1).connect(scratch_url.as_str()).await.expect("connect to the scratch database");

    let mut before = sqlx::migrate!("./migrations/postgres");
    before.migrations =
        std::borrow::Cow::Owned(before.migrations.iter().filter(|m| m.version < 20240101000032).cloned().collect());
    before.run(&pool).await.expect("migrations before 032");
    for (id, webhook) in [("k-cleared", Some("")), ("k-hooked", Some("https://hooks.example/h")), ("k-none", None)] {
        sqlx::query("INSERT INTO api_keys (id, name, key_hash, key_prefix, webhook_url) VALUES ($1, $1, $2, 'lg-k', $3)")
            .bind(id)
            .bind(format!("hash-{id}"))
            .bind(webhook)
            .execute(&pool)
            .await
            .unwrap();
    }
    let applied = sqlx::migrate!("./migrations/postgres").run(&pool).await;
    let stored: Result<Vec<(String, Option<String>)>, _> = sqlx::query_as("SELECT id, webhook_url FROM api_keys ORDER BY id").fetch_all(&pool).await;
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {name}")).execute(&server).await.expect("drop the scratch database");
    applied.expect("migration 032 applies");
    let expected = [("k-cleared", None), ("k-hooked", Some("https://hooks.example/h")), ("k-none", None)];
    assert_eq!(stored.unwrap(), expected.map(|(id, url)| (id.to_string(), url.map(str::to_string))), "only the cleared one changes");
}

/// Postgres twin of `an_attempt_sent_from_memory_carries_its_jobs_org_and_app`.
#[tokio::test]
async fn an_attempt_sent_from_memory_carries_its_jobs_org_and_app_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping an_attempt_sent_from_memory_carries_its_jobs_org_and_app_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    for (tag, by_app) in [("app", true), ("org", false)] {
        let (org, app) = outbox_tenant(&db, &format!("memory-{tag}")).await;
        let key = outbox_key(&db, &org, &app, None).await;
        let id = format!("memory-{tag}-{}", Uuid::new_v4().simple());
        db.insert_generation(&id, Some(&key), "mock/x", "mock", "video", Some("j"), 0.0, Some(&org), Some(&app)).await.unwrap();
        let attempt = crate::types::WebhookDelivery {
            id: format!("wh-{}", Uuid::new_v4()),
            key_id: key.to_string(),
            generation_id: id.clone(),
            url: "https://hooks.example/h".to_string(),
            attempt_number: 1,
            status_code: Some(200),
            success: true,
            response_body: None,
            error_message: None,
            payload_json: "{}".to_string(),
            created_at: chrono::Utc::now(),
            delivery_id: Some(format!("whd-{}", Uuid::new_v4())),
        };
        db.insert_webhook_delivery(&attempt).await.unwrap();
        let tenant: (Option<String>, Option<String>) =
            sqlx::query_as("SELECT org_id, app_id FROM webhook_deliveries WHERE id = $1").bind(&attempt.id).fetch_one(db.pool()).await.unwrap();
        assert_eq!(tenant, (Some(org.clone()), Some(app.clone())), "{tag}: the attempt carries its job's org and app");

        if by_app {
            assert!(db.delete_application(&app).await.unwrap());
        } else {
            assert!(db.delete_organization(&org).await.unwrap());
        }
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM webhook_deliveries WHERE id = $1").bind(&attempt.id).fetch_one(db.pool()).await.unwrap();
        assert_eq!(left, 0, "deleting the {tag} took the attempt");
    }
}

/// Postgres twin of `the_webhook_log_purge_takes_what_ended_before_its_window_and_never_a_pending_delivery`.
/// The shared database may hold other tests' old rows, so it purges until
/// nothing is left and judges only its own.
#[tokio::test]
async fn the_webhook_log_purge_takes_what_ended_before_its_window_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping the_webhook_log_purge_takes_what_ended_before_its_window_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let _turn = OUTBOX_TESTS.lock().await;
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let (org, app) = outbox_tenant(&db, "purge").await;
    let key = outbox_key(&db, &org, &app, Some("https://hooks.example/h")).await;
    let t = Uuid::new_v4().simple().to_string();
    let deliveries = [("old-delivered", "delivered", Some(31)), ("old-failed", "failed", Some(31)), ("old-pending", "pending", None), ("recent", "delivered", Some(29))];
    for (name, status, ended) in deliveries {
        let id = format!("{name}-{t}");
        db.insert_generation(&id, Some(&key), "mock/x", "mock", "video", Some("j"), 0.0, Some(&org), Some(&app)).await.unwrap();
        db.update_generation_if_active(&id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
        sqlx::query(
            "UPDATE webhook_outbox SET status = $1, created_at = now() - interval '400 days', next_attempt_at = now() - interval '399 days', \
             finished_at = now() - make_interval(days => $2) WHERE generation_id = $3",
        )
        .bind(status)
        .bind(ended)
        .bind(&id)
        .execute(db.pool())
        .await
        .unwrap();
    }
    let mut attempts = Vec::new();
    for days in [31, 31, 29] {
        let attempt = crate::types::WebhookDelivery {
            id: format!("wh-{}", Uuid::new_v4()),
            key_id: key.to_string(),
            generation_id: format!("old-delivered-{t}"),
            url: "https://hooks.example/h".to_string(),
            attempt_number: 1,
            status_code: Some(503),
            success: false,
            response_body: None,
            error_message: None,
            payload_json: String::new(),
            created_at: chrono::Utc::now(),
            delivery_id: None,
        };
        db.insert_webhook_delivery(&attempt).await.unwrap();
        sqlx::query("UPDATE webhook_deliveries SET created_at = now() - make_interval(days => $1) WHERE id = $2").bind(days).bind(&attempt.id).execute(db.pool()).await.unwrap();
        attempts.push(attempt.id);
    }

    let first = db.purge_webhook_log(30, 1).await.unwrap();
    assert_eq!(first, crate::db::WebhookLogPurged { deliveries: 1, attempts: 1 }, "at most one of each per batch");
    while db.purge_webhook_log(30, 100).await.unwrap() != crate::db::WebhookLogPurged::default() {}
    for (name, status, _) in deliveries {
        let kept = db.webhook_outbox_of(&format!("{name}-{t}")).await.unwrap().is_some();
        assert_eq!(kept, status == "pending" || name == "recent", "{name}");
    }
    let left: Vec<String> = sqlx::query_scalar("SELECT id FROM webhook_deliveries WHERE id = ANY($1)").bind(attempts.clone()).fetch_all(db.pool()).await.unwrap();
    assert_eq!(left, [attempts[2].clone()], "the attempt within the window stays");
    // Nothing of this test's stays due.
    sqlx::query("UPDATE webhook_outbox SET status = 'failed', finished_at = now() WHERE generation_id = $1").bind(format!("old-pending-{t}")).execute(db.pool()).await.unwrap();
}

/// Postgres twin of `a_finished_delivery_is_made_due_again_once_per_cooldown`.
#[tokio::test]
async fn a_finished_delivery_is_made_due_again_once_per_cooldown_against_postgres() {
    use crate::db::Redelivery;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_finished_delivery_is_made_due_again_once_per_cooldown_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let _turn = OUTBOX_TESTS.lock().await;
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let (org, app) = outbox_tenant(&db, "redeliver").await;
    let key = outbox_key(&db, &org, &app, Some("https://hooks.example/h")).await;
    let other = outbox_key(&db, &org, &app, Some("https://hooks.example/o")).await;
    let id = format!("redeliver-{}", Uuid::new_v4().simple());
    db.insert_generation(&id, Some(&key), "mock/x", "mock", "video", Some("j"), 0.0, Some(&org), Some(&app)).await.unwrap();
    db.update_generation_if_active(&id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
    let delivery = db.webhook_outbox_of(&id).await.unwrap().unwrap().id;
    let (key, other, cooldown) = (key.to_string(), other.to_string(), chrono::Duration::seconds(60));
    assert!(matches!(db.redeliver_webhook_delivery(&key, &delivery, cooldown).await.unwrap(), Redelivery::Pending(_)), "still being sent");
    sqlx::query("UPDATE webhook_outbox SET status = 'failed', attempts = 13, last_status_code = 500, last_error = 'HTTP 500', finished_at = now() WHERE id = $1")
        .bind(&delivery)
        .execute(db.pool())
        .await
        .unwrap();

    let again = match db.redeliver_webhook_delivery(&key, &delivery, cooldown).await.unwrap() {
        Redelivery::Redelivered(again) => again,
        other => panic!("not redelivered: {other:?}"),
    };
    assert_eq!((again.status.as_str(), again.attempts, again.last_status_code, again.last_error.as_deref(), again.finished_at), ("pending", 0, None, None, None), "{again:?}");
    assert!((again.next_attempt_at - chrono::Utc::now()).num_seconds().abs() < 5, "due now: {again:?}");
    sqlx::query("UPDATE webhook_outbox SET status = 'delivered', finished_at = now() WHERE id = $1").bind(&delivery).execute(db.pool()).await.unwrap();
    match db.redeliver_webhook_delivery(&key, &delivery, cooldown).await.unwrap() {
        Redelivery::TooSoon { retry_at, .. } => {
            let wait = retry_at - chrono::Utc::now();
            assert!(wait > chrono::Duration::seconds(50) && wait <= cooldown + chrono::Duration::seconds(1), "{wait:?}");
        }
        other => panic!("not too soon: {other:?}"),
    }
    sqlx::query("UPDATE webhook_outbox SET redelivered_at = now() - interval '61 seconds' WHERE id = $1").bind(&delivery).execute(db.pool()).await.unwrap();
    assert_eq!(db.redeliver_webhook_delivery(&other, &delivery, cooldown).await.unwrap(), Redelivery::NotFound, "another key's");
    assert!(matches!(db.redeliver_webhook_delivery(&key, &delivery, cooldown).await.unwrap(), Redelivery::Redelivered(_)), "the cooldown is over");
    // Nothing of this test's stays due.
    sqlx::query("UPDATE webhook_outbox SET status = 'failed', finished_at = now() WHERE id = $1").bind(&delivery).execute(db.pool()).await.unwrap();
}

/// The dispatcher's reads on Postgres: whether any delivery is due (so an
/// idle tick claims nothing), and whether a job's webhook was answered 2xx
/// already, by an attempt with no delivery id (a replica from before the
/// outbox) or any other.
#[tokio::test]
async fn the_outbox_reads_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping the_outbox_reads_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let _turn = OUTBOX_TESTS.lock().await;
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    // The due read sees the whole shared database: none of it may be pending.
    sqlx::query("UPDATE webhook_outbox SET status = 'failed', finished_at = now() WHERE status = 'pending'").execute(db.pool()).await.unwrap();
    let (org, app) = outbox_tenant(&db, "reads").await;
    let key = outbox_key(&db, &org, &app, Some("https://hooks.example/h")).await;
    let id = format!("reads-{}", Uuid::new_v4().simple());
    db.insert_generation(&id, Some(&key), "mock/x", "mock", "video", Some("j"), 0.0, Some(&org), Some(&app)).await.unwrap();
    db.update_generation_if_active(&id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
    assert!(!db.webhook_delivery_due().await.unwrap(), "a delivery within its grace is not due");
    sqlx::query("UPDATE webhook_outbox SET next_attempt_at = now() - interval '1 second' WHERE generation_id = $1").bind(&id).execute(db.pool()).await.unwrap();
    assert!(db.webhook_delivery_due().await.unwrap(), "due once its grace is over");
    let claimant = format!("reads-{}", Uuid::new_v4().simple());
    assert_eq!(db.claim_due_webhook_deliveries(&claimant, 10, 10, chrono::Duration::seconds(60)).await.unwrap().len(), 1);
    assert!(!db.webhook_delivery_due().await.unwrap(), "a held one is not due");

    assert!(!db.webhook_answered_for(&id).await.unwrap(), "no attempt yet");
    let attempt = crate::types::WebhookDelivery {
        id: format!("wh-{}", Uuid::new_v4()),
        key_id: key.to_string(),
        generation_id: id.clone(),
        url: "https://hooks.example/h".to_string(),
        attempt_number: 1,
        status_code: Some(200),
        success: true,
        response_body: None,
        error_message: None,
        payload_json: "{}".to_string(),
        created_at: chrono::Utc::now(),
        delivery_id: None,
    };
    db.insert_webhook_delivery(&attempt).await.unwrap();
    assert!(db.webhook_answered_for(&id).await.unwrap(), "an attempt with no delivery id, answered 2xx");
    // Nothing of this test's stays due.
    sqlx::query("UPDATE webhook_outbox SET status = 'failed', finished_at = now(), claimed_by = NULL, claimed_until = NULL WHERE generation_id = $1")
        .bind(&id)
        .execute(db.pool())
        .await
        .unwrap();
}

/// Postgres twin of `a_delivery_refused_at_send_time_is_listed_and_sent_once_redelivered`
/// (proxy/webhook.rs): a Member's key whose webhook points inside the
/// deployment is refused at send time; the refusal is an attempt row of the
/// key's list, under the delivery's id; once the Member is an Admin, the
/// redelivered delivery is sent.
#[tokio::test]
async fn a_delivery_refused_at_send_time_is_listed_and_sent_once_redelivered_against_postgres() {
    use crate::types::{Role, User};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_delivery_refused_at_send_time_is_listed_and_sent_once_redelivered_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let _turn = OUTBOX_TESTS.lock().await;
    let db = std::sync::Arc::new(PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    let now = chrono::Utc::now();
    let member = User {
        id: Uuid::new_v4().to_string(),
        email: format!("{}@refused.test", Uuid::new_v4().simple()),
        password_hash: None,
        role: Role::Member,
        oauth_github_id: None,
        oauth_google_id: None,
        created_at: now,
        updated_at: now,
        last_login_at: None,
        is_active: true,
        sign_in_epoch: 0,
    };
    db.create_user(&member).await.unwrap();
    let org = crate::api::middleware::DEFAULT_ORG_ID;
    db.add_org_member(org, &member.id, Role::Member).await.unwrap();
    let hook = format!("{}/hook", server.uri());
    let t = Uuid::new_v4().simple().to_string();
    let key = db.create_api_key(&format!("refused-{t}"), &format!("hash-refused-{t}"), "lg-k", None, None, "generate,read", Some(&hook)).await.unwrap();
    db.set_api_key_owner(&key.id, &member.id).await.unwrap();
    let id = format!("refused-{t}");
    db.insert_generation(&id, Some(&key.id), "mock/x", "mock", "video", Some("j"), 0.0, None, None).await.unwrap();
    db.update_generation_if_active(&id, "completed", 100, None, None, Some(now), None).await.unwrap();
    let delivery = db.webhook_outbox_of(&id).await.unwrap().expect("recorded").id;

    let store: std::sync::Arc<dyn DatabaseStore> = db.clone();
    let outbox = std::sync::Arc::new(crate::proxy::webhook::Outbox::new(
        format!("pg-{t}"),
        std::sync::Arc::new(crate::proxy::webhook::WebhookDeliveries::new(4, 2)),
        vec![std::time::Duration::from_secs(10)],
    ));
    let ctx = crate::proxy::webhook::SendContext::new(crate::config::Mode::SingleTenant, None, crate::util::ssrf::no_redirect_client());
    assert_eq!(outbox.send_first(&store, &ctx, &id).await, crate::proxy::webhook::FirstAttempt::Sent, "claimed and tried");
    assert!(server.received_requests().await.unwrap_or_default().is_empty(), "a Member's webhook may not reach inside");
    let (rows, total) = db.list_webhook_deliveries(&key.id.to_string(), 1, 50).await.unwrap();
    assert_eq!(total, 1, "the refusal is in the key's list");
    let row = &rows[0];
    assert_eq!((row.delivery_id.as_deref(), row.status_code, row.success), (Some(delivery.as_str()), None, false), "{row:?}");
    assert!(row.error_message.as_deref().is_some_and(|e| e.contains("not allowed")), "it says why: {row:?}");

    db.update_member_role(org, &member.id, Role::Admin).await.unwrap();
    let asked = db.redeliver_webhook_delivery(&key.id.to_string(), &delivery, chrono::Duration::seconds(60)).await.unwrap();
    assert!(matches!(asked, crate::db::Redelivery::Redelivered(_)), "{asked:?}");
    assert_eq!(outbox.send_first(&store, &ctx, &id).await, crate::proxy::webhook::FirstAttempt::Sent, "its fresh series starts at once");
    let received = server.received_requests().await.unwrap_or_default();
    assert_eq!(received.len(), 1, "sent once the cause is fixed");
    let sent_as = received[0].headers.get("x-litegen-delivery-id").and_then(|v| v.to_str().ok()).unwrap_or_default();
    assert_eq!(sent_as, delivery, "under its delivery id");
    assert_eq!(db.webhook_outbox_of(&id).await.unwrap().unwrap().status, "delivered");
}

// ─── a deleted org's or app's files (migration 031) ─────────────────────────

/// A fresh org with `apps` apps, for the file-deletion tests: (org id, app ids).
/// Each test claims only its own org's or app's rows, so they run side by side.
async fn files_tenant(db: &PostgresDatabase, tag: &str, apps: usize) -> (String, Vec<String>) {
    use crate::types::{Application, Organization};
    let now = chrono::Utc::now();
    let org = Organization {
        id: Uuid::new_v4().to_string(),
        name: format!("files-{tag}"),
        slug: format!("files-{tag}-{}", Uuid::new_v4().simple()),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_organization(&org).await.unwrap();
    let mut ids = Vec::new();
    for n in 0..apps {
        let app = Application {
            id: Uuid::new_v4().to_string(),
            org_id: org.id.clone(),
            name: format!("files-{tag}-{n}"),
            slug: format!("files-{tag}-{n}-{}", Uuid::new_v4().simple()),
            status: "active".into(),
            created_at: now,
            updated_at: now,
        };
        db.create_application(&app).await.unwrap();
        ids.push(app.id);
    }
    (org.id, ids)
}

/// A finished generation of `app` with `result_url` and `meta` as its metadata.
async fn finished_generation(db: &PostgresDatabase, id: &str, org: &str, app: &str, result_url: Option<&str>, meta: Option<serde_json::Value>) {
    db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, Some(org), Some(app)).await.unwrap();
    db.update_generation_if_active(id, "completed", 100, result_url, None, Some(chrono::Utc::now()), meta.as_ref()).await.unwrap();
}

/// Deleting an org records, in the transaction that deletes its rows, the
/// files each of its apps' generations kept, under the org: an asset's URL
/// unless it is `kept: false`; a row from before litegen kept results, or
/// whose metadata isn't JSON, its result_url.
#[tokio::test]
async fn deleting_an_org_records_the_files_it_kept_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping deleting_an_org_records_the_files_it_kept_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let (org, apps) = files_tenant(&db, "org", 2).await;
    let base = "http://127.0.0.1:4000/v1/models3d/assets/litegen/3d";
    let mut want = Vec::new();
    for app in &apps {
        let id = format!("tfd-{}", Uuid::new_v4().simple());
        let kept = format!("{base}/{id}/model.glb");
        let meta = serde_json::json!({ "assets": [
            { "kind": "mesh", "url": kept },
            { "kind": "preview", "url": "https://cdn.vendor.example/p.png?sig=1", "kept": false },
        ] });
        finished_generation(&db, &id, &org, app, Some(&kept), Some(meta)).await;
        want.push(("org".to_string(), org.clone(), id, kept));
    }
    let legacy = format!("tfd-{}", Uuid::new_v4().simple());
    let legacy_url = format!("{base}/{legacy}/model.glb");
    finished_generation(&db, &legacy, &org, &apps[0], Some(&legacy_url), None).await;
    want.push(("org".to_string(), org.clone(), legacy.clone(), legacy_url));
    let broken = format!("tfd-{}", Uuid::new_v4().simple());
    let broken_url = format!("{base}/{broken}/model.glb");
    finished_generation(&db, &broken, &org, &apps[1], Some(&broken_url), None).await;
    sqlx::query("UPDATE generations SET metadata = 'not json' WHERE id = $1").bind(&broken).execute(db.pool()).await.unwrap();
    want.push(("org".to_string(), org.clone(), broken, broken_url));

    assert!(db.delete_organization(&org).await.unwrap());

    let claimed = db.claim_tenant_file_deletions(Some(("org", &org)), 100, chrono::Duration::seconds(60)).await.unwrap();
    for d in &claimed {
        db.finish_tenant_file_deletion(d.id).await.unwrap();
    }
    let mut got: Vec<_> = claimed.into_iter().map(|d| (d.scope, d.tenant_id, d.generation_id, d.url)).collect();
    got.sort();
    want.sort();
    assert_eq!(got, want);
}

/// Two claims at once never take the same row; a claim holds what it took
/// until its hold passes (a worker that died), and then another takes it,
/// with an attempt more. A retry from a claim that is no longer the latest
/// is let go; the latest's makes it due again later, with its error. A
/// finish removes it.
#[tokio::test]
async fn file_deletion_claims_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping file_deletion_claims_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let (org, apps) = files_tenant(&db, "claims", 1).await;
    let base = "http://127.0.0.1:4000/v1/models3d/assets/litegen/3d";
    for _ in 0..40 {
        let id = format!("tfd-{}", Uuid::new_v4().simple());
        let kept = format!("{base}/{id}/model.glb");
        finished_generation(&db, &id, &org, &apps[0], Some(&kept), Some(serde_json::json!({ "assets": [{ "kind": "mesh", "url": kept }] }))).await;
    }
    assert!(db.delete_application(&apps[0]).await.unwrap());
    let of = Some(("app", apps[0].as_str()));
    let hold = chrono::Duration::seconds(60);

    let (a, b) = tokio::join!(db.claim_tenant_file_deletions(of, 25, hold), db.claim_tenant_file_deletions(of, 25, hold));
    let (a, b) = (a.unwrap(), b.unwrap());
    let taken: std::collections::HashSet<i64> = a.iter().chain(&b).map(|d| d.id).collect();
    assert_eq!((a.len() + b.len(), taken.len()), (40, 40), "every row once: {} and {}", a.len(), b.len());
    assert!(db.claim_tenant_file_deletions(of, 100, hold).await.unwrap().is_empty(), "all held");

    // The holds pass: the workers that held them died.
    sqlx::query("UPDATE tenant_file_deletions SET next_attempt_at = now() - interval '1 second' WHERE scope = 'app' AND tenant_id = $1")
        .bind(&apps[0])
        .execute(db.pool())
        .await
        .unwrap();
    let again = db.claim_tenant_file_deletions(of, 100, hold).await.unwrap();
    assert_eq!(again.len(), 40);
    assert!(again.iter().all(|d| d.attempts == 2), "{again:?}");
    let d = &again[0];
    assert!(!db.retry_tenant_file_deletion(d.id, 1, hold, "late").await.unwrap(), "an earlier claim's retry is let go");
    assert!(db.retry_tenant_file_deletion(d.id, 2, chrono::Duration::seconds(600), "S3 DELETE returned HTTP 503").await.unwrap());
    let (error, later): (Option<String>, bool) =
        sqlx::query_as("SELECT last_error, next_attempt_at > now() + interval '590 seconds' FROM tenant_file_deletions WHERE id = $1")
            .bind(d.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!((error.as_deref(), later), (Some("S3 DELETE returned HTTP 503"), true));
    for other in &again[1..] {
        assert!(db.finish_tenant_file_deletion(other.id).await.unwrap());
    }
    assert!(!db.finish_tenant_file_deletion(again[1].id).await.unwrap(), "gone already");
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenant_file_deletions WHERE scope = 'app' AND tenant_id = $1")
        .bind(&apps[0])
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(left, 1, "the retried one waits");
    assert!(db.finish_tenant_file_deletion(d.id).await.unwrap());
}

/// An org with more rows than its statements can delete within the pooled
/// connections' statement timeout is deleted all the same: the delete's own
/// transaction lifts the limit, and only for itself.
#[tokio::test]
async fn a_large_org_is_deleted_under_a_short_statement_timeout_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_large_org_is_deleted_under_a_short_statement_timeout_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let (org, apps) = files_tenant(&db, "large", 1).await;
    sqlx::query(
        "INSERT INTO request_logs (id, model, provider, status, media_type, cost_usd, latency_ms, org_id, app_id) \
         SELECT 'tfd-large-' || $1 || '-' || g, 'mock/x', 'mock', 'completed', 'image', 0, 1, $1, $2 FROM generate_series(1, 200000) g",
    )
    .bind(&org)
    .bind(&apps[0])
    .execute(db.pool())
    .await
    .unwrap();
    let limits = super::postgres::StatementLimits {
        statement: std::time::Duration::from_millis(25),
        lock: std::time::Duration::from_secs(30),
    };
    let limited = PostgresDatabase::connect_with_limits(&url, limits).await.expect("connect + migrate against the test Postgres");

    let deleted = limited.delete_organization(&org).await;
    assert!(matches!(deleted, Ok(true)), "{deleted:?}");
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM request_logs WHERE org_id = $1").bind(&org).fetch_one(db.pool()).await.unwrap();
    assert_eq!(left, 0);
    // The pool's own limit still holds for every other statement.
    let slow = sqlx::query("SELECT pg_sleep(1)").execute(limited.pool()).await;
    assert_eq!(slow.as_ref().err().and_then(sqlstate).as_deref(), Some("57014"), "{slow:?}");
}

/// The files a job stored after its app was deleted are recorded for
/// deletion on Postgres too, under that app, due at once.
#[tokio::test]
async fn files_stored_after_a_delete_are_recorded_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping files_stored_after_a_delete_are_recorded_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let app = format!("app-gone-{}", Uuid::new_v4().simple());
    let generation = format!("tfd-late-{}", Uuid::new_v4().simple());
    let urls = vec![
        format!("http://127.0.0.1:4000/v1/models3d/assets/litegen/3d/{generation}/model.glb"),
        format!("http://127.0.0.1:4000/v1/models3d/assets/litegen/3d/{generation}/preview.png"),
    ];
    db.record_tenant_file_deletions("app", &app, &generation, &urls).await.unwrap();

    let claimed = db.claim_tenant_file_deletions(Some(("app", &app)), 10, chrono::Duration::seconds(60)).await.unwrap();
    for d in &claimed {
        db.finish_tenant_file_deletion(d.id).await.unwrap();
    }
    let mut got: Vec<_> = claimed.into_iter().map(|d| (d.generation_id, d.url)).collect();
    got.sort();
    let mut want: Vec<_> = urls.into_iter().map(|u| (generation.clone(), u)).collect();
    want.sort();
    assert_eq!(got, want);
}

/// On Postgres too, the spend tags of an org's or app's requests go with it.
#[tokio::test]
async fn deleting_an_app_or_an_org_takes_its_requests_tags_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping deleting_an_app_or_an_org_takes_its_requests_tags_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    for by_app in [true, false] {
        let (org, apps) = files_tenant(&db, if by_app { "tags-app" } else { "tags-org" }, 1).await;
        let id = format!("req-tagged-{}", Uuid::new_v4().simple());
        db.log_request(&id, "mock/x", "mock", "completed", "image", 1.0, 5, None, None, Some(&org), Some(&apps[0])).await.unwrap();
        let tags = crate::management::types::Attribution { tags: vec!["campaign".into(), "client".into()], ..Default::default() };
        db.management().unwrap().attribute_request_log(&id, &tags).await.unwrap();
        let count = || sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM request_log_tags WHERE request_id = $1").bind(id.clone()).fetch_one(db.pool());
        assert_eq!(count().await.unwrap(), 2, "tagged");

        if by_app {
            assert!(db.delete_application(&apps[0]).await.unwrap());
        } else {
            assert!(db.delete_organization(&org).await.unwrap());
        }
        assert_eq!(count().await.unwrap(), 0, "deleting the {} took its requests' tags", if by_app { "app" } else { "org" });
    }
}

/// A row held `FOR KEY SHARE` on a connection of its own, in a transaction
/// left open until [`HeldRow::release`]: whatever needs that row for an
/// update or a delete waits until then.
struct HeldRow {
    conn: sqlx::PgConnection,
    pid: i32,
}

impl HeldRow {
    /// Holds the row `select` (a `... FOR KEY SHARE` with `$1`) finds for `id`.
    async fn hold(url: &str, select: &str, id: &str) -> Self {
        use sqlx::Connection;
        let mut conn = sqlx::PgConnection::connect(url).await.expect("a connection to hold the row");
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut conn).await.unwrap();
        sqlx::raw_sql("BEGIN").execute(&mut conn).await.unwrap();
        sqlx::query(select).bind(id).execute(&mut conn).await.unwrap();
        Self { conn, pid }
    }

    /// Returns once another connection waits on the held row. Postgres says
    /// who waits for whom (`pg_blocking_pids`), so nothing depends on timing.
    async fn until_waited_on(&self, url: &str) {
        use sqlx::Connection;
        let mut observer = sqlx::PgConnection::connect(url).await.expect("a connection to watch the locks");
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))")
                    .bind(self.pid)
                    .fetch_one(&mut observer)
                    .await
                    .unwrap();
                if waiting > 0 {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("nothing waited on the held row");
    }

    async fn release(mut self) {
        sqlx::raw_sql("COMMIT").execute(&mut self.conn).await.unwrap();
    }
}

/// An app with one request-log row, and that row's id.
async fn an_app_with_a_request(db: &PostgresDatabase, tag: &str) -> (String, String) {
    let (org, apps) = files_tenant(db, tag, 1).await;
    let request = format!("req-{tag}-{}", Uuid::new_v4().simple());
    db.log_request(&request, "mock/x", "mock", "completed", "image", 1.0, 5, None, None, Some(&org), Some(&apps[0])).await.unwrap();
    (apps[0].clone(), request)
}

async fn tags_of(db: &PostgresDatabase, request: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM request_log_tags WHERE request_id = $1").bind(request).fetch_one(db.pool()).await.unwrap()
}

/// A tag written while its app's delete runs, after the delete took the
/// app's tags and before it reached the tag's log row, goes with that row.
#[tokio::test]
async fn a_tag_written_while_its_apps_delete_runs_goes_with_its_log_row_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_tag_written_while_its_apps_delete_runs_goes_with_its_log_row_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = std::sync::Arc::new(PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
    let (app, request) = an_app_with_a_request(&db, "tag-race-before").await;

    // The delete stops at the log row, its tags already deleted.
    let held = HeldRow::hold(&url, "SELECT 1 FROM request_logs WHERE id = $1 FOR KEY SHARE", &request).await;
    let deleting = {
        let (db, app) = (db.clone(), app.clone());
        tokio::spawn(async move { db.delete_application(&app).await })
    };
    held.until_waited_on(&url).await;
    sqlx::query("INSERT INTO request_log_tags (request_id, tag) VALUES ($1, 'raced')")
        .bind(&request)
        .execute(db.pool())
        .await
        .expect("a tag of a log row that is still there");
    held.release().await;

    assert!(deleting.await.expect("the delete's task").expect("the delete"), "the app was deleted");
    assert_eq!(tags_of(&db, &request).await, 0, "the tag went with its log row");
}

/// A tag written once its app's delete has deleted the tag's log row, while
/// the delete has yet to commit, is refused: no tag is left once it has.
#[tokio::test]
async fn a_tag_written_after_its_log_row_was_deleted_is_refused_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_tag_written_after_its_log_row_was_deleted_is_refused_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = std::sync::Arc::new(PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
    let (app, request) = an_app_with_a_request(&db, "tag-race-after").await;

    // The delete stops at the app's own row, its log rows already deleted.
    let held = HeldRow::hold(&url, "SELECT 1 FROM applications WHERE id = $1 FOR KEY SHARE", &app).await;
    let deleting = {
        let (db, app) = (db.clone(), app.clone());
        tokio::spawn(async move { db.delete_application(&app).await })
    };
    held.until_waited_on(&url).await;
    let writing = {
        let (pool, request) = (db.pool().clone(), request.clone());
        tokio::spawn(async move { sqlx::query("INSERT INTO request_log_tags (request_id, tag) VALUES ($1, 'raced')").bind(&request).execute(&pool).await })
    };
    held.release().await;

    assert!(deleting.await.expect("the delete's task").expect("the delete"), "the app was deleted");
    let written = writing.await.expect("the tag's task");
    assert_eq!(tags_of(&db, &request).await, 0, "no tag is left of a deleted log row: {written:?}");
    assert!(
        written.as_ref().err().and_then(|e| e.as_database_error()).is_some_and(|e| e.is_foreign_key_violation()),
        "the tag was refused: {written:?}"
    );
}

/// Migrations 037 and 038 on a database of its own: every migration before
/// 037, a tag of a log row and one whose log row is gone (as a delete that
/// ran before tags went with their org or app left them), then each in turn.
/// 037 adds the key without checking the tags already there (NOT VALID):
/// the orphan is still there, but a new one is refused and a log row's
/// delete takes its tags. 038 deletes the orphan, keeps the other, and
/// validates the key.
#[tokio::test]
async fn migrations_0037_and_0038_tie_tags_to_their_log_rows_against_postgres() {
    use sqlx::postgres::PgPoolOptions;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping migrations_0037_and_0038_tie_tags_to_their_log_rows_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let server = PgPoolOptions::new().max_connections(1).connect(&url).await.expect("connect to the test Postgres");
    let name = format!("litegen_mig0038_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&server).await.expect("create a scratch database");
    let mut scratch_url = reqwest::Url::parse(&url).expect("LITEGEN_PG_TEST_URL is a URL");
    scratch_url.set_path(&format!("/{name}"));
    let pool = PgPoolOptions::new().max_connections(1).connect(scratch_url.as_str()).await.expect("connect to the scratch database");

    // The migrations up to `last`, so each step is checked on its own.
    let up_to = |last: i64| {
        let mut migrator = sqlx::migrate!("./migrations/postgres");
        migrator.migrations = std::borrow::Cow::Owned(migrator.migrations.iter().filter(|m| m.version <= last).cloned().collect());
        migrator
    };
    let tags = || sqlx::query_as::<_, (String, String)>("SELECT request_id, tag FROM request_log_tags ORDER BY request_id, tag").fetch_all(&pool);
    let key = || {
        sqlx::query_as::<_, (bool, String)>(
            "SELECT convalidated, confdeltype::text FROM pg_constraint \
             WHERE conrelid = 'request_log_tags'::regclass AND confrelid = 'request_logs'::regclass AND contype = 'f'",
        )
        .fetch_optional(&pool)
    };
    up_to(20240101000036).run(&pool).await.expect("migrations before 037");
    for stmt in [
        "INSERT INTO request_logs (id, model, provider, status) VALUES ('req-kept', 'mock/x', 'mock', 'completed')",
        "INSERT INTO request_logs (id, model, provider, status) VALUES ('req-deleted', 'mock/x', 'mock', 'completed')",
        "INSERT INTO request_log_tags (request_id, tag) VALUES ('req-kept', 'kept'), ('req-deleted', 'deleted'), ('req-gone', 'orphan')",
    ] {
        sqlx::query(stmt).execute(&pool).await.unwrap();
    }

    let applied_037 = up_to(20240101000037).run(&pool).await;
    let key_after_037 = key().await;
    let new_orphan = sqlx::query("INSERT INTO request_log_tags (request_id, tag) VALUES ('req-never', 'orphan')").execute(&pool).await;
    let deleted_row = sqlx::query("DELETE FROM request_logs WHERE id = 'req-deleted'").execute(&pool).await;
    let tags_after_037 = tags().await;

    let applied_038 = up_to(20240101000038).run(&pool).await;
    let key_after_038 = key().await;
    let tags_after_038 = tags().await;

    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {name}")).execute(&server).await.expect("drop the scratch database");
    let tag = |request: &str, tag: &str| (request.to_string(), tag.to_string());
    applied_037.expect("migration 037 applies");
    assert_eq!(key_after_037.unwrap(), Some((false, "c".to_string())), "037: a key not yet validated, that deletes a tag with its log row");
    assert!(
        new_orphan.as_ref().err().and_then(|e| e.as_database_error()).is_some_and(|e| e.is_foreign_key_violation()),
        "037: a new tag of no log row is refused: {new_orphan:?}"
    );
    deleted_row.expect("a log row with a tag is deleted");
    assert_eq!(tags_after_037.unwrap(), vec![tag("req-gone", "orphan"), tag("req-kept", "kept")], "037: the old orphan is still there; the deleted row's tag went with it");
    applied_038.expect("migration 038 applies");
    assert_eq!(key_after_038.unwrap(), Some((true, "c".to_string())), "038: the key is validated");
    assert_eq!(tags_after_038.unwrap(), vec![tag("req-kept", "kept")], "038: only the orphan goes");
}

/// Retention deletes a generation's files, and with them the request log's
/// copy of its output (an image's artifact keeps up to 2 MiB of it inline,
/// a kept video's its link): the output goes (`output_kind` `deleted`, no
/// value), the prompt and params stay, and an artifact with no output (a
/// failed request's) stays as it is. A database of its own: a sweep takes
/// every due row it finds.
#[tokio::test]
async fn a_sweep_clears_the_request_logs_copies_of_the_outputs_it_deletes_against_postgres() {
    use crate::types::RequestArtifact;
    use std::sync::Arc;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping a_sweep_clears_the_request_logs_copies_of_the_outputs_it_deletes_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let (server, name, db) = own_database(&url, "litegen_sweep_outputs").await;
    let db: Arc<dyn DatabaseStore> = Arc::new(db);
    let now = chrono::Utc::now();
    let rows = [
        ("g-image", "completed", "b64", Some("iVBORw0KGgo="), "a red cube"),
        ("g-video", "completed", "url", Some("http://127.0.0.1:4000/v1/assets/litegen/video/g-video/video.mp4"), "a wave"),
        ("g-failed", "failed", "error", None, "a refused cube"),
    ];
    for (id, status, kind, value, prompt) in rows {
        db.insert_generation(id, None, "mock/image-gen", "mock", "image", None, 0.0, None, None).await.unwrap();
        db.update_generation_if_active(id, status, 100, None, None, Some(now), None).await.unwrap();
        db.insert_request_artifact(&RequestArtifact {
            request_id: id.to_string(),
            media_type: "image".into(),
            prompt: Some(prompt.into()),
            negative_prompt: None,
            params_json: Some(serde_json::json!({ "size": "1024x1024" })),
            refs_meta_json: None,
            output_kind: kind.into(),
            output_value: value.map(str::to_string),
            output_mime: Some("image/png".into()),
            output_truncated: false,
            error_message: (kind == "error").then(|| "refused".to_string()),
            created_at: now,
            org_id: None,
            app_id: None,
        })
        .await
        .unwrap();
    }
    let mut config = crate::config::AppConfig::default();
    config.storage_access.retention_days = 1;
    let cache = Arc::new(crate::proxy::cache::GenerationCache::new(&config.cache));
    let router = crate::proxy::router::ProxyRouter::new(
        Arc::new(crate::proxy::registry::ProviderRegistry::new()),
        cache,
        Arc::new(config),
        Arc::new(crate::proxy::storage::LocalStore),
    );
    let presenting = crate::access::present::Presenting { router: Arc::new(router), db: db.clone(), secrets_key: None, mode: crate::config::Mode::SingleTenant };

    let report = crate::proxy::retention::sweep_retention_once(&presenting, now + chrono::Duration::days(2), 500).await;
    let mut stored = Vec::new();
    for (id, ..) in rows {
        stored.push(db.get_request_artifact(id).await);
    }
    drop(presenting);
    drop(db);
    drop_own_database(&server, &name).await;

    assert_eq!(report.generations, 3, "{report:?}");
    let stored: Vec<RequestArtifact> = stored.into_iter().map(|a| a.unwrap().expect("the request log's row stays")).collect();
    for artifact in &stored[..2] {
        assert_eq!((artifact.output_kind.as_str(), artifact.output_value.as_deref()), ("deleted", None), "{artifact:?}");
    }
    for (artifact, (_, _, _, _, prompt)) in stored.iter().zip(rows) {
        assert_eq!(artifact.prompt.as_deref(), Some(prompt), "the prompt stays: {artifact:?}");
        assert_eq!(artifact.params_json, Some(serde_json::json!({ "size": "1024x1024" })), "and the params: {artifact:?}");
    }
    assert_eq!((stored[2].output_kind.as_str(), stored[2].error_message.as_deref()), ("error", Some("refused")), "no output to clear");
}

/// Migration 040 clears the request log's copies of outputs whose files
/// were deleted before it (retention left them; a cache hit after a delete
/// wrote one back), keeping each one's prompt and params; every other
/// artifact stays as it was. As `migration_0040_clears_the_request_logs_copies_of_deleted_files`
/// on SQLite.
#[tokio::test]
async fn migration_0040_clears_the_request_logs_copies_of_deleted_files_against_postgres() {
    use sqlx::postgres::PgPoolOptions;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping migration_0040_clears_the_request_logs_copies_of_deleted_files_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let server = PgPoolOptions::new().max_connections(1).connect(&url).await.expect("connect to the test Postgres");
    let name = format!("litegen_mig0040_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&server).await.expect("create a scratch database");
    let mut scratch_url = reqwest::Url::parse(&url).expect("LITEGEN_PG_TEST_URL is a URL");
    scratch_url.set_path(&format!("/{name}"));
    let pool = PgPoolOptions::new().max_connections(1).connect(scratch_url.as_str()).await.expect("connect to the scratch database");

    let mut before = sqlx::migrate!("./migrations/postgres");
    before.migrations =
        std::borrow::Cow::Owned(before.migrations.iter().filter(|m| m.version < 20240101000040).cloned().collect());
    before.run(&pool).await.expect("migrations before 040");
    for stmt in [
        "INSERT INTO generations (id, model, provider, media_type, status, files_deleted_at, files_deleted_reason) VALUES \
         ('g-swept', 'mock/image-gen', 'mock', 'image', 'completed', NOW(), 'retention'), \
         ('g-hit-after-delete', 'mock/image-gen', 'mock', 'image', 'completed', NOW(), 'deleted'), \
         ('g-video', 'mock/video-gen', 'mock', 'video', 'completed', NOW(), 'retention'), \
         ('g-kept', 'mock/image-gen', 'mock', 'image', 'completed', NULL, NULL), \
         ('g-failed', 'mock/image-gen', 'mock', 'image', 'failed', NOW(), 'retention')",
        "INSERT INTO request_artifacts (request_id, media_type, prompt, params_json, output_kind, output_value, output_mime, output_truncated) VALUES \
         ('g-swept', 'image', 'a red cube', '{\"size\":\"1024x1024\"}', 'b64', 'AAAA', 'image/png', TRUE), \
         ('g-hit-after-delete', 'image', 'a red cube', NULL, 'b64', 'BBBB', 'image/png', FALSE), \
         ('g-video', 'video', 'a wave', NULL, 'url', 'http://127.0.0.1:4000/v1/assets/litegen/video/g-video/video.mp4', 'video/mp4', FALSE), \
         ('g-kept', 'image', 'a kept cube', NULL, 'b64', 'CCCC', 'image/png', FALSE), \
         ('g-failed', 'image', 'a refused cube', NULL, 'error', NULL, NULL, FALSE), \
         ('litegen-img-no-row', 'image', 'a cube with no row', NULL, 'b64', 'DDDD', 'image/png', FALSE)",
    ] {
        sqlx::query(stmt).execute(&pool).await.unwrap();
    }

    let applied = sqlx::migrate!("./migrations/postgres").run(&pool).await;
    // request_id, output_kind, output_value, output_truncated, prompt, params_json
    type Row = (String, String, Option<String>, bool, Option<String>, Option<String>);
    let stored: Result<Vec<Row>, _> = sqlx::query_as(
        "SELECT request_id, output_kind, output_value, output_truncated, prompt, params_json FROM request_artifacts ORDER BY request_id",
    )
    .fetch_all(&pool)
    .await;
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {name}")).execute(&server).await.expect("drop the scratch database");
    applied.expect("migration 040 applies");
    let row = |id: &str, kind: &str, value: Option<&str>, truncated: bool, prompt: &str, params: Option<&str>| {
        (id.to_string(), kind.to_string(), value.map(str::to_string), truncated, Some(prompt.to_string()), params.map(str::to_string))
    };
    assert_eq!(
        stored.unwrap(),
        vec![
            row("g-failed", "error", None, false, "a refused cube", None),
            row("g-hit-after-delete", "deleted", None, false, "a red cube", None),
            row("g-kept", "b64", Some("CCCC"), false, "a kept cube", None),
            row("g-swept", "deleted", None, false, "a red cube", Some("{\"size\":\"1024x1024\"}")),
            row("g-video", "deleted", None, false, "a wave", None),
            row("litegen-img-no-row", "b64", Some("DDDD"), false, "a cube with no row", None),
        ],
        "only the outputs of generations whose files are deleted go"
    );
}

/// Deleting a generation removes its request from the request log, and with
/// it the artifacts of the cache hits it answered (`output_kind`
/// `generation` naming it), which carry its prompt and params. Another
/// generation's hits, and every other artifact, stay. As
/// `redacting_a_generation_deletes_the_artifacts_of_its_cache_hits` on SQLite.
#[tokio::test]
async fn redacting_a_generation_deletes_the_artifacts_of_its_cache_hits_against_postgres() {
    use crate::types::RequestArtifact;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping redacting_a_generation_deletes_the_artifacts_of_its_cache_hits_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let tag = Uuid::new_v4().simple().to_string();
    let id = |name: &str| format!("{name}-{tag}");
    db.insert_generation(&id("g-deleted"), None, "mock/image-gen", "mock", "image", None, 0.0, None, None).await.unwrap();
    let rows = [
        (id("g-deleted"), "b64", "AAAA".to_string()),
        (id("hit-1"), "generation", id("g-deleted")),
        (id("hit-2"), "generation", id("g-deleted")),
        (id("hit-of-another"), "generation", id("g-kept")),
        (id("g-kept"), "b64", "BBBB".to_string()),
        (id("g-deleted-named-in-a-url"), "url", id("g-deleted")),
    ];
    for (request_id, kind, value) in &rows {
        db.insert_request_artifact(&RequestArtifact {
            request_id: request_id.clone(),
            media_type: "image".into(),
            prompt: Some("a red cube".into()),
            negative_prompt: None,
            params_json: None,
            refs_meta_json: None,
            output_kind: (*kind).into(),
            output_value: Some(value.clone()),
            output_mime: Some("image/png".into()),
            output_truncated: false,
            error_message: None,
            created_at: chrono::Utc::now(),
            org_id: None,
            app_id: None,
        })
        .await
        .unwrap();
    }

    db.redact_generation_request(&id("g-deleted")).await.unwrap();
    let mut left = Vec::new();
    for (request_id, ..) in &rows {
        if db.get_request_artifact(request_id).await.unwrap().is_some() {
            left.push(request_id.clone());
        }
    }
    assert_eq!(left, [id("hit-of-another"), id("g-kept"), id("g-deleted-named-in-a-url")], "its own artifact and its hits' go");
}

/// What is left of requests `ids` in the request log: their rows, their
/// tags (by request) and their artifacts, each by request id.
async fn request_log_left(db: &PostgresDatabase, ids: &[String]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let of = |sql: &'static str| sqlx::query_scalar::<_, String>(sql).bind(ids.to_vec()).fetch_all(db.pool());
    (
        of("SELECT id FROM request_logs WHERE id = ANY($1) ORDER BY id").await.unwrap(),
        of("SELECT request_id FROM request_log_tags WHERE request_id = ANY($1) ORDER BY request_id, tag").await.unwrap(),
        of("SELECT request_id FROM request_artifacts WHERE request_id = ANY($1) ORDER BY request_id").await.unwrap(),
    )
}

/// Postgres twin of `the_request_log_is_purged_a_batch_at_a_time_before_its_cutoff`;
/// here a row's tags go by their foreign key (migration 037). Other tests
/// share this database, so this one judges only its own rows: a batch of 1
/// deletes one due row, whichever it took.
#[tokio::test]
async fn the_request_log_is_purged_a_batch_at_a_time_before_its_cutoff_against_postgres() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping the_request_log_is_purged_a_batch_at_a_time_before_its_cutoff_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let db = PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres");
    let t = Uuid::new_v4().simple().to_string();
    let (old_1, old_2, new) = (format!("req-old-1-{t}"), format!("req-old-2-{t}"), format!("req-new-{t}"));
    for (id, days) in [(&old_1, 31), (&old_2, 31), (&new, 29)] {
        db.log_request(id, "mock/x", "mock", "completed", "image", 1.0, 5, None, None, None, None).await.unwrap();
        let tags = crate::management::types::Attribution { tags: vec!["campaign".into(), "client".into()], ..Default::default() };
        db.management().unwrap().attribute_request_log(id, &tags).await.unwrap();
        let artifact = crate::types::RequestArtifact {
            request_id: id.clone(),
            media_type: "image".to_string(),
            prompt: Some("a red cube".to_string()),
            negative_prompt: None,
            params_json: None,
            refs_meta_json: None,
            output_kind: "url".to_string(),
            output_value: Some("https://cdn.example/a.png".to_string()),
            output_mime: Some("image/png".to_string()),
            output_truncated: false,
            error_message: None,
            created_at: chrono::Utc::now(),
            org_id: None,
            app_id: None,
        };
        db.insert_request_artifact(&artifact).await.unwrap();
        for stmt in [
            "UPDATE request_logs SET created_at = now() - make_interval(days => $1) WHERE id = $2",
            "UPDATE request_artifacts SET created_at = now() - make_interval(days => $1) WHERE request_id = $2",
        ] {
            sqlx::query(stmt).bind(days).bind(id).execute(db.pool()).await.unwrap();
        }
    }
    db.insert_generation(&old_1, None, "mock/x", "mock", "video", Some("job"), 0.0, Some(DEFAULT_ORG_ID), Some(DEFAULT_APP_ID)).await.unwrap();
    let cutoff = db.clock_now().await.unwrap() - chrono::Duration::days(30);
    let ids = [new.clone(), old_1.clone(), old_2.clone()];
    let twice = |ids: &[String]| ids.iter().flat_map(|id| [id.clone(), id.clone()]).collect::<Vec<_>>();

    assert_eq!(db.purge_request_artifacts(cutoff, 1).await.unwrap(), 1, "one artifact per batch");
    while db.purge_request_artifacts(cutoff, 100).await.unwrap() > 0 {}
    assert_eq!(request_log_left(&db, &ids).await, (ids.to_vec(), twice(&ids), vec![new.clone()]), "only the artifacts go");

    assert_eq!(db.purge_request_logs(cutoff, 1).await.unwrap(), 1, "one log row per batch");
    while db.purge_request_logs(cutoff, 100).await.unwrap() > 0 {}
    assert_eq!(request_log_left(&db, &ids).await, (vec![new.clone()], twice(std::slice::from_ref(&new)), vec![new.clone()]), "the row after the cutoff stays whole");
    assert!(db.get_generation(&old_1).await.unwrap().is_some(), "a generation row is never the purge's");
}

/// Migration 039 applies on a fresh database with every other one, and
/// leaves a valid index that deleting a generation's served artifacts
/// (`output_kind = 'generation' AND output_value = $1`) can use.
#[tokio::test]
async fn migration_0039_indexes_the_artifacts_a_generation_served_against_postgres() {
    use sqlx::postgres::PgPoolOptions;
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping migration_0039_indexes_the_artifacts_a_generation_served_against_postgres: set LITEGEN_PG_TEST_URL");
        return;
    };
    let server = PgPoolOptions::new().max_connections(1).connect(&url).await.expect("connect to the test Postgres");
    let name = format!("litegen_mig0039_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&server).await.expect("create a scratch database");
    let mut scratch_url = reqwest::Url::parse(&url).expect("LITEGEN_PG_TEST_URL is a URL");
    scratch_url.set_path(&format!("/{name}"));
    let pool = PgPoolOptions::new().max_connections(1).connect(scratch_url.as_str()).await.expect("connect to the scratch database");

    let applied = sqlx::migrate!("./migrations/postgres").run(&pool).await;
    let valid: Result<Option<bool>, _> = sqlx::query_scalar(
        "SELECT i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid WHERE c.relname = 'idx_request_artifacts_served_generation'",
    )
    .fetch_optional(&pool)
    .await;
    // An empty table is read whole by any plan; with that ruled out, the
    // plan shows whether the index serves the delete at all.
    let plan: Result<Vec<String>, sqlx::Error> = async {
        let mut tx = pool.begin().await?;
        sqlx::query("SET LOCAL enable_seqscan = off").execute(&mut *tx).await?;
        let plan = sqlx::query_scalar("EXPLAIN DELETE FROM request_artifacts WHERE output_kind = 'generation' AND output_value = 'g-1'").fetch_all(&mut *tx).await?;
        tx.rollback().await?;
        Ok(plan)
    }
    .await;
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {name}")).execute(&server).await.expect("drop the scratch database");
    applied.expect("every migration applies on a fresh database");
    assert_eq!(valid.unwrap(), Some(true), "the index is there, and valid");
    let plan = plan.unwrap().join("\n");
    assert!(plan.contains("idx_request_artifacts_served_generation"), "{plan}");
}
