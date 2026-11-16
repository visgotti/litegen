//! [`ReservationStore`] contract tests on every backend: SQLite always,
//! Postgres when `LITEGEN_PG_TEST_URL` is set (like `db::postgres_tests`).
//!
//! Ids are unique per run, so a shared Postgres database can be reused. A
//! sweep takes any lapsed record in the database, another test's too, so each
//! test checks its own rows' amounts, never a sweep's count. Each row starts
//! with a charge already on it, so a reservation given back twice shows (a
//! release never takes a row below zero).

use chrono::Utc;
use uuid::Uuid;

use super::reservations::*;
use super::types::*;
use crate::db::DatabaseStore;

/// A lease that ran out a minute ago: the record of a request whose process died.
const LAPSED: chrono::Duration = chrono::Duration::seconds(-60);

/// What every row starts with, charged before the test reserves on it.
const CHARGED: f64 = 1.0;

fn uid(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

fn store(db: &dyn DatabaseStore) -> &dyn ReservationStore {
    db.reservations().expect("this backend records its reservations")
}

/// A key with `quota`, [`CHARGED`] used.
async fn key(db: &dyn DatabaseStore, quota: Option<f64>) -> Uuid {
    let name = uid("rsv-key");
    let key = db.create_api_key(&name, &format!("{name}-hash"), "lg-rsv", quota, None, "generate", None).await.unwrap();
    db.atomic_charge_tokens(&key.id, CHARGED).await.unwrap();
    key.id
}

async fn tokens(db: &dyn DatabaseStore, key: &Uuid) -> f64 {
    db.get_api_key(key).await.unwrap().unwrap().tokens_used
}

/// An app's limits row capped at `cap`, [`CHARGED`] spent.
async fn limits_row(db: &dyn DatabaseStore, cap: Option<f64>) -> String {
    let now = Utc::now();
    let id = uid("rsv-app");
    db.management()
        .unwrap()
        .upsert_limits(&EntityLimits {
            entity_type: EntityType::App,
            entity_id: id.clone(),
            org_id: None,
            max_budget_usd: cap,
            soft_budget_usd: None,
            budget_duration: None,
            spend_usd: CHARGED,
            budget_reset_at: None,
            soft_budget_alerted_at: None,
            allowed_models: None,
            max_parallel_requests: None,
            rpm_limit: None,
            metadata: None,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    id
}

async fn spend(db: &dyn DatabaseStore, id: &str) -> f64 {
    db.management().unwrap().get_limits(EntityType::App, id).await.unwrap().unwrap().spend_usd
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Move every reservation record of `db` two minutes on: each lease lapsed a
/// minute ago, as a dead process's has by then. For the tests that take
/// their reservations through a guard, which leases them for [`LEASE`].
pub(crate) async fn lapse_every_reservation(db: &crate::db::SqliteDatabase) {
    sqlx::query("UPDATE reservations SET expires_at = strftime('%Y-%m-%d %H:%M:%f', 'now', '-60.000 seconds')")
        .execute(db.pool())
        .await
        .unwrap();
}

/// Held by each test that makes a lapsed record or sweeps: on a shared
/// Postgres database, one test's sweep would take another's lapsed record
/// before that test renews or sweeps it itself.
static SWEEPS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Sweep until nothing lapsed is left.
async fn sweep_all(db: &dyn DatabaseStore) -> u64 {
    let mut total = 0;
    loop {
        let n = store(db).sweep(50).await.unwrap();
        total += n;
        if n < 50 {
            return total;
        }
    }
}

/// A reservation is recorded in the step that takes it, and settling it
/// deletes the record and charges the actual amount: 0.5 reserved, 0.3
/// charged. Its record is gone after, so nothing is left to give back.
async fn settling_charges_the_actual_amount_and_ends_the_record(db: &dyn DatabaseStore) {
    let k = key(db, Some(10.0)).await;
    let hold = Hold::new();
    assert!(store(db).reserve_key_quota(&hold, &k, 0.5).await.unwrap());
    assert!(close(tokens(db, &k).await, CHARGED + 0.5));
    assert!(store(db).settle_key_quota(&hold.id, &k, 0.3, IfGone::ChargeActual).await.unwrap(), "the record was there to settle");
    assert!(close(tokens(db, &k).await, CHARGED + 0.3), "{}", tokens(db, &k).await);
    assert!(!store(db).release(&hold.id).await.unwrap(), "a settled reservation has no record left");
    assert!(close(tokens(db, &k).await, CHARGED + 0.3));

    let row = limits_row(db, Some(10.0)).await;
    let hold = Hold::new();
    assert!(store(db).reserve_limits(&hold, EntityType::App, &row, 0.5).await.unwrap());
    assert!(close(spend(db, &row).await, CHARGED + 0.5));
    let after = store(db).settle_limits(&hold.id, EntityType::App, &row, 0.2, IfGone::ChargeActual).await.unwrap();
    assert!(after.is_some_and(|s| close(s, CHARGED + 0.2)), "{after:?}");
    assert!(!store(db).release(&hold.id).await.unwrap());
}

/// A reservation over the cap takes nothing and records nothing.
async fn a_refused_reservation_holds_nothing(db: &dyn DatabaseStore) {
    let k = key(db, Some(CHARGED + 0.4)).await;
    let hold = Hold::new();
    assert!(!store(db).reserve_key_quota(&hold, &k, 0.5).await.unwrap(), "0.5 more is over the quota");
    assert!(close(tokens(db, &k).await, CHARGED));
    assert!(!store(db).release(&hold.id).await.unwrap(), "nothing was recorded");
    assert!(close(tokens(db, &k).await, CHARGED));

    let row = limits_row(db, Some(CHARGED + 0.4)).await;
    let hold = Hold::new();
    assert!(!store(db).reserve_limits(&hold, EntityType::App, &row, 0.5).await.unwrap());
    assert!(!store(db).release(&hold.id).await.unwrap());
    assert!(close(spend(db, &row).await, CHARGED));
}

/// What a dropped guard does: give back what it reserved, once.
async fn a_release_gives_back_once(db: &dyn DatabaseStore) {
    let k = key(db, None).await;
    let row = limits_row(db, None).await;
    let (on_key, on_row) = (Hold::new(), Hold::new());
    assert!(store(db).reserve_key_quota(&on_key, &k, 0.5).await.unwrap());
    assert!(store(db).reserve_limits(&on_row, EntityType::App, &row, 0.5).await.unwrap());
    assert!(store(db).release(&on_key.id).await.unwrap(), "the key's reservation was given back");
    assert!(store(db).release(&on_row.id).await.unwrap(), "the row's reservation was given back");
    assert!(!store(db).release(&on_key.id).await.unwrap(), "given back already");
    assert!(close(tokens(db, &k).await, CHARGED), "{}", tokens(db, &k).await);
    assert!(close(spend(db, &row).await, CHARGED), "{}", spend(db, &row).await);
}

/// The records of requests whose process died (their leases lapsed) are
/// given back by the sweep, each exactly once, though two sweepers run at
/// once.
async fn a_lapsed_reservation_is_given_back_once_by_two_sweepers(db: &dyn DatabaseStore) {
    let _one_at_a_time = SWEEPS.lock().await;
    let mut keys = Vec::new();
    let mut rows = Vec::new();
    let mut holds = Vec::new();
    for _ in 0..8 {
        let (k, row) = (key(db, None).await, limits_row(db, None).await);
        let (on_key, on_row) = (Hold::leased(LAPSED), Hold::leased(LAPSED));
        assert!(store(db).reserve_key_quota(&on_key, &k, 0.5).await.unwrap());
        assert!(store(db).reserve_limits(&on_row, EntityType::App, &row, 0.5).await.unwrap());
        keys.push(k);
        rows.push(row);
        holds.extend([on_key, on_row]);
    }
    let _ = tokio::join!(sweep_all(db), sweep_all(db));
    for k in &keys {
        assert!(close(tokens(db, k).await, CHARGED), "key {k}: {} (given back {} times)", tokens(db, k).await, (CHARGED + 0.5 - tokens(db, k).await) / 0.5);
    }
    for row in &rows {
        assert!(close(spend(db, row).await, CHARGED), "row {row}: {}", spend(db, row).await);
    }
    for hold in &holds {
        assert!(!store(db).release(&hold.id).await.unwrap(), "the sweep deleted {}'s record", hold.id);
    }
}

/// A live request renews its records: renewed, a record whose lease had
/// lapsed is held again, and the sweep leaves it. Only its own replica
/// renews it.
async fn a_renewed_reservation_survives_the_sweep(db: &dyn DatabaseStore) {
    let _one_at_a_time = SWEEPS.lock().await;
    let k = key(db, None).await;
    let hold = Hold::leased(LAPSED);
    assert!(store(db).reserve_key_quota(&hold, &k, 0.5).await.unwrap());
    assert!(store(db).renew("another-replica", std::slice::from_ref(&hold.id), LEASE).await.unwrap().is_empty(), "another replica's renewal");
    assert_eq!(store(db).renew(&hold.holder, std::slice::from_ref(&hold.id), LEASE).await.unwrap(), vec![hold.id.clone()]);
    sweep_all(db).await;
    assert!(close(tokens(db, &k).await, CHARGED + 0.5), "the sweep gave back a live request's reservation: {}", tokens(db, &k).await);
    assert!(store(db).release(&hold.id).await.unwrap(), "still recorded");
}

/// A request whose record was swept while it ran (its renewals failed) and
/// that then settles is charged its whole actual amount: the sweep gave back
/// what it held.
async fn a_request_that_settles_after_the_sweep_is_charged_whole(db: &dyn DatabaseStore) {
    let _one_at_a_time = SWEEPS.lock().await;
    let k = key(db, None).await;
    let hold = Hold::leased(LAPSED);
    assert!(store(db).reserve_key_quota(&hold, &k, 0.5).await.unwrap());
    sweep_all(db).await;
    assert!(close(tokens(db, &k).await, CHARGED));
    assert!(!store(db).settle_key_quota(&hold.id, &k, 0.3, IfGone::ChargeActual).await.unwrap(), "the sweep took the record");
    assert!(close(tokens(db, &k).await, CHARGED + 0.3), "{}", tokens(db, &k).await);

    let row = limits_row(db, None).await;
    let hold = Hold::leased(LAPSED);
    assert!(store(db).reserve_limits(&hold, EntityType::App, &row, 0.5).await.unwrap());
    sweep_all(db).await;
    let after = store(db).settle_limits(&hold.id, EntityType::App, &row, 0.3, IfGone::ChargeActual).await.unwrap();
    assert!(after.is_some_and(|s| close(s, CHARGED + 0.3)), "{after:?}");
}

/// An `Idempotency-Key` whose request died (its lease lapsed, nobody
/// renewed it) lets the retry run at once. It used to answer 409
/// `idempotency_request_in_progress` for 15 minutes.
async fn an_idempotency_key_whose_request_died_is_free_once_its_lease_lapsed(db: &dyn DatabaseStore) {
    use crate::db::IdempotencyBegin;
    let (scope, key) = (uid("scope"), uid("key"));
    assert_eq!(db.idempotency_begin_held(&scope, &key, "h", &uid("idem"), LAPSED).await.unwrap(), IdempotencyBegin::Started);
    assert_eq!(
        db.idempotency_begin_held(&scope, &key, "h", &uid("idem"), LEASE).await.unwrap(),
        IdempotencyBegin::Started,
        "the retry of a request whose lease lapsed runs"
    );
    assert_eq!(db.idempotency_begin_held(&scope, &key, "h", &uid("idem"), LEASE).await.unwrap(), IdempotencyBegin::InProgress, "the retry holds it now");
}

/// A live request renews its `Idempotency-Key`'s lease, so a duplicate still
/// finds it in progress; another request's renewal leaves it alone.
async fn a_renewed_idempotency_key_stays_in_progress(db: &dyn DatabaseStore) {
    use crate::db::IdempotencyBegin;
    let (scope, key, held_by) = (uid("scope"), uid("key"), uid("idem"));
    assert_eq!(db.idempotency_begin_held(&scope, &key, "h", &held_by, LAPSED).await.unwrap(), IdempotencyBegin::Started);
    let held = HeldKey { scope: scope.clone(), key: key.clone(), held_by: held_by.clone() };
    let other = HeldKey { held_by: uid("idem"), ..held.clone() };
    assert_eq!(store(db).renew_idempotency(&[other], LEASE).await.unwrap(), 0, "another request's renewal");
    assert_eq!(store(db).renew_idempotency(&[held], LEASE).await.unwrap(), 1);
    assert_eq!(db.idempotency_begin_held(&scope, &key, "h", &uid("idem"), LEASE).await.unwrap(), IdempotencyBegin::InProgress);
}

/// A record with no lease (an older replica's during a rolling upgrade, or
/// one from before 030) keeps the 15-minute rule: a fresh one is in
/// progress. `unleased` writes one, as such a replica does.
async fn an_idempotency_key_held_without_a_lease_keeps_the_15_minute_rule(db: &dyn DatabaseStore, unleased: impl AsyncFn(&str, &str)) {
    use crate::db::IdempotencyBegin;
    let (scope, key) = (uid("scope"), uid("key"));
    unleased(&scope, &key).await;
    assert_eq!(db.idempotency_begin_held(&scope, &key, "h", &uid("idem"), LEASE).await.unwrap(), IdempotencyBegin::InProgress);
}

#[tokio::test]
async fn sqlite_an_idempotency_key_held_without_a_lease_keeps_the_15_minute_rule() {
    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    let unleased = async |scope: &str, key: &str| {
        sqlx::query("INSERT INTO idempotency_keys (scope, idem_key, request_hash, state, created_at) VALUES (?, ?, 'h', 'in_progress', ?)")
            .bind(scope)
            .bind(key)
            .bind(Utc::now())
            .execute(db.pool())
            .await
            .unwrap();
    };
    an_idempotency_key_held_without_a_lease_keeps_the_15_minute_rule(&db, unleased).await;
}

#[tokio::test]
async fn postgres_an_idempotency_key_held_without_a_lease_keeps_the_15_minute_rule() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping postgres_an_idempotency_key_held_without_a_lease_keeps_the_15_minute_rule: LITEGEN_PG_TEST_URL not set");
        return;
    };
    let db = crate::db::PostgresDatabase::connect(&url).await.expect("connect to LITEGEN_PG_TEST_URL");
    let unleased = async |scope: &str, key: &str| {
        sqlx::query("INSERT INTO idempotency_keys (scope, idem_key, request_hash, state, created_at) VALUES ($1, $2, 'h', 'in_progress', now())")
            .bind(scope)
            .bind(key)
            .execute(db.pool())
            .await
            .unwrap();
    };
    an_idempotency_key_held_without_a_lease_keeps_the_15_minute_rule(&db, unleased).await;
}

/// Review round 3, finding 2: once a request's lease lapsed and its retry
/// took the `Idempotency-Key` over, the first request's finish leaves the
/// retry's record alone. Its response used to overwrite the retry's record,
/// or its release delete it, letting a third request run the same billed job.
async fn a_finish_after_its_lease_lapsed_leaves_the_retrys_record(db: &dyn DatabaseStore) {
    use crate::db::{IdempotencyBegin, StoredResponse};
    let stored = StoredResponse { status: 200, headers: vec![], body: b"{}".to_vec() };
    // Completed late: the retry's record stays in progress.
    let (scope, key, first) = (uid("scope"), uid("key"), uid("idem"));
    assert_eq!(db.idempotency_begin_held(&scope, &key, "h", &first, LAPSED).await.unwrap(), IdempotencyBegin::Started);
    assert_eq!(db.idempotency_begin_held(&scope, &key, "h", &uid("idem"), LEASE).await.unwrap(), IdempotencyBegin::Started, "the retry takes the key over");
    db.idempotency_complete_held(&scope, &key, &first, &stored).await.unwrap();
    assert_eq!(
        db.idempotency_begin_held(&scope, &key, "h", &uid("idem"), LEASE).await.unwrap(),
        IdempotencyBegin::InProgress,
        "the first request's late response was stored over the retry's record"
    );
    // Released late: the retry's record stays.
    let (scope, key, first) = (uid("scope"), uid("key"), uid("idem"));
    assert_eq!(db.idempotency_begin_held(&scope, &key, "h", &first, LAPSED).await.unwrap(), IdempotencyBegin::Started);
    assert_eq!(db.idempotency_begin_held(&scope, &key, "h", &uid("idem"), LEASE).await.unwrap(), IdempotencyBegin::Started);
    db.idempotency_release_held(&scope, &key, &first).await.unwrap();
    assert_eq!(
        db.idempotency_begin_held(&scope, &key, "h", &uid("idem"), LEASE).await.unwrap(),
        IdempotencyBegin::InProgress,
        "the first request's late release deleted the retry's record"
    );
    // Its own record it completes as before.
    let (scope, key, held_by) = (uid("scope"), uid("key"), uid("idem"));
    assert_eq!(db.idempotency_begin_held(&scope, &key, "h", &held_by, LEASE).await.unwrap(), IdempotencyBegin::Started);
    db.idempotency_complete_held(&scope, &key, &held_by, &stored).await.unwrap();
    assert_eq!(db.idempotency_begin_held(&scope, &key, "h", &uid("idem"), LEASE).await.unwrap(), IdempotencyBegin::Replay(stored));
}

/// Review round 3, finding 1: a settlement tried again after an attempt
/// whose answer was lost finds the record gone when that attempt landed, and
/// changes nothing. A first attempt still charges the whole actual for a
/// record the sweep took (`a_request_that_settles_after_the_sweep_is_charged_whole`).
async fn a_retried_settle_changes_nothing_when_its_record_is_gone(db: &dyn DatabaseStore) {
    let k = key(db, None).await;
    let hold = Hold::new();
    assert!(store(db).reserve_key_quota(&hold, &k, 0.5).await.unwrap());
    assert!(store(db).settle_key_quota(&hold.id, &k, 0.3, IfGone::ChargeActual).await.unwrap());
    assert!(!store(db).settle_key_quota(&hold.id, &k, 0.3, IfGone::DoNothing).await.unwrap(), "settled already");
    assert!(close(tokens(db, &k).await, CHARGED + 0.3), "a retry charged a settled reservation again: {}", tokens(db, &k).await);

    let row = limits_row(db, None).await;
    let hold = Hold::new();
    assert!(store(db).reserve_limits(&hold, EntityType::App, &row, 0.5).await.unwrap());
    assert!(store(db).settle_limits(&hold.id, EntityType::App, &row, 0.3, IfGone::ChargeActual).await.unwrap().is_some());
    store(db).settle_limits(&hold.id, EntityType::App, &row, 0.3, IfGone::DoNothing).await.unwrap();
    assert!(close(spend(db, &row).await, CHARGED + 0.3), "a retry charged a settled reservation again: {}", spend(db, &row).await);
}

/// A request that runs past 15 minutes (a long vendor timeout) keeps its
/// `Idempotency-Key` while it renews the lease: a leased record is judged by
/// its lease, not by the 15-minute rule of a record with none. `made_then`
/// stamps the record 20 minutes ago; `now` renews it and is asked again.
async fn a_renewed_idempotency_key_outlasts_the_15_minutes(made_then: &dyn DatabaseStore, now: &dyn DatabaseStore) {
    use crate::db::IdempotencyBegin;
    let (scope, key, held_by) = (uid("scope"), uid("key"), uid("idem"));
    assert_eq!(made_then.idempotency_begin_held(&scope, &key, "h", &held_by, LEASE).await.unwrap(), IdempotencyBegin::Started);
    let held = HeldKey { scope: scope.clone(), key: key.clone(), held_by };
    assert_eq!(store(now).renew_idempotency(&[held], LEASE).await.unwrap(), 1);
    assert_eq!(
        now.idempotency_begin_held(&scope, &key, "h", &uid("idem"), LEASE).await.unwrap(),
        IdempotencyBegin::InProgress,
        "a request still renewing its key keeps it past 15 minutes"
    );
}

#[tokio::test]
async fn sqlite_a_renewed_idempotency_key_outlasts_the_15_minutes() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}?mode=rwc", dir.path().join("keys.db").display());
    let made_then = crate::db::SqliteDatabase::connect(&url).await.unwrap().with_clock_offset(chrono::Duration::minutes(-20));
    let now = crate::db::SqliteDatabase::connect(&url).await.unwrap();
    a_renewed_idempotency_key_outlasts_the_15_minutes(&made_then, &now).await;
}

