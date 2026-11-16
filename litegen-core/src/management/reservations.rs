//! Reservations held by billed requests in flight, recorded so that none is
//! held for good (migration 030; docs/scaling.md, "Reservations").
//!
//! A billed request reserves its price before it calls the vendor: on its
//! key's quota (`api_keys.tokens_used`) and on each limits row it counts
//! against (`entity_limits.spend_usd`). Each reservation is recorded in the
//! same step that takes it, and its record deleted in the same step that
//! settles it or gives it back, so a record exists exactly while its amount
//! is held. While the request runs, its replica renews its records' lease.
//! A record whose lease lapsed belongs to a request that can no longer
//! finish, and every replica's sweep gives its amount back.

use async_trait::async_trait;
use uuid::Uuid;

use super::types::EntityType;

/// How long a reservation's record, or an `Idempotency-Key` in progress, stays
/// held without its request renewing it.
pub const LEASE: chrono::Duration = chrono::Duration::minutes(2);

/// What [`Live::held_until`] takes off a lease: room for a renewal's or a
/// reservation's statement to land after its time was read.
pub(crate) const LEASE_MARGIN: std::time::Duration = std::time::Duration::from_secs(15);

/// Where a reservation is recorded, and given back from (migration 030). The
/// SQLite and Postgres stores keep one; test doubles don't, and their
/// requests reserve as before 030, unrecorded.
#[async_trait]
pub trait ReservationStore: Send + Sync {
    /// Add `amount` to key `key_id`'s `tokens_used` unless that passes its
    /// quota, and record it as `hold`, in one step. False when the quota would
    /// be passed or the key is missing: then nothing is held or recorded.
    async fn reserve_key_quota(&self, hold: &Hold, key_id: &Uuid, amount: f64) -> Result<bool, sqlx::Error>;

    /// Add `amount` to a limits row's spend unless that passes its hard cap,
    /// and record it as `hold`, in one step. False when the cap would be
    /// passed or the row is missing.
    async fn reserve_limits(&self, hold: &Hold, entity_type: EntityType, entity_id: &str, amount: f64) -> Result<bool, sqlx::Error>;

    /// Settle reservation `id` on key `key_id` to `actual`: delete its record
    /// and apply `actual` less what it held, in one step. With its record
    /// gone, `if_gone` says what to do. Whether the record was there.
    async fn settle_key_quota(&self, id: &str, key_id: &Uuid, actual: f64, if_gone: IfGone) -> Result<bool, sqlx::Error>;

    /// [`settle_key_quota`](Self::settle_key_quota) for a limits row; the
    /// row's spend after, `None` when the row is gone or nothing was written.
    async fn settle_limits(&self, id: &str, entity_type: EntityType, entity_id: &str, actual: f64, if_gone: IfGone) -> Result<Option<f64>, sqlx::Error>;

    /// Give back what reservation `id` holds and delete its record, in one
    /// step. False when it has no record (settled, given back or swept).
    async fn release(&self, id: &str) -> Result<bool, sqlx::Error>;

    /// Renew the leases of `holder`'s reservations among `ids` to `lease` from
    /// the database's now. The ids it renewed: those still recorded.
    async fn renew(&self, holder: &str, ids: &[String], lease: chrono::Duration) -> Result<Vec<String>, sqlx::Error>;

    /// Renew the leases of these `Idempotency-Key`s, each while it is still in
    /// progress and held by the same request, to `lease` from the database's
    /// now. How many it renewed.
    async fn renew_idempotency(&self, keys: &[HeldKey], lease: chrono::Duration) -> Result<u64, sqlx::Error>;

    /// Give back up to `limit` reservations whose lease lapsed on the
    /// database's clock, and delete their records, in one step: of two
    /// sweepers, exactly one gives each back. How many it gave back.
    async fn sweep(&self, limit: u32) -> Result<u64, sqlx::Error>;

