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

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let (db, scope) = (db.clone(), scope.clone());
        tasks.push(tokio::spawn(async move { db.idempotency_begin(&scope, "k", "h").await.unwrap() }));
    }
    let mut started = 0;
    for t in tasks {
        if matches!(t.await.unwrap(), IdempotencyBegin::Started) {
            started += 1;
        }
    }
    assert_eq!(started, 1);

    let stored = StoredResponse { status: 504, headers: vec![("content-type".into(), "application/json".into())], body: b"{\"x\":1}".to_vec() };
    db.idempotency_complete(&scope, "k", &stored).await.unwrap();
    assert_eq!(db.idempotency_begin(&scope, "k", "h").await.unwrap(), IdempotencyBegin::Replay(stored));
    assert_eq!(db.idempotency_begin(&scope, "k", "other").await.unwrap(), IdempotencyBegin::Conflict);

    assert_eq!(db.idempotency_begin(&scope, "fresh", "h").await.unwrap(), IdempotencyBegin::Started);
    db.idempotency_release(&scope, "fresh").await.unwrap();
    assert_eq!(db.idempotency_begin(&scope, "fresh", "h2").await.unwrap(), IdempotencyBegin::Started);
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
    let begun = db.idempotency_begin(&scope, "k", "h").await.unwrap();

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