#[tokio::test]
async fn postgres_a_renewed_idempotency_key_outlasts_the_15_minutes() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping postgres_a_renewed_idempotency_key_outlasts_the_15_minutes: LITEGEN_PG_TEST_URL not set");
        return;
    };
    let made_then = crate::db::PostgresDatabase::connect(&url).await.unwrap().with_clock_offset(chrono::Duration::minutes(-20));
    let now = crate::db::PostgresDatabase::connect(&url).await.unwrap();
    a_renewed_idempotency_key_outlasts_the_15_minutes(&made_then, &now).await;
}

/// A live request's reservation is renewed by its replica and kept through
/// the sweep; one whose hold is no longer live (its process died, or its
/// release failed) is not renewed, and the sweep gives it back. SQLite: what
/// is tested is this process's renewal, not the store.
#[tokio::test]
async fn a_live_requests_reservation_is_renewed_and_a_dead_ones_given_back() {
    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    let (live_key, dead_key) = (key(&db, None).await, key(&db, None).await);
    let Reserved::Held(Some(live)) = reserve_on_key(&db, &live_key, 0.5).await.unwrap() else {
        panic!("the live request's reservation has no record")
    };
    let Reserved::Held(Some(dead)) = reserve_on_key(&db, &dead_key, 0.5).await.unwrap() else {
        panic!("the dead request's reservation has no record")
    };
    // Its request is gone: nothing renews it any more.
    drop(dead);
    lapse_every_reservation(&db).await;
    renew_live(&db).await.unwrap();
    sweep_lapsed(&db).await.unwrap();
    assert!(close(tokens(&db, &live_key).await, CHARGED + 0.5), "the sweep gave back a live request's reservation: {}", tokens(&db, &live_key).await);
    assert!(close(tokens(&db, &dead_key).await, CHARGED), "the dead request's reservation: {}", tokens(&db, &dead_key).await);
    let db: std::sync::Arc<dyn DatabaseStore> = std::sync::Arc::new(db);
    settle_on_key(&db, Some(live), &live_key, 0.5, 0.3).await;
    let tokens_after = tokens(&*db, &live_key).await;
    assert!(close(tokens_after, CHARGED + 0.3), "{tokens_after}");
}

