//! Who delivers a finished 3D job, or keeps a finished video: a slot of this
//! process's delivery pool, then a lease on the generation's row that every
//! process respects, so the client's poll and the poller never deliver the
//! same job twice. Server state (the database); the delivery itself is
//! model3d_deliver.rs (moved from there,
//! docs/superpowers/specs/2026-11-07-engine-boundary-design.md D3), and
//! keeping a video is `proxy::router::rehost_video`.

/// How long a delivery lease holds the other observers off. It is taken
/// before the vendor's files are downloaded ([`DeliveryGate`]), so it covers
/// the downloads (each may run up to 600 s on a slow CDN) as well as the
/// delivery (the largest scene converts and re-hosts in well under a
/// minute): it lapses only on a holder that died mid-delivery. Every
/// observer uses this one length, since a lease is judged by its own.
pub const DELIVERY_LEASE: chrono::Duration = chrono::Duration::minutes(15);

/// The work a delivery lease is recorded under, `metadata.leases.delivery`.
const DELIVERY_WORK: &str = "delivery";

/// Deliveries this process runs at once, by either observer. A delivery of
/// every container peaks at 12–31 times the vendor's mesh (measured on vendor
/// meshes, `examples/mesh_peak.rs`), so this is what bounds a burst of
/// finishing jobs in memory.
pub fn delivery_slots() -> std::sync::Arc<tokio::sync::Semaphore> {
    static SLOTS: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(delivery_slot_count()))).clone()
}

/// How many [`delivery_slots`] there are: the cores, two to four.
pub fn delivery_slot_count() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(2, 4)
}

/// Finished videos this process keeps at once, by either observer. A video
/// streams to disk and then to the store a chunk at a time, so this bounds
/// bandwidth, and the bytes vendors (Veo, Bedrock, Sora), which hand a
/// video over whole (up to 1 GiB each), rather than memory per download.
pub const VIDEO_DELIVERY_SLOTS: usize = 8;

/// The pool of [`VIDEO_DELIVERY_SLOTS`]: apart from the 3D pool, so a burst
/// of finished videos never holds up a 3D delivery, nor the other way round.
pub fn video_delivery_slots() -> std::sync::Arc<tokio::sync::Semaphore> {
    static SLOTS: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(VIDEO_DELIVERY_SLOTS))).clone()
}

/// Which observer is claiming a delivery: the name its lease is recorded
/// under, and whether it waits for a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observer {
    /// The client's `GET /v1/models3d/{id}`. It waits [`CLIENT_SLOT_WAIT`] at
    /// most for a slot, then stands aside: a client polls again, and the
    /// poller delivers a job no client could.
    Client,
    /// The background poller, which waits its turn.
    Poller,
}

/// How long a client's poll waits for a delivery slot before it reports the
/// job still processing. Deliveries take well under this, so a slot that is
/// only briefly busy still answers the poll that saw the job finish.
pub const CLIENT_SLOT_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

impl Observer {
    fn name(self) -> &'static str {
        match self {
            Observer::Client => "client",
            Observer::Poller => "poller",
        }
    }
}

/// The right to deliver one finished job: one of this process's
/// [`delivery_slots`] and the row's delivery lease.
///
/// Hold it until the observation's outcome is written, then drop it.
/// Dropping it frees the slot and, in the background, releases the lease:
/// a no-op once the row is settled, and otherwise (a re-host that failed, a
/// client that hung up mid-conversion, a write that did not land) what lets
/// the next observer deliver at once rather than after [`DELIVERY_LEASE`].
/// Dropping it before the write would let a second observer deliver in
/// between.
pub struct DeliveryClaim {
    db: std::sync::Arc<dyn crate::db::DatabaseStore>,
    id: String,
    /// The holder recorded on the lease: the observer and a token of its own,
    /// so two observers in one process exclude each other too. `None` when no
    /// lease was taken (the store could not be asked).
    holder: Option<String>,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

impl Drop for DeliveryClaim {
    fn drop(&mut self) {
        let Some(holder) = self.holder.take() else { return };
        let (db, id) = (self.db.clone(), std::mem::take(&mut self.id));
        // `Drop` cannot await. A lease left behind only lapses, so a release
        // that cannot run here costs the next observer a wait, nothing more.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                if let Err(e) = db.release_generation_work(&id, DELIVERY_WORK, &holder).await {
                    tracing::warn!(generation_id = %id, error = %e, "could not release a 3d delivery lease; it lapses");
                }
            });
        }
    }
}