    /// Whether this store takes the reservations it records. SQLite takes
    /// them by migration 030's trigger `reservations_take`, which a database
    /// restored without it lacks; Postgres needs none.
    async fn takes_reservations(&self) -> Result<bool, sqlx::Error>;
}

/// What settling a reservation whose record is gone does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfGone {
    /// Charge the whole actual amount: the record was swept, and what it held
    /// given back. A request's first attempt to settle.
    ChargeActual,
    /// Change nothing: an earlier attempt, whose answer was lost, may have
    /// settled it. A retry.
    DoNothing,
}

/// A reservation about to be taken: its record's id, the replica that holds
/// it, and its first lease.
#[derive(Debug, Clone, PartialEq)]
pub struct Hold {
    pub id: String,
    pub holder: String,
    pub lease: chrono::Duration,
}

impl Hold {
    /// A hold of this replica, leased for [`LEASE`].
    pub fn new() -> Self {
        Self::leased(LEASE)
    }

    /// A hold of this replica, leased for `lease`.
    pub fn leased(lease: chrono::Duration) -> Self {
        Self { id: format!("rsv-{}", Uuid::new_v4()), holder: crate::shared::replica_id().to_string(), lease }
    }
}

impl Default for Hold {
    fn default() -> Self {
        Self::new()
    }
}

/// An `Idempotency-Key` in progress, held by the request `held_by` names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HeldKey {
    pub scope: String,
    pub key: String,
    pub held_by: String,
}

// ─── Live holds ──────────────────────────────────────────────────────────────

/// What a live request holds that its replica renews.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Renewable {
    /// A reservation's record, by its id.
    Reservation(String),
    /// An `Idempotency-Key` in progress.
    Key(HeldKey),
}

/// Everything this process's live requests hold, and when each lease was
/// last set to [`LEASE`] (taken or renewed), on this process's clock.
fn live() -> std::sync::MutexGuard<'static, std::collections::HashMap<Renewable, std::time::Instant>> {
    static LIVE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<Renewable, std::time::Instant>>> = std::sync::OnceLock::new();
    LIVE.get_or_init(Default::default).lock().unwrap_or_else(|p| p.into_inner())
}

/// A hold of a live request: its replica renews it while this lives, and
/// leaves it to lapse once this is dropped.
#[derive(Debug)]
pub struct Live(Renewable);

impl Live {
    /// Registered with `set`, when its lease was set to [`LEASE`] or before.
    fn register(what: Renewable, set: std::time::Instant) -> Self {
        live().insert(what.clone(), set);
        Self(what)
    }

    /// Until when, on this process's clock, its lease surely runs: [`LEASE`]
    /// after it was last set, less a margin for the time a renewal takes to
    /// land. No sweep can take its record before then.
    pub(crate) fn held_until(&self) -> Option<std::time::Instant> {
        let set = *live().get(&self.0)?;
        Some(set + LEASE.to_std().unwrap_or_default().saturating_sub(LEASE_MARGIN))
    }

    /// The `Idempotency-Key` in progress that this request holds as `held_by`.
    pub fn idempotency(scope: &str, key: &str, held_by: &str) -> Self {
        Self::register(
            Renewable::Key(HeldKey { scope: scope.to_string(), key: key.to_string(), held_by: held_by.to_string() }),
            std::time::Instant::now(),
        )
    }