/// A store whose steps take effect but whose answers are lost, as when the
/// connection drops before the reply: each reservation lands, then answers
/// an error.
struct AnswersLost<'a>(&'a dyn ReservationStore);

#[async_trait::async_trait]
impl ReservationStore for AnswersLost<'_> {
    async fn reserve_key_quota(&self, hold: &Hold, key_id: &Uuid, amount: f64) -> Result<bool, sqlx::Error> {
        self.0.reserve_key_quota(hold, key_id, amount).await?;
        Err(sqlx::Error::PoolTimedOut)
    }
    async fn reserve_limits(&self, hold: &Hold, entity_type: EntityType, entity_id: &str, amount: f64) -> Result<bool, sqlx::Error> {
        self.0.reserve_limits(hold, entity_type, entity_id, amount).await?;
        Err(sqlx::Error::PoolTimedOut)
    }
    async fn settle_key_quota(&self, id: &str, key_id: &Uuid, actual: f64, if_gone: IfGone) -> Result<bool, sqlx::Error> {
        self.0.settle_key_quota(id, key_id, actual, if_gone).await
    }
    async fn settle_limits(&self, id: &str, entity_type: EntityType, entity_id: &str, actual: f64, if_gone: IfGone) -> Result<Option<f64>, sqlx::Error> {
        self.0.settle_limits(id, entity_type, entity_id, actual, if_gone).await
    }
    async fn release(&self, id: &str) -> Result<bool, sqlx::Error> {
        self.0.release(id).await
    }
    async fn renew(&self, holder: &str, ids: &[String], lease: chrono::Duration) -> Result<Vec<String>, sqlx::Error> {
        self.0.renew(holder, ids, lease).await
    }
    async fn renew_idempotency(&self, keys: &[HeldKey], lease: chrono::Duration) -> Result<u64, sqlx::Error> {
        self.0.renew_idempotency(keys, lease).await
    }
    async fn sweep(&self, limit: u32) -> Result<u64, sqlx::Error> {
        self.0.sweep(limit).await
    }
    async fn takes_reservations(&self) -> Result<bool, sqlx::Error> {
        self.0.takes_reservations().await
    }
}