impl DeliveryClaim {
    /// Take the lease again from now, while this claim still holds it: a
    /// delivery that runs long (a large video's upload) renews it, so it
    /// cannot lapse while the work goes on. Best effort, like the release.
    pub async fn renew(&self) {
        let Some(holder) = self.holder.as_deref() else { return };
        match self.db.lease_generation_work(&self.id, DELIVERY_WORK, holder, DELIVERY_LEASE).await {
            Ok(true) => {}
            Ok(false) => tracing::warn!(generation_id = %self.id, "a delivery lease could not be renewed: the row is settled or another observer took it"),
            Err(e) => tracing::warn!(generation_id = %self.id, error = %e, "could not renew a delivery lease"),
        }
    }

    /// Run `work`, renewing the lease as it starts and then every third of
    /// [`DELIVERY_LEASE`] until it ends. The claim may be old by then: Veo,
    /// Bedrock and Sora are claimed before their download, which may take up
    /// to 600 s (payloads review #1).
    pub async fn renewing_while<T>(&self, work: impl std::future::Future<Output = T>) -> T {
        self.renew().await;
        let every = DELIVERY_LEASE.to_std().unwrap_or(std::time::Duration::from_secs(300)) / 3;
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
        tokio::pin!(work);
        loop {
            tokio::select! {
                out = &mut work => return out,
                _ = tick.tick() => self.renew().await,
            }
        }
    }
}

/// Claim the delivery of generation `id`'s finished job for `observer`.
/// `None` when another observer has it, or (for the client) when every
/// delivery slot stays busy past [`CLIENT_SLOT_WAIT`]: report [`stand_aside`](super::model3d_deliver::stand_aside)
/// instead of delivering.
///
/// A store that cannot be asked does not stop a delivery: the terminal write
/// is guarded either way, so the worst case is the duplicate work the lease
/// exists to save.
pub async fn claim_delivery(
    db: &std::sync::Arc<dyn crate::db::DatabaseStore>,
    id: &str,
    observer: Observer,
) -> Option<DeliveryClaim> {
    claim_from(&delivery_slots(), CLIENT_SLOT_WAIT, db, id, observer).await
}

/// Claim the keeping of generation `id`'s finished video for `observer`,
/// from the [`video_delivery_slots`]; as [`claim_delivery`] otherwise. The
/// lease is the same record: a row is a video or a 3D job, never both.
pub async fn claim_video_delivery(
    db: &std::sync::Arc<dyn crate::db::DatabaseStore>,
    id: &str,
    observer: Observer,
) -> Option<DeliveryClaim> {
    claim_from(&video_delivery_slots(), CLIENT_SLOT_WAIT, db, id, observer).await
}

/// [`claim_delivery`] from `slots`, a client waiting at most `wait` for one.
async fn claim_from(
    slots: &std::sync::Arc<tokio::sync::Semaphore>,
    wait: std::time::Duration,
    db: &std::sync::Arc<dyn crate::db::DatabaseStore>,
    id: &str,
    observer: Observer,
) -> Option<DeliveryClaim> {
    let slot = match observer {
        Observer::Client => tokio::time::timeout(wait, slots.clone().acquire_owned()).await.ok()?.ok()?,
        Observer::Poller => slots.clone().acquire_owned().await.ok()?,
    };
    let holder = format!("{}:{}", observer.name(), uuid::Uuid::new_v4());
    match db.lease_generation_work(id, DELIVERY_WORK, &holder, DELIVERY_LEASE).await {
        Ok(true) => Some(DeliveryClaim { db: db.clone(), id: id.to_string(), holder: Some(holder), _slot: slot }),
        Ok(false) => None,
        Err(e) => {
            tracing::warn!(generation_id = %id, error = %e, "could not lease a 3d delivery; delivering unleased");
            Some(DeliveryClaim { db: db.clone(), id: id.to_string(), holder: None, _slot: slot })
        }
    }
}

