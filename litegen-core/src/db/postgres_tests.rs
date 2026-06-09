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
        .get_stats_for_tenant(DEFAULT_ORG_ID, None)
        .await
        .expect("get_stats_for_tenant (org) must decode AVG(latency_ms)");
    assert!(by_org.total_requests >= 1);
    assert!(by_org.avg_latency_ms > 0.0);

    let by_app = db
        .get_stats_for_tenant(DEFAULT_ORG_ID, Some(DEFAULT_APP_ID))
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