    /// The reservation's record id, or the request an `Idempotency-Key` is
    /// held by.
    pub fn id(&self) -> &str {
        match &self.0 {
            Renewable::Reservation(id) => id,
            Renewable::Key(k) => &k.held_by,
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        live().remove(&self.0);
    }
}

impl Hold {
    /// Renew this hold's record while the [`Live`] lives, once its
    /// reservation is taken. `sent` is when the statement that took it was
    /// sent: Postgres starts the lease at the statement's start, which a lock
    /// wait can put long before its answer, so the lease is reckoned from no
    /// later than that (review round 3b, finding 2).
    pub fn live(&self, sent: std::time::Instant) -> Live {
        Live::register(Renewable::Reservation(self.id.clone()), sent)
    }
}

/// Renew what this process's live requests hold, to [`LEASE`] from now:
/// their reservations' records and their `Idempotency-Key`s. How many it
/// renewed.
pub async fn renew_live(db: &dyn crate::db::DatabaseStore) -> Result<u64, sqlx::Error> {
    let Some(store) = db.reservations() else { return Ok(0) };
    let (ids, keys) = {
        let live = live();
        let mut ids = Vec::new();
        let mut keys = Vec::new();
        for held in live.keys() {
            match held {
                Renewable::Reservation(id) => ids.push(id.clone()),
                Renewable::Key(key) => keys.push(key.clone()),
            }
        }
        (ids, keys)
    };
    let holder = crate::shared::replica_id();
    let mut renewed = 0;
    for batch in ids.chunks(BATCH) {
        // Taken before the statement: the lease it sets runs from no earlier.
        let set = std::time::Instant::now();
        let held = store.renew(holder, batch, LEASE).await?;
        renewed += held.len() as u64;
        let mut live = live();
        for id in held {
            if let Some(at) = live.get_mut(&Renewable::Reservation(id)) {
                *at = set;
            }
        }
    }
    for batch in keys.chunks(BATCH) {
        renewed += store.renew_idempotency(batch, LEASE).await?;
    }
    Ok(renewed)
}

/// How many records one renewal or sweep statement takes.
const BATCH: usize = 500;

/// Give back every reservation whose lease lapsed, a batch at a time. How
/// many it gave back.
pub async fn sweep_lapsed(db: &dyn crate::db::DatabaseStore) -> Result<u64, sqlx::Error> {
    let Some(store) = db.reservations() else { return Ok(0) };
    let mut given_back = 0;
    loop {
        let n = store.sweep(BATCH as u32).await?;
        given_back += n;
        if n < BATCH as u64 {
            return Ok(given_back);
        }
    }
}

/// How often each replica renews its live requests' holds and sweeps.
pub const UPKEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(20);

/// Every replica's upkeep, until the process ends: every [`UPKEEP_EVERY`] it
/// renews what its live requests hold, then gives back the reservations
/// whose lease lapsed, another replica's included. The first round runs at
/// once, so a replica that starts after a crash gives back the dead one's
/// reservations as soon as their leases lapse. It goes on through a graceful
/// stop: the requests still running keep their holds. First of all, it says
/// if the store doesn't take the reservations it records
/// ([`ReservationStore::takes_reservations`]).
pub fn spawn_upkeep(db: std::sync::Arc<dyn crate::db::DatabaseStore>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // A SQLite database restored without migration 030's trigger records
        // reservations without taking them: said once, as the replica starts.
        if let Some(store) = db.reservations() {
            match store.takes_reservations().await {
                Ok(true) => {}
                Ok(false) => tracing::error!(
                    "the database lacks migration 030's trigger reservations_take: billed requests' reservations are recorded but not taken, \
                     so each is charged its cost less its estimate; re-create the trigger with the CREATE TRIGGER statement of \
                     litegen-core/migrations/sqlite/20240101000030_reservations.sql (docs/RUNBOOK.md)"
                ),
                Err(e) => tracing::warn!(error = %e, "could not check for migration 030's trigger reservations_take"),
            }
        }
        let mut every = tokio::time::interval(UPKEEP_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            if let Some(Err(e)) = crate::supervise::guarded("a renewal of the live requests' holds", renew_live(&*db)).await {
                tracing::warn!(error = %e, "could not renew the leases of this replica's requests in flight; each lapses 2 minutes after its last renewal");
            }
            match crate::supervise::guarded("a sweep of lapsed reservations", sweep_lapsed(&*db)).await {
                Some(Ok(0)) | None => {}
                Some(Ok(given_back)) => tracing::info!(given_back, "gave back the reservations of requests that can no longer finish (their leases lapsed)"),
                Some(Err(e)) => tracing::warn!(error = %e, "could not sweep the lapsed reservations; the next round tries again"),
            }
        }
    })
}