/// Item 2's lost reply (enforce.rs, `begin`'s reserve loop): a reservation
/// that landed but whose answer was lost is given back at once, by its
/// record. It used to stay held for good: the request never counted it.
#[tokio::test]
async fn a_reservation_whose_answer_was_lost_is_given_back() {
    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    let lost = AnswersLost(store(&db));
    let k = key(&db, None).await;
    assert!(reserve_on_key_in(Some(&lost), &db, &k, 0.5).await.is_err(), "the answer was lost");
    assert!(close(tokens(&db, &k).await, CHARGED), "a quota reservation whose answer was lost: {} used", tokens(&db, &k).await);
    let row = limits_row(&db, None).await;
    assert!(reserve_on_limits_in(Some(&lost), db.management().unwrap(), EntityType::App, &row, 0.5).await.is_err());
    assert!(close(spend(&db, &row).await, CHARGED), "a limits reservation whose answer was lost: {} spent", spend(&db, &row).await);
}

/// Every replica's upkeep sweeps as it starts: a reservation whose lease
/// lapsed (its replica died before this one started) is given back by the
/// first round.
#[tokio::test]
async fn the_upkeep_gives_back_a_lapsed_reservation_as_it_starts() {
    let db = std::sync::Arc::new(crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let k = key(&*db, None).await;
    assert!(store(&*db).reserve_key_quota(&Hold::leased(LAPSED), &k, 0.5).await.unwrap());
    let upkeep = spawn_upkeep(db.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !close(tokens(&*db, &k).await, CHARGED) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    upkeep.abort();
    assert!(close(tokens(&*db, &k).await, CHARGED), "the upkeep's first round left {} used", tokens(&*db, &k).await);
}

/// A SQLite database restored without migration 030's trigger records each
/// reservation without taking it: litegen tells, so it can say so as it
/// starts. Postgres takes reservations without one.
#[tokio::test]
async fn a_sqlite_database_without_the_taking_trigger_says_so() {
    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    assert!(store(&db).takes_reservations().await.unwrap(), "a database with every migration takes its reservations");
    sqlx::query("DROP TRIGGER reservations_take").execute(db.pool()).await.unwrap();
    assert!(!store(&db).takes_reservations().await.unwrap(), "restored without the trigger, it records reservations without taking them");
}

/// Review round 3, finding 5: a Postgres sweep takes the rows it gives back
/// to one at a time in a fixed order (keys by id, then limits rows), so two
/// sweeps can't each hold a row the other waits for. Here another
/// transaction holds the lower key; while the sweep waits for it, the higher
/// key must still be free. Two connections, no timing: the test waits until
/// Postgres shows the sweep waiting on a lock.
#[tokio::test]
async fn postgres_a_sweep_takes_its_rows_in_order() {
    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
        eprintln!("skipping postgres_a_sweep_takes_its_rows_in_order: LITEGEN_PG_TEST_URL not set");
        return;
    };
    let _one_at_a_time = SWEEPS.lock().await;
    let db = std::sync::Arc::new(crate::db::PostgresDatabase::connect(&url).await.expect("connect to LITEGEN_PG_TEST_URL"));
    let (a, b) = (key(&*db, None).await, key(&*db, None).await);
    let (low, high) = if a.to_string() < b.to_string() { (a, b) } else { (b, a) };
    for k in [low, high] {
        assert!(store(&*db).reserve_key_quota(&Hold::leased(LAPSED), &k, 0.5).await.unwrap());
    }
    let mut holder = db.pool().begin().await.unwrap();
    sqlx::query("SELECT 1 FROM api_keys WHERE id = $1 FOR UPDATE").bind(low.to_string()).execute(&mut *holder).await.unwrap();

    let sweeping = {
        let db = db.clone();
        tokio::spawn(async move { store(&*db).sweep(50).await })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' \
             AND query ILIKE '%api_keys%' AND pid <> pg_backend_pid()",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        if waiting > 0 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the sweep never waited for the lower key");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let mut other = db.pool().begin().await.unwrap();
    let higher_free = sqlx::query("SELECT 1 FROM api_keys WHERE id = $1 FOR UPDATE NOWAIT").bind(high.to_string()).execute(&mut *other).await;
    other.rollback().await.unwrap();
    holder.rollback().await.unwrap();
    sweeping.await.unwrap().unwrap();
    assert!(higher_free.is_ok(), "the sweep held the higher key while it waited for the lower one: two sweeps could deadlock ({higher_free:?})");
    for k in [low, high] {
        assert!(close(tokens(&*db, &k).await, CHARGED), "key {k}: {}", tokens(&*db, &k).await);
    }
}

/// Settlement retries that come at once, for the tests.
const QUICK: [std::time::Duration; 3] = [std::time::Duration::from_millis(1); 3];

/// How a test store's first settlement goes; the later ones go through.
#[derive(Clone, Copy)]
enum FirstSettle {
    /// It fails before anything lands.
    Fails,
    /// It lands, and its answer is lost.
    LandsButItsAnswerIsLost,
}

/// Whether a closure's first call has come, shared by its calls.
fn first_call() -> impl FnMut() -> bool + Send {
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    move || !called.swap(true, std::sync::atomic::Ordering::SeqCst)
}

fn settle_key_on(db: std::sync::Arc<crate::db::SqliteDatabase>, id: String, key: Uuid, actual: f64, first: FirstSettle) -> SettleAttempt<bool> {
    let mut is_first = first_call();
    Box::new(move |if_gone| {
        let (db, id, first_time) = (db.clone(), id.clone(), is_first());
        Box::pin(async move {
            match (first_time, first) {
                (true, FirstSettle::Fails) => Err(sqlx::Error::PoolTimedOut),
                (true, FirstSettle::LandsButItsAnswerIsLost) => {
                    db.settle_key_quota(&id, &key, actual, if_gone).await?;
                    Err(sqlx::Error::PoolTimedOut)
                }
                _ => db.settle_key_quota(&id, &key, actual, if_gone).await,
            }
        })
    })
}

fn settle_row_on(db: std::sync::Arc<crate::db::SqliteDatabase>, id: String, row: String, actual: f64, first: FirstSettle) -> SettleAttempt<Option<f64>> {
    let mut is_first = first_call();
    Box::new(move |if_gone| {
        let (db, id, row, first_time) = (db.clone(), id.clone(), row.clone(), is_first());
        Box::pin(async move {
            match (first_time, first) {
                (true, FirstSettle::Fails) => Err(sqlx::Error::PoolTimedOut),
                (true, FirstSettle::LandsButItsAnswerIsLost) => {
                    db.settle_limits(&id, EntityType::App, &row, actual, if_gone).await?;
                    Err(sqlx::Error::PoolTimedOut)
                }
                _ => db.settle_limits(&id, EntityType::App, &row, actual, if_gone).await,
            }
        })
    })
}

/// A key's settlement that fails `failures` times before anything lands,
/// then goes through.
fn settle_key_failing_first(db: std::sync::Arc<crate::db::SqliteDatabase>, id: String, key: Uuid, actual: f64, failures: usize) -> SettleAttempt<bool> {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    Box::new(move |if_gone| {
        let (db, id, call) = (db.clone(), id.clone(), calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
        Box::pin(async move {
            if call < failures {
                return Err(sqlx::Error::PoolTimedOut);
            }
            db.settle_key_quota(&id, &key, actual, if_gone).await
        })
    })
}

/// Review round 3b, finding 1: a settlement keeps being tried, its
/// reservation held, until it lands, however long the database keeps
/// failing fast (a failover onto a read-only node, a database starting up).
/// It used to give up after its schedule (about a minute) and let the sweep
/// give the reservation back: the request then cost nothing.
#[tokio::test]
async fn a_settle_that_keeps_failing_past_its_schedule_still_charges_once() {
    let db = std::sync::Arc::new(crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let k = key(&*db, None).await;
    let Reserved::Held(Some(live)) = reserve_on_key(&*db, &k, 0.5).await.unwrap() else { panic!("no record") };
    // The first attempt and the 3 retries of QUICK fail, and 6 more after them.
    let attempt = settle_key_failing_first(db.clone(), live.id().to_string(), k, 0.3, 1 + QUICK.len() + 6);
    let (_, retries) = settle_recorded(live, format!("key {k}"), 0.3, &QUICK, attempt, |settled| *settled).await;
    if let Some(retries) = retries {
        retries.await.unwrap();
    }
    assert!(
        close(tokens(&*db, &k).await, CHARGED + 0.3),
        "a settlement that failed past its schedule: {} used (1.5: given up, its reservation left for the sweep)",
        tokens(&*db, &k).await
    );
}

/// A store whose reserve notes when its statement began, and takes it.
#[derive(Default)]
struct NotesItsStart(std::sync::Mutex<Option<std::time::Instant>>);

#[async_trait::async_trait]
impl ReservationStore for NotesItsStart {
    async fn reserve_key_quota(&self, _hold: &Hold, _key_id: &Uuid, _amount: f64) -> Result<bool, sqlx::Error> {
        *self.0.lock().unwrap() = Some(std::time::Instant::now());
        Ok(true)
    }
    async fn reserve_limits(&self, _hold: &Hold, _entity_type: EntityType, _entity_id: &str, _amount: f64) -> Result<bool, sqlx::Error> {
        *self.0.lock().unwrap() = Some(std::time::Instant::now());
        Ok(true)
    }
    async fn settle_key_quota(&self, _id: &str, _key_id: &Uuid, _actual: f64, _if_gone: IfGone) -> Result<bool, sqlx::Error> {
        Err(sqlx::Error::Protocol("not used here".into()))
    }
    async fn settle_limits(&self, _id: &str, _entity_type: EntityType, _entity_id: &str, _actual: f64, _if_gone: IfGone) -> Result<Option<f64>, sqlx::Error> {
        Err(sqlx::Error::Protocol("not used here".into()))
    }
    async fn release(&self, _id: &str) -> Result<bool, sqlx::Error> {
        Ok(false)
    }
    async fn renew(&self, _holder: &str, _ids: &[String], _lease: chrono::Duration) -> Result<Vec<String>, sqlx::Error> {
        Ok(Vec::new())
    }
    async fn renew_idempotency(&self, _keys: &[HeldKey], _lease: chrono::Duration) -> Result<u64, sqlx::Error> {
        Ok(0)
    }
    async fn sweep(&self, _limit: u32) -> Result<u64, sqlx::Error> {
        Ok(0)
    }
    async fn takes_reservations(&self) -> Result<bool, sqlx::Error> {
        Ok(true)
    }
}

/// Review round 3b, finding 2: a reservation's lease is reckoned from before
/// its statement was sent. Postgres starts the lease (`expires_at`) at the
/// statement's start, which a lock wait can put long before its answer;
/// reckoned from the answer, `held_until` could outlast the real lease, and a
/// settlement retry would take a swept record for a settled one.
#[tokio::test]
async fn a_reservations_lease_is_reckoned_from_before_its_statement() {
    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
    let notes = NotesItsStart::default();
    let Reserved::Held(Some(live)) = reserve_on_key_in(Some(&notes), &db, &Uuid::new_v4(), 0.5).await.unwrap() else {
        panic!("not held")
    };
    let began = notes.0.lock().unwrap().expect("the statement ran");
    let reckoned_from = live.held_until().expect("held") - (LEASE.to_std().unwrap() - LEASE_MARGIN);
    assert!(reckoned_from <= began, "the key's lease was reckoned from {:?} after its statement began", reckoned_from - began);

    let notes = NotesItsStart::default();
    let Reserved::Held(Some(live)) = reserve_on_limits_in(Some(&notes), db.management().unwrap(), EntityType::App, "app", 0.5).await.unwrap() else {
        panic!("not held")
    };
    let began = notes.0.lock().unwrap().expect("the statement ran");
    let reckoned_from = live.held_until().expect("held") - (LEASE.to_std().unwrap() - LEASE_MARGIN);
    assert!(reckoned_from <= began, "the limits row's lease was reckoned from {:?} after its statement began", reckoned_from - began);
}

/// Review round 3, finding 1: a settlement that fails once is tried again,
/// its reservation held meanwhile, and charges the actual amount once. It
/// used to be dropped: the sweep then gave the reservation back, and the
/// request cost nothing.
#[tokio::test]
async fn a_settle_that_fails_once_is_tried_again_and_charges_once() {
    let db = std::sync::Arc::new(crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let k = key(&*db, None).await;
    let Reserved::Held(Some(live)) = reserve_on_key(&*db, &k, 0.5).await.unwrap() else { panic!("no record") };
    let attempt = settle_key_on(db.clone(), live.id().to_string(), k, 0.3, FirstSettle::Fails);
    let (_, retries) = settle_recorded(live, format!("key {k}"), 0.3, &QUICK, attempt, |settled| *settled).await;
    if let Some(retries) = retries {
        retries.await.unwrap();
    }
    assert!(close(tokens(&*db, &k).await, CHARGED + 0.3), "a settlement that failed once: {} used (1.5: never settled)", tokens(&*db, &k).await);

    let row = limits_row(&*db, None).await;
    let Reserved::Held(Some(live)) = reserve_on_limits(&*db, db.management().unwrap(), EntityType::App, &row, 0.5).await.unwrap() else {
        panic!("no record")
    };
    let attempt = settle_row_on(db.clone(), live.id().to_string(), row.clone(), 0.3, FirstSettle::Fails);
    let (_, retries) = settle_recorded(live, format!("app {row}"), 0.3, &QUICK, attempt, Option::is_some).await;
    if let Some(retries) = retries {
        retries.await.unwrap();
    }
    assert!(close(spend(&*db, &row).await, CHARGED + 0.3), "a settlement that failed once: {} spent (1.5: never settled)", spend(&*db, &row).await);
}

/// Review round 3, finding 1: a settlement that landed but whose answer was
/// lost is tried again; the retry finds its record gone while its lease
/// held, so the earlier attempt landed, and charges nothing more.
#[tokio::test]
async fn a_settle_whose_answer_was_lost_charges_once() {
    let db = std::sync::Arc::new(crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let k = key(&*db, None).await;
    let Reserved::Held(Some(live)) = reserve_on_key(&*db, &k, 0.5).await.unwrap() else { panic!("no record") };
    let attempt = settle_key_on(db.clone(), live.id().to_string(), k, 0.3, FirstSettle::LandsButItsAnswerIsLost);
    let (_, retries) = settle_recorded(live, format!("key {k}"), 0.3, &QUICK, attempt, |settled| *settled).await;
    if let Some(retries) = retries {
        retries.await.unwrap();
    }
    assert!(close(tokens(&*db, &k).await, CHARGED + 0.3), "a settlement whose answer was lost: {} used (1.6: charged twice)", tokens(&*db, &k).await);

    let row = limits_row(&*db, None).await;
    let Reserved::Held(Some(live)) = reserve_on_limits(&*db, db.management().unwrap(), EntityType::App, &row, 0.5).await.unwrap() else {
        panic!("no record")
    };
    let attempt = settle_row_on(db.clone(), live.id().to_string(), row.clone(), 0.3, FirstSettle::LandsButItsAnswerIsLost);
    let (_, retries) = settle_recorded(live, format!("app {row}"), 0.3, &QUICK, attempt, Option::is_some).await;
    if let Some(retries) = retries {
        retries.await.unwrap();
    }
    assert!(close(spend(&*db, &row).await, CHARGED + 0.3), "a settlement whose answer was lost: {} spent (1.6: charged twice)", spend(&*db, &row).await);
}

macro_rules! on_every_backend {
    ($($name:ident),* $(,)?) => {
        mod sqlite {
            $(
                #[tokio::test]
                async fn $name() {
                    let db = crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
                    super::$name(&db).await;
                }
            )*
        }
        mod postgres {
            $(
                #[tokio::test]
                async fn $name() {
                    let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
                        eprintln!("skipping {}: LITEGEN_PG_TEST_URL not set", stringify!($name));
                        return;
                    };
                    let db = crate::db::PostgresDatabase::connect(&url).await.expect("connect to LITEGEN_PG_TEST_URL");
                    super::$name(&db).await;
                }
            )*
        }
    };
}

on_every_backend!(
    settling_charges_the_actual_amount_and_ends_the_record,
    a_refused_reservation_holds_nothing,
    a_release_gives_back_once,
    a_lapsed_reservation_is_given_back_once_by_two_sweepers,
    a_renewed_reservation_survives_the_sweep,
    a_request_that_settles_after_the_sweep_is_charged_whole,
    an_idempotency_key_whose_request_died_is_free_once_its_lease_lapsed,
    a_renewed_idempotency_key_stays_in_progress,
    a_finish_after_its_lease_lapsed_leaves_the_retrys_record,
    a_retried_settle_changes_nothing_when_its_record_is_gone,
);