/// What a [`DeliveryGate`] decided.
pub enum GateState {
    /// The adapter never asked: it does not download in its poll (a mock),
    /// so the caller claims after the poll, as before.
    NotAsked,
    /// This observer delivers: hold the claim until the outcome is written.
    Claimed(DeliveryClaim),
    /// Another observer delivers; the adapter downloaded nothing.
    Refused,
}

/// The server's answer to an adapter's [`FetchGate`](crate::providers::FetchGate)
/// question: the delivery claim ([`claim_delivery`]), taken when the adapter
/// first asks, before it downloads a finished job's files. So an observer
/// that will stand aside never downloads them, and the poller holds no
/// files while it waits for a slot. Asked again, it gives the same answer.
pub struct DeliveryGate {
    db: std::sync::Arc<dyn crate::db::DatabaseStore>,
    id: String,
    observer: Observer,
    slots: std::sync::Arc<tokio::sync::Semaphore>,
    wait: std::time::Duration,
    state: tokio::sync::Mutex<GateState>,
}

impl DeliveryGate {
    /// The gate for generation `id`'s delivery by `observer`, from this
    /// process's [`delivery_slots`].
    pub fn new(db: &std::sync::Arc<dyn crate::db::DatabaseStore>, id: &str, observer: Observer) -> Self {
        Self::with_slots(delivery_slots(), CLIENT_SLOT_WAIT, db, id, observer)
    }

    /// The gate for keeping generation `id`'s finished video, from the
    /// [`video_delivery_slots`].
    pub fn for_video(db: &std::sync::Arc<dyn crate::db::DatabaseStore>, id: &str, observer: Observer) -> Self {
        Self::with_slots(video_delivery_slots(), CLIENT_SLOT_WAIT, db, id, observer)
    }

    fn with_slots(
        slots: std::sync::Arc<tokio::sync::Semaphore>,
        wait: std::time::Duration,
        db: &std::sync::Arc<dyn crate::db::DatabaseStore>,
        id: &str,
        observer: Observer,
    ) -> Self {
        Self { db: db.clone(), id: id.to_string(), observer, slots, wait, state: tokio::sync::Mutex::new(GateState::NotAsked) }
    }

    /// What the gate decided, with the claim when it took one.
    pub fn into_outcome(self) -> GateState {
        self.state.into_inner()
    }
}