// ─── Taking, settling and giving back ────────────────────────────────────────
//
// Each takes the path of a store that records its reservations, and the one
// of a store that keeps none (the test doubles: as before 030) otherwise.

/// What taking a reservation got.
#[derive(Debug)]
pub enum Reserved {
    /// Over the cap, or no such row: nothing is held.
    Refused,
    /// Held, with its record when the store keeps them.
    Held(Option<Live>),
}

/// Reserve `amount` on key `key_id`'s quota, recorded where `db` records
/// reservations.
pub async fn reserve_on_key(db: &dyn crate::db::DatabaseStore, key_id: &Uuid, amount: f64) -> Result<Reserved, sqlx::Error> {
    reserve_on_key_in(db.reservations(), db, key_id, amount).await
}

/// [`reserve_on_key`], recorded in `records`.
pub(crate) async fn reserve_on_key_in(
    records: Option<&dyn ReservationStore>,
    db: &dyn crate::db::DatabaseStore,
    key_id: &Uuid,
    amount: f64,
) -> Result<Reserved, sqlx::Error> {
    let Some(records) = records else {
        return Ok(if db.reserve_tokens(key_id, amount).await? { Reserved::Held(None) } else { Reserved::Refused });
    };
    let hold = Hold::new();
    let sent = std::time::Instant::now();
    match records.reserve_key_quota(&hold, key_id, amount).await {
        Ok(true) => Ok(Reserved::Held(Some(hold.live(sent)))),
        Ok(false) => Ok(Reserved::Refused),
        Err(e) => {
            give_back_lost(records, &hold).await;
            Err(e)
        }
    }
}

/// Reserve `amount` on a limits row's spend, recorded where `db` records
/// reservations.
pub async fn reserve_on_limits(
    db: &dyn crate::db::DatabaseStore,
    store: &dyn super::store::ManagementStore,
    entity_type: EntityType,
    entity_id: &str,
    amount: f64,
) -> Result<Reserved, sqlx::Error> {
    reserve_on_limits_in(db.reservations(), store, entity_type, entity_id, amount).await
}

/// [`reserve_on_limits`], recorded in `records`.
pub(crate) async fn reserve_on_limits_in(
    records: Option<&dyn ReservationStore>,
    store: &dyn super::store::ManagementStore,
    entity_type: EntityType,
    entity_id: &str,
    amount: f64,
) -> Result<Reserved, sqlx::Error> {
    let Some(records) = records else {
        return Ok(if store.reserve_limits_spend(entity_type, entity_id, amount).await? { Reserved::Held(None) } else { Reserved::Refused });
    };
    let hold = Hold::new();
    let sent = std::time::Instant::now();
    match records.reserve_limits(&hold, entity_type, entity_id, amount).await {
        Ok(true) => Ok(Reserved::Held(Some(hold.live(sent)))),
        Ok(false) => Ok(Reserved::Refused),
        Err(e) => {
            give_back_lost(records, &hold).await;
            Err(e)
        }
    }
}

/// A reservation step that failed may still have taken effect, its answer
/// lost (a dropped connection, a timeout): give back what its record holds,
/// if it has one, at once. Failing that, its record lapses unrenewed, and
/// the sweep gives it back.
async fn give_back_lost(records: &dyn ReservationStore, hold: &Hold) {
    match records.release(&hold.id).await {
        Ok(true) => tracing::info!(reservation = %hold.id, "gave back a reservation whose answer was lost"),
        Ok(false) => {}
        Err(e) => tracing::warn!(error = %e, reservation = %hold.id, "could not give back a reservation whose answer may have been lost; the sweep gives it back once its lease lapses"),
    }
}

/// When a failed settlement is tried again: after each of these, then every
/// 30 s (the last) until it lands or the process stops. Its record's
/// renewals keep its lease ([`LEASE`]) running meanwhile.
pub const SETTLE_RETRY_DELAYS: [std::time::Duration; 6] = [
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(4),
    std::time::Duration::from_secs(8),
    std::time::Duration::from_secs(15),
    std::time::Duration::from_secs(30),
];

/// One attempt at settling a recorded reservation: the store call, with
/// what to do if its record is gone.
pub(crate) type SettleAttempt<T> = Box<dyn FnMut(IfGone) -> futures::future::BoxFuture<'static, Result<T, sqlx::Error>> + Send>;

/// Settle a recorded reservation, `record`: `attempt` once, now, which
/// charges the whole actual if its record is gone (swept). `settled` tells
/// whether an answer settled it (its record was there). `on` and `actual`
/// name it in the logs. The first attempt's answer, and the task trying it
/// again if there is one.
///
/// An attempt that fails may still have landed, its answer lost. So the
/// record stays held, its lease renewed as before, and the settlement is
/// tried again in a tracked task after each of `delays`, then every last
/// delay until it lands or the process stops, changing nothing if the record
/// is gone. A record gone while its lease held can't have been swept: the
/// earlier attempt landed, and there is nothing to do. Gone with its lease no
/// longer sure, it can't be told which, so nothing more is charged, and an
/// error says so. A stop waits for the retries no longer than its grace and
/// then cuts them: the record, no longer renewed, goes to the sweep, which
/// gives the reservation back (review round 3b, finding 1).
pub(crate) async fn settle_recorded<T: Send + 'static>(
    record: Live,
    on: String,
    actual: f64,
    delays: &'static [std::time::Duration],
    mut attempt: SettleAttempt<T>,
    settled: fn(&T) -> bool,
) -> (Option<T>, Option<tokio::task::JoinHandle<()>>) {
    match attempt(IfGone::ChargeActual).await {
        Ok(answer) => return (Some(answer), None),
        Err(e) => tracing::warn!(error = %e, on = %on, actual, "a settlement failed; trying it again while its reservation stays held"),
    }
    let retries = crate::shutdown::spawn_tracked(async move {
        let every = delays.last().copied().unwrap_or(std::time::Duration::from_secs(30));
        let mut failures: u32 = 1;
        for delay in delays.iter().copied().chain(std::iter::repeat(every)) {
            tokio::time::sleep(delay).await;
            match attempt(IfGone::DoNothing).await {
                Ok(answer) if settled(&answer) => return,
                Ok(_) if record.held_until().is_some_and(|until| std::time::Instant::now() < until) => return,
                Ok(_) => {
                    tracing::error!(
                        on = %on,
                        actual,
                        "a failed settlement's reservation record is gone, settled by its first attempt or swept, which can't be told \
                         apart now that its lease isn't sure: nothing more is charged, so {actual} USD may be uncharged"
                    );
                    return;
                }
                Err(e) => {
                    failures += 1;
                    tracing::warn!(error = %e, on = %on, actual, failures, "a settlement failed again; it is tried again until it lands, its reservation held");
                }
            }
        }
    });
    (None, Some(retries))
}

/// Settle what was reserved on key `key_id` (`reserved`, recorded as
/// `record`) to `actual`. A failure is logged, never surfaced.
pub async fn settle_on_key(db: &std::sync::Arc<dyn crate::db::DatabaseStore>, record: Option<Live>, key_id: &Uuid, reserved: f64, actual: f64) {
    if let (Some(_), Some(record)) = (db.reservations(), record) {
        let (db, id, key_id) = (db.clone(), record.id().to_string(), *key_id);
        let attempt: SettleAttempt<bool> = Box::new(move |if_gone| {
            let (db, id) = (db.clone(), id.clone());
            Box::pin(async move {
                let records = db.reservations().ok_or_else(|| sqlx::Error::Protocol("the store keeps no reservations".into()))?;
                records.settle_key_quota(&id, &key_id, actual, if_gone).await
            })
        });
        settle_recorded(record, format!("key {key_id}"), actual, &SETTLE_RETRY_DELAYS, attempt, |settled| *settled).await;
        return;
    }
    let delta = actual - reserved;
    if delta.abs() > f64::EPSILON {
        if let Err(e) = db.atomic_charge_tokens(key_id, delta).await {
            tracing::warn!(error = %e, key_id = %key_id, "quota settlement failed");
        }
    }
}