#[async_trait::async_trait]
impl crate::providers::FetchGate for DeliveryGate {
    async fn ask(&self) -> crate::providers::Fetch {
        use crate::providers::Fetch;
        let mut state = self.state.lock().await;
        match &*state {
            GateState::Claimed(_) => return Fetch::Go { waited: std::time::Duration::ZERO },
            GateState::Refused => return Fetch::StandAside,
            GateState::NotAsked => {}
        }
        let started = tokio::time::Instant::now();
        match claim_from(&self.slots, self.wait, &self.db, &self.id, self.observer).await {
            Some(claim) => {
                *state = GateState::Claimed(claim);
                Fetch::Go { waited: started.elapsed() }
            }
            None => {
                *state = GateState::Refused;
                Fetch::StandAside
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 2026-11-11 P5: the gate claims once, however often it is
    /// asked, stands aside while another observer holds the lease or (for a
    /// client) while every slot stays busy, and reports when nobody asked.
    #[tokio::test]
    async fn a_gate_claims_once_and_remembers_its_answer() {
        use crate::providers::{Fetch, FetchGate};
        let db: std::sync::Arc<dyn crate::db::DatabaseStore> =
            std::sync::Arc::new(crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        for id in ["g-gate", "g-held", "g-busy"] {
            db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", Some("j"), 0.0, None, None).await.unwrap();
        }
        let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let wait = std::time::Duration::from_millis(100);

        let gate = DeliveryGate::with_slots(slots.clone(), wait, &db, "g-gate", Observer::Client);
        assert!(matches!(gate.ask().await, Fetch::Go { .. }));
        assert!(matches!(gate.ask().await, Fetch::Go { .. }), "asked again: the same answer");
        let other = DeliveryGate::with_slots(std::sync::Arc::new(tokio::sync::Semaphore::new(1)), wait, &db, "g-gate", Observer::Poller);
        assert_eq!(other.ask().await, Fetch::StandAside, "one claim: the lease is the first gate's");
        assert!(matches!(other.into_outcome(), GateState::Refused));
        assert!(matches!(gate.into_outcome(), GateState::Claimed(_)), "the claim is handed back");

        assert!(db.lease_generation_work("g-held", "delivery", "elsewhere", DELIVERY_LEASE).await.unwrap());
        let held = DeliveryGate::with_slots(slots.clone(), wait, &db, "g-held", Observer::Poller);
        assert_eq!(held.ask().await, Fetch::StandAside, "another observer holds the lease");

        let never = DeliveryGate::with_slots(slots.clone(), wait, &db, "g-busy", Observer::Client);
        assert!(matches!(never.into_outcome(), GateState::NotAsked));
        let _busy = slots.clone().acquire_owned().await.unwrap();
        let client = DeliveryGate::with_slots(slots.clone(), wait, &db, "g-busy", Observer::Client);
        assert_eq!(client.ask().await, Fetch::StandAside, "every slot busy past the client's wait");
    }

    /// A client's poll that finds every delivery slot busy waits a moment for
    /// one rather than answering `processing` at once, and stands aside only
    /// when they stay busy. A slot pool of its own: the process-wide one is
    /// shared with every test that delivers.
    #[tokio::test]
    async fn a_client_poll_waits_a_moment_for_a_busy_delivery_slot() {
        let db: std::sync::Arc<dyn crate::db::DatabaseStore> =
            std::sync::Arc::new(crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let wait = std::time::Duration::from_secs(5);

        let held = slots.clone().acquire_owned().await.unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drop(held);
        });
        let claim = claim_from(&slots, wait, &db, "g-wait", Observer::Client).await;
        assert!(claim.is_some(), "a slot freed within the wait is taken");
        drop(claim);

        let _held = slots.clone().acquire_owned().await.unwrap();
        let short = std::time::Duration::from_millis(100);
        let started = std::time::Instant::now();
        assert!(claim_from(&slots, short, &db, "g-busy", Observer::Client).await.is_none(), "slots busy past the wait: stand aside");
        assert!(started.elapsed() >= short);
    }

    /// Payloads review #1: a keep renews its delivery lease as it starts.
    /// Veo, Bedrock and Sora are claimed before the adapter's download (up
    /// to 600 s), so a keep may start with most of the lease gone; renewed
    /// first a third of the lease in, it could lapse before that renewal and
    /// let another observer keep the video too.
    #[tokio::test]
    async fn a_keep_renews_its_lease_as_it_starts() {
        let db: std::sync::Arc<dyn crate::db::DatabaseStore> =
            std::sync::Arc::new(crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        db.insert_generation("g-keep", None, "openai/sora-2", "openai", "video", Some("j"), 0.0, None, None).await.unwrap();
        let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let claim = claim_from(&slots, CLIENT_SLOT_WAIT, &db, "g-keep", Observer::Poller).await.expect("claimed");
        let holder = claim.holder.clone().expect("a lease was taken");
        // 14 of its 15 minutes gone: taken before a long download, then a wait.
        let taken = chrono::Utc::now() - chrono::Duration::minutes(14);
        let aged = serde_json::json!({ "leases": { "delivery": crate::db::stage_claim_record(&holder, taken) } });
        db.merge_generation_metadata_if_active("g-keep", &aged).await.unwrap();
        let free_two_minutes_in = claim
            .renewing_while(async {
                let meta = db.get_generation("g-keep").await.unwrap().expect("the row").metadata;
                let then = chrono::Utc::now() + chrono::Duration::minutes(2);
                crate::db::lease_is_free(meta.as_ref(), DELIVERY_WORK, "client:another", DELIVERY_LEASE, then)
            })
            .await;
        assert!(!free_two_minutes_in, "two minutes into the keep another observer could take the delivery: the lease was not renewed as the keep started");
    }
}