/// Settle what was reserved on a limits row (`reserved`, recorded as
/// `record`) to `actual`. The row's spend after, `None` when that isn't
/// known (a failure, logged, never surfaced). Unrecorded, with nothing to
/// write, it is the spend `limits` read plus `reserved`.
pub async fn settle_on_limits(
    db: &std::sync::Arc<dyn crate::db::DatabaseStore>,
    store: &dyn super::store::ManagementStore,
    record: Option<Live>,
    limits: &super::types::EntityLimits,
    reserved: f64,
    actual: f64,
) -> Option<f64> {
    if let (Some(_), Some(record)) = (db.reservations(), record) {
        let (db, id, entity_type, entity_id) = (db.clone(), record.id().to_string(), limits.entity_type, limits.entity_id.clone());
        let on = format!("{} {entity_id}", entity_type.as_str());
        let attempt: SettleAttempt<Option<f64>> = Box::new(move |if_gone| {
            let (db, id, entity_id) = (db.clone(), id.clone(), entity_id.clone());
            Box::pin(async move {
                let records = db.reservations().ok_or_else(|| sqlx::Error::Protocol("the store keeps no reservations".into()))?;
                records.settle_limits(&id, entity_type, &entity_id, actual, if_gone).await
            })
        });
        return settle_recorded(record, on, actual, &SETTLE_RETRY_DELAYS, attempt, Option::is_some).await.0.flatten();
    }
    let delta = actual - reserved;
    if delta.abs() <= f64::EPSILON {
        return Some(limits.spend_usd + reserved);
    }
    match store.charge_limits_spend(limits.entity_type, &limits.entity_id, delta).await {
        Ok(spend) => spend,
        Err(e) => {
            tracing::warn!(error = %e, entity = %limits.entity_id, "limits settlement failed");
            None
        }
    }
}

/// Give back what was reserved on key `key_id` and never settled: what its
/// record holds, or `reserved` unrecorded. A failure is logged: the record
/// then lapses, and the sweep gives it back.
pub async fn give_back_on_key(db: &dyn crate::db::DatabaseStore, record: Option<Live>, key_id: &Uuid, reserved: f64) {
    if let (Some(records), Some(record)) = (db.reservations(), &record) {
        if let Err(e) = records.release(record.id()).await {
            tracing::warn!(error = %e, key_id = %key_id, "releasing a dropped quota reservation failed; the sweep gives it back once its lease lapses");
        }
        return;
    }
    if let Err(e) = db.atomic_charge_tokens(key_id, -reserved).await {
        tracing::warn!(error = %e, key_id = %key_id, "releasing a dropped quota reservation failed");
    }
}

/// [`give_back_on_key`] for a limits row.
pub async fn give_back_on_limits(db: &dyn crate::db::DatabaseStore, record: Option<Live>, entity_type: EntityType, entity_id: &str, reserved: f64) {
    if let (Some(records), Some(record)) = (db.reservations(), &record) {
        if let Err(e) = records.release(record.id()).await {
            tracing::warn!(error = %e, entity = %entity_id, "releasing a dropped limits reservation failed; the sweep gives it back once its lease lapses");
        }
        return;
    }
    let Some(store) = db.management() else { return };
    if let Err(e) = store.charge_limits_spend(entity_type, entity_id, -reserved).await {
        tracing::warn!(error = %e, entity = %entity_id, "releasing a dropped limits reservation failed");
    }
}
