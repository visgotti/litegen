use std::sync::Arc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tracing::{info, warn};

use crate::db::DatabaseStore;
use crate::types::{Generation, WebhookDelivery};

// Moved from util/ssrf.rs (engine boundary, D7): webhooks are the server's,
// and so is the deployment mode it reads.
/// The client a webhook goes out with. In hosted mode the URL is a tenant's,
/// so the client connects only to public addresses, on the first attempt and
/// on every retry. In single-tenant mode it connects anywhere, for the keys
/// that may reach the deployment's network ([`may_reach_inside`]): a caller
/// sends every other key's webhook on a public-only client instead
/// (`dispatch_terminal_webhook`, test-webhook).
pub fn webhook_client(mode: crate::config::Mode) -> reqwest::Client {
    match mode {
        crate::config::Mode::Hosted => crate::util::ssrf::public_only_client(),
        crate::config::Mode::SingleTenant => crate::util::ssrf::no_redirect_client(),
    }
}

type HmacSha256 = Hmac<Sha256>;

/// Webhook deliveries sending at once for one org, per replica.
pub const DELIVERIES_PER_ORG: usize = 16;
/// Webhook deliveries sending at once in all, per replica.
pub const DELIVERIES_IN_ALL: usize = 256;
/// One org's deliveries waiting or running: every this many, a warning.
const ORG_BACKLOG_WARNING: usize = 1000;

/// The terminal webhooks this process is sending. Each runs on its own task
/// once it holds a slot of its org's ([`DELIVERIES_PER_ORG`]) and then one of
/// the process's ([`DELIVERIES_IN_ALL`]): one org whose receiver is slow or
/// hangs (an attempt waits up to 30 s for its answer) holds at most its own
/// share, never every slot, and its queue holds none of the process's. The
/// rest wait in memory, in order.
///
/// An outbox delivery ([`Outbox`]) runs here one attempt at a time: between
/// attempts it waits in the database, not here, so a stop never waits for
/// it and a crash loses nothing. A delivery sent from memory (where its
/// terminal write recorded no outbox delivery: a database without migration
/// 029's trigger, or a store that keeps no outbox) runs here whole, its
/// retries included, holding its slots only while it sends: waiting for its
/// next attempt (up to 10 min) it gives both back and takes them again in
/// turn (`without_slots`). Those live in this process's memory only: a stop
/// [drains](Self::drain) them within what is left of its grace, and a crash
/// or a kill loses them.
pub struct WebhookDeliveries {
    all: Arc<tokio::sync::Semaphore>,
    per_org: usize,
    /// Each org with a delivery waiting or running.
    orgs: std::sync::Mutex<std::collections::HashMap<String, Arc<OrgDeliveries>>>,
    /// Every delivery waiting or running, by its number: its generation.
    running: std::sync::Mutex<std::collections::HashMap<u64, String>>,
    next: std::sync::atomic::AtomicU64,
    /// Woken when the last delivery ends.
    idle: tokio::sync::Notify,
}

/// One org's slots, and its deliveries waiting or running (changed only
/// under the `orgs` lock; the entry goes when they are back to 0).
struct OrgDeliveries {
    slots: Arc<tokio::sync::Semaphore>,
    count: std::sync::atomic::AtomicUsize,
}

/// A delivery's place in the dispatcher, given back when its task ends,
/// however it ends.
struct Ticket {
    owner: Arc<WebhookDeliveries>,
    number: u64,
    org: String,
    bucket: Arc<OrgDeliveries>,
}

fn locked<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Drop for Ticket {
    fn drop(&mut self) {
        {
            let mut orgs = locked(&self.owner.orgs);
            let last = self.bucket.count.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) == 1;
            if last && orgs.get(&self.org).is_some_and(|b| Arc::ptr_eq(b, &self.bucket)) {
                orgs.remove(&self.org);
            }
        }
        let mut running = locked(&self.owner.running);
        running.remove(&self.number);
        if running.is_empty() {
            self.owner.idle.notify_waiters();
        }
    }
}

impl WebhookDeliveries {
    /// At most `all` deliveries at once, and `per_org` for one org.
    pub fn new(all: usize, per_org: usize) -> Self {
        let bound = |n: usize| n.clamp(1, tokio::sync::Semaphore::MAX_PERMITS);
        Self {
            all: Arc::new(tokio::sync::Semaphore::new(bound(all))),
            per_org: bound(per_org),
            orgs: Default::default(),
            running: Default::default(),
            next: Default::default(),
            idle: tokio::sync::Notify::new(),
        }
    }

    /// Run `delivery`, the webhook of generation `generation_id` of `org`,
    /// on its own task once it holds its org's slot and then one of all,
    /// which it gives back while it waits for a retry (`without_slots`).
    /// Returns at once.
    pub fn spawn(self: &Arc<Self>, org: &str, generation_id: &str, delivery: impl std::future::Future<Output = ()> + Send + 'static) {
        let number = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        locked(&self.running).insert(number, generation_id.to_string());
        let bucket = {
            let mut orgs = locked(&self.orgs);
            let bucket = orgs
                .entry(org.to_string())
                .or_insert_with(|| {
                    Arc::new(OrgDeliveries { slots: Arc::new(tokio::sync::Semaphore::new(self.per_org)), count: Default::default() })
                })
                .clone();
            let count = bucket.count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if count.is_multiple_of(ORG_BACKLOG_WARNING) {
                warn!(org = %org, deliveries = count, "one org's webhook deliveries are piling up (its receiver is slow or down)");
            }
            bucket
        };
        let ticket = Ticket { owner: self.clone(), number, org: org.to_string(), bucket };
        tokio::spawn(async move {
            // Dropped last, after the slots, whatever ends the task.
            let ticket = ticket;
            let Ok(org_slot) = ticket.bucket.slots.clone().acquire_owned().await else { return };
            let Ok(slot) = ticket.owner.all.clone().acquire_owned().await else { return };
            // Held while the delivery sends, given back while it waits for a retry.
            let held = HeldSlots {
                org: ticket.bucket.slots.clone(),
                all: ticket.owner.all.clone(),
                permits: std::sync::Mutex::new(Some((org_slot, slot))),
            };
            HELD.scope(held, delivery).await;
        });
    }

    /// Wait until no delivery is waiting or running, or until `deadline`.
    /// Returns the generation ids of the deliveries still waiting or running
    /// then (sorted); they go on until the process ends. It never closes the
    /// dispatcher: a delivery spawned meanwhile is waited for too.
    pub async fn drain(&self, deadline: tokio::time::Instant) -> Vec<String> {
        loop {
            let idle = self.idle.notified();
            let mut idle = std::pin::pin!(idle);
            // Listening before looking, so an end in between is not missed.
            idle.as_mut().enable();
            if locked(&self.running).is_empty() {
                return Vec::new();
            }
            if tokio::time::timeout_at(deadline, idle).await.is_err() {
                let mut left: Vec<String> = locked(&self.running).values().cloned().collect();
                left.sort();
                return left;
            }
        }
    }
}

tokio::task_local! {
    /// The slots of the delivery running on this task, set by
    /// [`WebhookDeliveries::spawn`]: [`without_slots`] gives them back while
    /// the delivery waits for its next attempt.
    static HELD: HeldSlots;
}

/// A running delivery's slots, its org's and its one of all, and where to
/// take them again from.
struct HeldSlots {
    org: Arc<tokio::sync::Semaphore>,
    all: Arc<tokio::sync::Semaphore>,
    /// `None` while given back.
    permits: std::sync::Mutex<Option<(tokio::sync::OwnedSemaphorePermit, tokio::sync::OwnedSemaphorePermit)>>,
}

/// Waits out `wait`, a delivery's back-off, holding no slot: a delivery on
/// the dispatcher's task ([`WebhookDeliveries::spawn`]) gives its org's slot
/// and its slot of all back first, so another delivery can go meanwhile, and
/// takes them again after, its org's first as at its start, before its next
/// attempt. Anywhere else (a direct call, a test) it only waits.
async fn without_slots(wait: impl std::future::Future<Output = ()>) {
    let semaphores = HELD.try_with(|held| {
        drop(locked(&held.permits).take());
        (held.org.clone(), held.all.clone())
    });
    wait.await;
    let Ok((org, all)) = semaphores else { return };
    let Ok(org) = org.acquire_owned().await else { return };
    let Ok(all) = all.acquire_owned().await else { return };
    let _ = HELD.try_with(|held| *locked(&held.permits) = Some((org, all)));
}

/// This process's webhook deliveries.
pub fn deliveries() -> &'static Arc<WebhookDeliveries> {
    static DELIVERIES: std::sync::OnceLock<Arc<WebhookDeliveries>> = std::sync::OnceLock::new();
    DELIVERIES.get_or_init(|| Arc::new(WebhookDeliveries::new(DELIVERIES_IN_ALL, DELIVERIES_PER_ORG)))
}

/// How long an outbox delivery waits after a failed attempt before its next:
/// 10 s, 30 s, 1, 2, 5, 10 and 30 min, then 1, 2, 4, 8 and 8 h. That makes 13
/// attempts, the last about 23 h 49 min after the first (a few minutes more
/// when attempts wait out their 30 s for an answer). A 410 Gone ends a
/// delivery at once; a network error, a timeout and every other answer but a
/// 2xx are retried alike. docs/management-api.md ("Key webhooks") gives
/// receivers this schedule.
pub const WEBHOOK_RETRY_DELAYS: [std::time::Duration; 12] = [
    std::time::Duration::from_secs(10),
    std::time::Duration::from_secs(30),
    std::time::Duration::from_secs(60),
    std::time::Duration::from_secs(2 * 60),
    std::time::Duration::from_secs(5 * 60),
    std::time::Duration::from_secs(10 * 60),
    std::time::Duration::from_secs(30 * 60),
    std::time::Duration::from_secs(3600),
    std::time::Duration::from_secs(2 * 3600),
    std::time::Duration::from_secs(4 * 3600),
    std::time::Duration::from_secs(8 * 3600),
    std::time::Duration::from_secs(8 * 3600),
];

/// The waits of a delivery sent from memory ([`deliver`]): where no outbox
/// delivery was recorded. The outbox's first six, 7 attempts over about 19
/// minutes, as before the outbox: a day would be too long to wait in memory.
const MEMORY_RETRY_DELAYS: [std::time::Duration; 6] = [
    WEBHOOK_RETRY_DELAYS[0],
    WEBHOOK_RETRY_DELAYS[1],
    WEBHOOK_RETRY_DELAYS[2],
    WEBHOOK_RETRY_DELAYS[3],
    WEBHOOK_RETRY_DELAYS[4],
    WEBHOOK_RETRY_DELAYS[5],
];

/// The header that names a delivery, `whd-` and a UUID: the same on every
/// attempt of one delivery and new for each delivery, so a receiver can drop
/// a repeat. The signature covers the body, not this header.
pub const DELIVERY_ID_HEADER: &str = "x-litegen-delivery-id";

/// A new delivery's id.
fn new_delivery_id() -> String {
    format!("whd-{}", uuid::Uuid::new_v4())
}

/// One attempt's request: the JSON `body`, its signature header and its
/// delivery's id.
fn attempt_request(client: &reqwest::Client, url: &str, signature: &str, delivery_id: &str, body: String) -> reqwest::RequestBuilder {
    client
        .post(url)
        .header("content-type", "application/json")
        .header("x-litegen-signature", signature)
        .header(DELIVERY_ID_HEADER, delivery_id)
        .body(body)
}

/// POST `payload` as JSON to `url`, signed with HMAC-SHA256, until an
/// attempt is answered 2xx.
///
/// The signature is included as `X-Litegen-Signature: sha256=<hex>`.
/// If `secret` is None the generation id is used as a placeholder secret so
/// the receiver can at least verify the id hasn't been tampered with. Every
/// attempt carries the delivery's id ([`DELIVERY_ID_HEADER`]).
///
/// A failed attempt is retried after 10 s, 30 s, 1, 2, 5 and 10 min, in
/// memory: the schedule of a delivery sent from memory, the outbox's first
/// six waits. A 410 Gone ends it at once, as it ends an outbox delivery.
/// Returns `Ok(())` if any attempt succeeds, `Err(String)` describing the
/// final failure otherwise.
pub async fn dispatch_webhook(
    client: &reqwest::Client,
    url: &str,
    secret: Option<&str>,
    payload: &Generation,
) -> Result<(), String> {
    deliver(client, url, secret, payload, None, tokio::time::sleep).await
}

/// POST `payload` as JSON to `url`, signed with HMAC-SHA256 — ONE attempt, no
/// retries: a delivery of its own, with a new [`DELIVERY_ID_HEADER`].
///
/// Returns the raw `reqwest::Response` on success (any HTTP status), or a
/// `reqwest::Error` if the request itself failed (network / timeout).
pub async fn dispatch_webhook_once(
    client: &reqwest::Client,
    url: &str,
    secret: Option<&str>,
    payload: &Generation,
) -> Result<reqwest::Response, reqwest::Error> {
    let body = serde_json::to_string(payload).unwrap_or_default();
    let effective_secret = secret.unwrap_or(&payload.id);
    let signature = compute_signature(effective_secret, &body);
    let sig_header = format!("sha256={signature}");

    attempt_request(client, url, &sig_header, &new_delivery_id(), body).send().await
}

/// Like [`dispatch_webhook`] but records each attempt in the `webhook_deliveries`
/// table via `db`.  A failed insert never aborts the delivery.
pub async fn dispatch_webhook_logged(
    client: &reqwest::Client,
    url: &str,
    secret: Option<&str>,
    payload: &Generation,
    db: Arc<dyn DatabaseStore>,
    key_id: &str,
) -> Result<(), String> {
    deliver(client, url, secret, payload, Some(AttemptLog { db: &*db, key_id }), tokio::time::sleep).await
}

/// Where a delivery records its attempts: `key_id`'s rows in `webhook_deliveries`.
struct AttemptLog<'a> {
    db: &'a dyn DatabaseStore,
    key_id: &'a str,
}

/// One delivery of `payload` to `url`, the loop behind [`dispatch_webhook`]
/// and [`dispatch_webhook_logged`]: the first attempt at once, then a retry
/// after each of the in-memory waits until one is answered 2xx, all
/// under one delivery id. Each attempt is recorded in `log`, when there is
/// one; `wait` waits out each back-off (`tokio::time::sleep`, or a test's own
/// clock), holding no slot of the dispatcher's ([`without_slots`]).
async fn deliver<W, F>(
    client: &reqwest::Client,
    url: &str,
    secret: Option<&str>,
    payload: &Generation,
    log: Option<AttemptLog<'_>>,
    wait: W,
) -> Result<(), String>
where
    W: Fn(std::time::Duration) -> F,
    F: std::future::Future<Output = ()>,
{
    let body = match serde_json::to_string(payload) {
        Ok(b) => b,
        Err(e) => return Err(format!("serialise payload: {e}")),
    };

    let effective_secret = secret.unwrap_or(&payload.id);
    let signature = compute_signature(effective_secret, &body);
    let sig_header = format!("sha256={signature}");

    let delivery_id = new_delivery_id();
    let attempts = MEMORY_RETRY_DELAYS.len() + 1;
    let mut last_err = String::new();

    // The first attempt goes at once, each retry after its wait.
    let waits = std::iter::once(None).chain(MEMORY_RETRY_DELAYS.map(Some));
    for (attempt, delay) in (1i32..).zip(waits) {
        if let Some(delay) = delay {
            without_slots(wait(delay)).await;
        }

        info!(
            generation_id = %payload.id,
            delivery_id = %delivery_id,
            url = %url,
            attempt,
            "Dispatching webhook"
        );

        let answer = match attempt_request(client, url, &sig_header, &delivery_id, body.clone()).send().await {
            Ok(resp) => Answer::of(resp).await,
            Err(e) => Answer::failed(e.to_string()),
        };
        let sent = SentAttempt {
            key_id: log.as_ref().map_or("", |l| l.key_id),
            generation_id: &payload.id,
            delivery_id: &delivery_id,
            url,
            number: attempt,
            payload_json: &body,
        };
        note_attempt(log.as_ref().map(|l| l.db), &sent, &answer).await;
        if answer.success {
            return Ok(());
        }
        last_err = answer.status.map_or_else(|| answer.error.clone().unwrap_or_default(), |code| format!("HTTP {code}"));
        if answer.is_gone() {
            warn!(generation_id = %payload.id, delivery_id = %delivery_id, url = %url, "Webhook delivery failed: the endpoint answered 410 Gone, so it is not retried");
            return Err(format!("webhook failed at attempt {attempt}: {last_err}, which ends a delivery"));
        }
    }

    warn!(
        generation_id = %payload.id,
        delivery_id = %delivery_id,
        url = %url,
        last_error = %last_err,
        "Webhook delivery failed after all retries"
    );
    Err(format!("webhook failed after {attempts} attempts: {last_err}"))
}

/// One attempt of a delivery, as its row and its log lines tell it.
struct SentAttempt<'a> {
    key_id: &'a str,
    generation_id: &'a str,
    delivery_id: &'a str,
    url: &'a str,
    /// Its number in its delivery's series.
    number: i32,
    /// What its row keeps of the payload: the body sent, or `""` for an
    /// outbox attempt (or one that couldn't be sent), whose delivery keeps
    /// the payload once.
    payload_json: &'a str,
}

/// Log what attempt `a` got back, and record it in `db`'s
/// `webhook_deliveries` when there is one ([`record_attempt`]): a delivery
/// sent from memory ([`deliver`]) and an outbox attempt alike.
async fn note_attempt(db: Option<&dyn DatabaseStore>, a: &SentAttempt<'_>, answer: &Answer) {
    match (answer.status, &answer.error) {
        (Some(code), _) if answer.success => {
            info!(generation_id = %a.generation_id, delivery_id = %a.delivery_id, url = %a.url, status = code, "Webhook delivered");
        }
        (Some(code), _) => {
            warn!(generation_id = %a.generation_id, delivery_id = %a.delivery_id, url = %a.url, status = code, attempt = a.number, "Webhook non-2xx response");
        }
        (None, Some(e)) => {
            warn!(generation_id = %a.generation_id, delivery_id = %a.delivery_id, url = %a.url, error = %e, attempt = a.number, "Webhook request error");
        }
        (None, None) => {}
    }
    if let Some(db) = db {
        record_attempt(db, a, answer).await;
    }
}

/// Record attempt `a` and what came of it in `db`'s `webhook_deliveries`:
/// the one place an attempt row is written, for a delivery sent from memory,
/// an outbox attempt, and an outbox delivery that can't be sent (no status,
/// the reason as its error). A failed insert never stops the delivery.
async fn record_attempt(db: &dyn DatabaseStore, a: &SentAttempt<'_>, answer: &Answer) {
    let row = WebhookDelivery {
        id: format!("wh-{}", uuid::Uuid::new_v4()),
        key_id: a.key_id.to_string(),
        generation_id: a.generation_id.to_string(),
        url: a.url.to_string(),
        attempt_number: a.number,
        status_code: answer.status.map(i32::from),
        success: answer.success,
        response_body: answer.reply.clone(),
        error_message: answer.error.clone(),
        payload_json: a.payload_json.to_string(),
        created_at: chrono::Utc::now(),
        delivery_id: Some(a.delivery_id.to_string()),
    };
    if let Err(e) = db.insert_webhook_delivery(&row).await {
        warn!(generation_id = %a.generation_id, delivery_id = %a.delivery_id, error = %e, "Failed to record webhook delivery");
    }
}

// ─── The durable outbox (migration 029) ─────────────────────────────────────

/// How long a dispatcher's claim on an outbox delivery holds unless it is
/// renewed: past it (a replica that died), another dispatcher takes it.
pub const OUTBOX_LEASE: std::time::Duration = std::time::Duration::from_secs(60);

/// How often each replica's dispatcher looks for due deliveries.
pub const OUTBOX_TICK: std::time::Duration = std::time::Duration::from_secs(1);

/// How long a new delivery is left to the replica whose terminal write
/// recorded it, before any dispatcher may send it: that replica settles the
/// job first (a cancel waits up to 5 s for the vendor before its refund) and
/// then sends the first attempt at once ([`Outbox::send_first`]), so the
/// webhook carries the settled row. The dispatcher sends it after the grace,
/// should that replica die first. The trigger of migration 032 writes it:
/// a new delivery's `next_attempt_at` is 30 s after its terminal write.
pub const OUTBOX_FIRST_ATTEMPT_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// What an outbox attempt is sent with.
#[derive(Clone)]
pub struct SendContext {
    /// The deployment's tenancy: hosted, a webhook goes to public addresses only.
    pub mode: crate::config::Mode,
    /// Who may reach the deployment's own network (`webhooks.internal_targets`).
    pub internal: crate::config::InternalTargets,
    /// How each app's files are handed out (its access mode). `None` sends
    /// the URLs as stored.
    pub presenting: Option<crate::access::present::Presenting>,
    /// The client for the keys whose webhook may reach inside the deployment
    /// ([`webhook_client`]); every other key's goes out on a public-only one.
    pub inside_client: reqwest::Client,
}

impl SendContext {
    /// What `presenting`'s replica sends with: who may reach inside is the
    /// deployment's setting, read from its config (the default without one).
    pub fn new(mode: crate::config::Mode, presenting: Option<crate::access::present::Presenting>, inside_client: reqwest::Client) -> Self {
        let internal = presenting.as_ref().map(|p| p.router.config.webhooks.internal_targets).unwrap_or_default();
        Self { mode, internal, presenting, inside_client }
    }
}

/// What a stop's [`Outbox::release_claims`] did with this replica's leases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleasedClaims {
    /// Leases given back: any dispatcher's next tick may take those deliveries.
    pub freed: u64,
    /// Leases kept: attempts still sending, whose deliveries stay this
    /// replica's until they end or the lease lapses.
    pub kept: usize,
}

/// What the terminal moment found for its generation's outbox delivery
/// ([`Outbox::send_first`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstAttempt {
    /// This replica claimed it, and its first attempt has run.
    Sent,
    /// It stays with the outbox: another dispatcher holds it, it is done, or
    /// this replica can't claim it now (a stop, a database error).
    LeftToTheOutbox,
    /// The terminal write recorded none.
    Unrecorded,
}

/// A replica's dispatcher of the durable outbox (migration 029). Every
/// replica runs one ([`spawn`](Self::spawn)); this process's is [`outbox`].
///
/// A job's guarded terminal write records its webhook as a pending delivery
/// in the database, in the same statement (a trigger; see
/// `dispatch_terminal_webhook`), due after its first-attempt grace
/// ([`OUTBOX_FIRST_ATTEMPT_GRACE`]). The replica that wrote it settles the
/// job, then claims the delivery and sends its first attempt at once
/// ([`send_first`](Self::send_first)), so the webhook carries the settled
/// row. Every [`OUTBOX_TICK`] each dispatcher renews the leases of the
/// deliveries it holds, then claims the due ones nobody holds, with a lease
/// of [`OUTBOX_LEASE`] on the database's clock: a retry whose time has come,
/// a delivery whose replica died before its first attempt (its grace is
/// over), or one whose replica died holding it (its lease lapsed). It holds at most
/// [`DELIVERIES_IN_ALL`] at once, [`DELIVERIES_PER_ORG`] of one org, so one
/// org's backlog waits in the database, not in memory. Each attempt runs on
/// this process's [`WebhookDeliveries`], within its org's share and the
/// process's.
///
/// Each attempt is built when it is sent, from the generation and its key as
/// they are then: the files' links presented (signed) per the app's access
/// mode, signed with the key's secret, sent under the delivery's id, to where
/// the key may send now (SEC-STWEBHOOK). It is recorded in
/// `webhook_deliveries`, without a copy of the payload, which the delivery
/// keeps once (its latest attempt's). A 2xx ends the delivery `delivered`;
/// a 410 Gone, its last attempt, or a delivery that can't be sent (its key's
/// webhook_url cleared, its generation gone, an address it may not reach)
/// ends it `failed`; anything else makes its next attempt due after the next
/// wait of the schedule ([`WEBHOOK_RETRY_DELAYS`]), stored with it, so a
/// restart resumes where it was. Delivery is at least once: a replica that
/// dies, or a stop that ends mid-attempt, may leave an attempt the receiver
/// got to be sent again.
pub struct Outbox {
    claimant: String,
    deliveries: Arc<WebhookDeliveries>,
    schedule: Vec<std::time::Duration>,
    lease: chrono::Duration,
    claim_limit: usize,
    claim_per_org: usize,
    /// The deliveries this dispatcher holds a lease on: waiting for their
    /// slots, or sending.
    in_hand: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Those of them whose attempt is under way: a stop keeps their leases
    /// ([`release_claims`](Self::release_claims)).
    sending: std::sync::Mutex<std::collections::HashSet<String>>,
    stopping: std::sync::atomic::AtomicBool,
}

/// An attempt this dispatcher is making: in its `sending` until dropped.
struct Sending<'a> {
    outbox: &'a Outbox,
    id: String,
}

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        locked(&self.outbox.sending).remove(&self.id);
    }
}

/// A delivery this dispatcher holds: in its `in_hand` until dropped.
struct InHand {
    outbox: Arc<Outbox>,
    id: String,
}

impl Drop for InHand {
    fn drop(&mut self) {
        locked(&self.outbox.in_hand).remove(&self.id);
    }
}

impl Outbox {
    /// A dispatcher that claims as `claimant`, sends on `deliveries`, and
    /// waits `schedule[n - 1]` after a delivery's `n`-th failed attempt (after
    /// the last, the delivery is failed).
    pub fn new(claimant: impl Into<String>, deliveries: Arc<WebhookDeliveries>, schedule: Vec<std::time::Duration>) -> Self {
        Self {
            claimant: claimant.into(),
            deliveries,
            schedule,
            lease: chrono::Duration::from_std(OUTBOX_LEASE).unwrap_or_else(|_| chrono::Duration::seconds(60)),
            claim_limit: DELIVERIES_IN_ALL,
            claim_per_org: DELIVERIES_PER_ORG,
            in_hand: Default::default(),
            sending: Default::default(),
            stopping: Default::default(),
        }
    }

    /// One round: renew the leases of the deliveries this dispatcher holds,
    /// then, when one is due (a read), claim the due ones there is room for,
    /// and start each. Returns how many it started. A stopped dispatcher
    /// claims nothing.
    pub async fn tick(self: &Arc<Self>, db: &Arc<dyn DatabaseStore>, ctx: &SendContext) -> usize {
        if self.is_stopping() {
            return 0;
        }
        let held: Vec<String> = locked(&self.in_hand).iter().cloned().collect();
        if !held.is_empty() {
            if let Err(e) = db.renew_webhook_claims(&self.claimant, &held, self.lease).await {
                warn!(error = %e, held = held.len(), "could not renew this replica's claims on webhook deliveries; one whose claim lapses may be sent by another replica too");
            }
        }
        let room = self.claim_limit.saturating_sub(held.len());
        if room == 0 {
            return 0;
        }
        // A read first: an idle dispatcher claims (an UPDATE, which on
        // SQLite takes the write lock) only once a delivery is due.
        match db.webhook_delivery_due().await {
            Ok(true) => {}
            Ok(false) => return 0,
            Err(e) => {
                warn!(error = %e, "could not look for due webhook deliveries; the next tick tries again");
                return 0;
            }
        }
        let claimed = match db.claim_due_webhook_deliveries(&self.claimant, room as u32, self.claim_per_org as u32, self.lease).await {
            Ok(claimed) => claimed,
            Err(e) => {
                warn!(error = %e, "could not claim the due webhook deliveries; the next tick tries again");
                return 0;
            }
        };
        let mut started = 0;
        for delivery in claimed {
            if let Some(held) = self.hold(&delivery.id) {
                self.start(db, ctx, delivery, held);
                started += 1;
            }
        }
        started
    }

    /// `id` in this dispatcher's hand; `None` when it is already (its task
    /// runs, and keeps it).
    fn hold(self: &Arc<Self>, id: &str) -> Option<InHand> {
        locked(&self.in_hand).insert(id.to_string()).then(|| InHand { outbox: self.clone(), id: id.to_string() })
    }

    /// Start `delivery`, which this dispatcher holds: its attempt runs on its
    /// own task once it holds its org's slot and one of all. One whose slots
    /// come after a stop sends nothing: it stays due, and its lease goes back
    /// with this replica's others.
    fn start(self: &Arc<Self>, db: &Arc<dyn DatabaseStore>, ctx: &SendContext, delivery: crate::db::OutboxDelivery, held: InHand) {
        let org = delivery.org_id.clone().unwrap_or_else(|| crate::api::middleware::DEFAULT_ORG_ID.to_string());
        let generation_id = delivery.generation_id.clone();
        let (me, db, ctx) = (self.clone(), db.clone(), ctx.clone());
        self.deliveries.spawn(&org, &generation_id, async move {
            let _held = held;
            me.attempt_unless_stopping(&db, &ctx, &delivery).await;
        });
    }

    /// Attempt `d` now, unless this dispatcher is stopping: whether it did.
    /// It counts as sending from before the stop is checked, so a stop that
    /// comes meanwhile either keeps its lease or is seen here first.
    async fn attempt_unless_stopping(&self, db: &Arc<dyn DatabaseStore>, ctx: &SendContext, d: &crate::db::OutboxDelivery) -> bool {
        locked(&self.sending).insert(d.id.clone());
        let _sending = Sending { outbox: self, id: d.id.clone() };
        if self.is_stopping() {
            return false;
        }
        self.attempt(db, ctx, d).await;
        true
    }

    /// The terminal moment (`dispatch_terminal_webhook`, after the job is
    /// settled), on a task of the dispatcher that holds its slots: claim
    /// `generation_id`'s delivery, which its terminal write recorded, and send
    /// its first attempt now, within its grace, as fast as the in-memory
    /// delivery did. While stopping it claims nothing: the delivery is sent
    /// once its grace is over, by another replica or by this one after its
    /// restart.
    pub async fn send_first(self: &Arc<Self>, db: &Arc<dyn DatabaseStore>, ctx: &SendContext, generation_id: &str) -> FirstAttempt {
        let claimed = if self.is_stopping() { Ok(None) } else { db.claim_webhook_delivery_of(generation_id, &self.claimant, self.lease).await };
        match claimed {
            Ok(Some(delivery)) => {
                let Some(_held) = self.hold(&delivery.id) else { return FirstAttempt::LeftToTheOutbox };
                if self.attempt_unless_stopping(db, ctx, &delivery).await {
                    FirstAttempt::Sent
                } else {
                    FirstAttempt::LeftToTheOutbox
                }
            }
            Ok(None) => match db.webhook_outbox_of(generation_id).await {
                Ok(None) => FirstAttempt::Unrecorded,
                Ok(Some(_)) => FirstAttempt::LeftToTheOutbox,
                Err(e) => {
                    warn!(generation_id = %generation_id, error = %e, "could not read a terminal webhook's outbox delivery");
                    FirstAttempt::LeftToTheOutbox
                }
            },
            Err(e) => {
                warn!(generation_id = %generation_id, error = %e, "could not claim a terminal webhook's delivery; it stays due in the outbox, for the next tick");
                FirstAttempt::LeftToTheOutbox
            }
        }
    }

    /// A graceful stop: this dispatcher claims nothing more, and an attempt
    /// that gets its slots from now on sends nothing. The ones sending go on.
    pub fn stop(&self) {
        self.stopping.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether [`stop`](Self::stop) was called.
    pub fn is_stopping(&self) -> bool {
        self.stopping.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A stop, once this dispatcher's attempts have had their grace: give
    /// back this replica's leases on the deliveries it isn't sending, so the
    /// other replicas send those at their next tick. An attempt still
    /// sending keeps its lease until it ends (and writes its outcome) or the
    /// lease lapses with the process, so no other replica sends its delivery
    /// meanwhile (review 2026-11-15 round 2, finding 5).
    pub async fn release_claims(&self, db: &Arc<dyn DatabaseStore>) -> Result<ReleasedClaims, sqlx::Error> {
        let keep: Vec<String> = locked(&self.sending).iter().cloned().collect();
        let freed = db.release_webhook_claims(&self.claimant, &keep).await?;
        Ok(ReleasedClaims { freed, kept: keep.len() })
    }

    /// Run this dispatcher on its own task: a [`tick`](Self::tick) at once and
    /// every [`OUTBOX_TICK`], until `shutdown`, which [stops](Self::stop) it.
    pub fn spawn(
        self: Arc<Self>,
        db: Arc<dyn DatabaseStore>,
        ctx: SendContext,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut shutdown = std::pin::pin!(shutdown);
            let mut interval = tokio::time::interval(OUTBOX_TICK);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    // The stop first: a tick due at the same moment claims nothing more.
                    biased;
                    _ = &mut shutdown => {
                        self.stop();
                        info!("the webhook outbox's dispatcher stopped claiming");
                        return;
                    }
                    _ = interval.tick() => {
                        crate::supervise::guarded("a webhook outbox tick", self.tick(&db, &ctx)).await;
                    }
                }
            }
        })
    }

    /// One attempt of `d`, which this dispatcher has claimed. It is built now,
    /// from the generation and its key as they are, sent, recorded, and the
    /// delivery's next step written. A database error before the request
    /// leaves the claim to lapse, and the delivery is tried again then.
    async fn attempt(&self, db: &Arc<dyn DatabaseStore>, ctx: &SendContext, d: &crate::db::OutboxDelivery) {
        // A job's webhook answered 2xx already is done (review 2026-11-15
        // round 2, finding 3): during a rolling upgrade a replica from before
        // the outbox sends it from memory too, and the attempt of a replica
        // that died before writing its outcome needs no repeat. A redelivered
        // one is sent all the same: it was asked for.
        if !d.redelivered {
            match db.webhook_answered_for(&d.generation_id).await {
                Ok(true) => return self.already_answered(db, d).await,
                Ok(false) => {}
                Err(e) => return unavailable(d, &e),
            }
        }
        let gen = match db.get_generation(&d.generation_id).await {
            Ok(Some(gen)) => gen,
            Ok(None) => return self.give_up(db, d, "its generation no longer exists", None).await,
            Err(e) => return unavailable(d, &e),
        };
        let key = match uuid::Uuid::parse_str(&d.key_id) {
            Ok(key_id) => db.get_api_key(&key_id).await,
            Err(_) => Ok(None),
        };
        let key = match key {
            Ok(Some(key)) => key,
            Ok(None) => return self.give_up(db, d, "its key no longer exists", None).await,
            Err(e) => return unavailable(d, &e),
        };
        let Some(url) = key.webhook_url.clone().filter(|u| !u.is_empty()) else {
            return self.give_up(db, d, "its key has no webhook_url now", Some("")).await;
        };
        // Where it may go is checked now, for the key's owner now
        // (SEC-STWEBHOOK): only a key of the deployment's may reach its
        // network, and every other key's goes out on a public-only client.
        let inside = may_reach_inside(&**db, ctx.mode, ctx.internal, key.owner_user_id.as_deref()).await.unwrap_or_else(|e| {
            warn!(generation_id = %d.generation_id, delivery_id = %d.id, error = %e, "could not look up a webhook key's owner; holding it to public addresses");
            false
        });
        let client = if inside {
            ctx.inside_client.clone()
        } else {
            match crate::util::ssrf::check_public_url(&url).await {
                Ok(()) => crate::util::ssrf::public_only_client(),
                Err(crate::util::ssrf::UrlCheckError::Refused(reason)) => {
                    warn!(generation_id = %d.generation_id, delivery_id = %d.id, reason = %reason, "skipping webhook dispatch: disallowed url");
                    return self.give_up(db, d, &format!("webhook_url is not allowed: {reason}"), Some(&url)).await;
                }
                Err(crate::util::ssrf::UrlCheckError::Lookup(reason)) => {
                    let error = format!("could not look up the webhook's host: {reason}");
                    return self.attempted(db, d, &url, Answer::failed(error), None).await;
                }
            }
        };
        // The payload is built now: the row as its terminal write and
        // settlement left it, its files presented as this key may open them
        // (signed links in a `signed` app, the file route in a `private`
        // one), so every attempt's links are fresh.
        let mut gen = gen;
        if let Some(presenting) = &ctx.presenting {
            use crate::access::present::{FileOwner, Viewer};
            presenting.presenter(&FileOwner::of(&gen), Viewer::Owner).await.present_row(&mut gen).await;
        }
        let body = match serde_json::to_string(&gen) {
            Ok(body) => body,
            Err(e) => return self.give_up(db, d, &format!("serialise payload: {e}"), Some(&url)).await,
        };
        let signature = format!("sha256={}", compute_signature(&key.key_hash, &body));
        info!(generation_id = %d.generation_id, delivery_id = %d.id, url = %url, attempt = d.attempts + 1, "Dispatching webhook");
        let answer = match attempt_request(&client, &url, &signature, &d.id, body.clone()).send().await {
            Ok(resp) => Answer::of(resp).await,
            Err(e) => Answer::failed(e.to_string()),
        };
        self.attempted(db, d, &url, answer, Some(body)).await
    }

    /// Record attempt `d.attempts + 1` to `url` and what it got back
    /// ([`note_attempt`]), and write the delivery's next step, with the
    /// payload it sent.
    async fn attempted(&self, db: &Arc<dyn DatabaseStore>, d: &crate::db::OutboxDelivery, url: &str, answer: Answer, payload: Option<String>) {
        let number = d.attempts + 1;
        let sent = SentAttempt {
            key_id: &d.key_id,
            generation_id: &d.generation_id,
            delivery_id: &d.id,
            url,
            number,
            // Kept once, on the delivery (`finish_webhook_attempt`).
            payload_json: "",
        };
        note_attempt(Some(&**db), &sent, &answer).await;
        // A 410 Gone says the endpoint is gone for good: no retry. After any
        // other failed n-th attempt the next waits the n-th wait; after the
        // last, the delivery has failed.
        let outcome = if answer.success {
            crate::db::OutboxOutcome::Delivered
        } else if answer.is_gone() {
            crate::db::OutboxOutcome::Failed
        } else {
            match self.schedule.get(number as usize - 1) {
                Some(wait) => crate::db::OutboxOutcome::Retry { after: chrono::Duration::from_std(*wait).unwrap_or_else(|_| chrono::Duration::hours(8)) },
                None => crate::db::OutboxOutcome::Failed,
            }
        };
        if answer.is_gone() {
            warn!(generation_id = %d.generation_id, delivery_id = %d.id, url = %url, "Webhook delivery failed: the endpoint answered 410 Gone, so it is not retried");
        } else if outcome == crate::db::OutboxOutcome::Failed {
            warn!(generation_id = %d.generation_id, delivery_id = %d.id, url = %url, attempts = number, "Webhook delivery failed after all retries");
        }
        let attempt = crate::db::OutboxAttempt {
            attempts: number,
            outcome,
            status_code: answer.status.map(i32::from),
            error: answer.error.as_deref(),
            payload_json: payload.as_deref(),
        };
        self.finish(db, d, &attempt).await;
    }

    /// End `d` delivered without a request: an attempt at its job's webhook
    /// was answered 2xx already.
    async fn already_answered(&self, db: &Arc<dyn DatabaseStore>, d: &crate::db::OutboxDelivery) {
        info!(generation_id = %d.generation_id, delivery_id = %d.id, "Webhook not sent again: an attempt at this job's webhook was already answered 2xx");
        let attempt = crate::db::OutboxAttempt {
            attempts: d.attempts,
            outcome: crate::db::OutboxOutcome::Delivered,
            status_code: None,
            error: Some("not sent again: an attempt at this job's webhook was answered 2xx already (from memory, by a replica from before migration 029, or an attempt whose outcome wasn't written)"),
            payload_json: None,
        };
        self.finish(db, d, &attempt).await;
    }

    /// End `d` failed without a request: it can't be sent. `listed_as` is
    /// the key's webhook_url (`""` for none) while the key exists: the reason
    /// is then an attempt row of the key's list ([`record_attempt`]), under
    /// the delivery's id, with no status and the reason as its error, so the
    /// key's managers see why the webhook never came and can redeliver it
    /// once the cause is fixed. A delivery whose generation or key is gone
    /// (`None`) writes no row: nothing would list it, and a row could trip a
    /// foreign key.
    async fn give_up(&self, db: &Arc<dyn DatabaseStore>, d: &crate::db::OutboxDelivery, why: &str, listed_as: Option<&str>) {
        warn!(generation_id = %d.generation_id, delivery_id = %d.id, reason = why, "Webhook delivery failed: it can't be sent");
        if let Some(url) = listed_as {
            let sent = SentAttempt { key_id: &d.key_id, generation_id: &d.generation_id, delivery_id: &d.id, url, number: d.attempts + 1, payload_json: "" };
            record_attempt(&**db, &sent, &Answer::failed(why.to_string())).await;
        }
        let attempt = crate::db::OutboxAttempt {
            attempts: d.attempts,
            outcome: crate::db::OutboxOutcome::Failed,
            status_code: None,
            error: Some(why),
            payload_json: None,
        };
        self.finish(db, d, &attempt).await;
    }

    /// Write what `attempt` decided for `d`, while this dispatcher holds it.
    async fn finish(&self, db: &Arc<dyn DatabaseStore>, d: &crate::db::OutboxDelivery, attempt: &crate::db::OutboxAttempt<'_>) {
        match db.finish_webhook_attempt(&d.id, &self.claimant, attempt).await {
            Ok(true) => {}
            Ok(false) => info!(generation_id = %d.generation_id, delivery_id = %d.id, "another dispatcher took this webhook delivery after this one's claim lapsed; its attempt decides"),
            Err(e) => warn!(generation_id = %d.generation_id, delivery_id = %d.id, error = %e, "could not record a webhook attempt's outcome; the delivery is tried again once its claim lapses"),
        }
    }
}

/// What an attempt got back: an answer's status (and the start of its
/// reply), or the error that kept it from one.
struct Answer {
    status: Option<u16>,
    success: bool,
    reply: Option<String>,
    error: Option<String>,
}

impl Answer {
    /// No answer: `error` kept the attempt from one.
    fn failed(error: String) -> Self {
        Self { status: None, success: false, reply: None, error: Some(error) }
    }

    /// What `resp` answered: its status, and the start of its reply
    /// ([`kept_reply`]).
    async fn of(resp: reqwest::Response) -> Self {
        let status = resp.status();
        Self { status: Some(status.as_u16()), success: status.is_success(), reply: kept_reply(resp).await, error: None }
    }

    /// A 410 Gone: the endpoint is gone for good, so the delivery ends.
    fn is_gone(&self) -> bool {
        self.status == Some(410)
    }
}

/// A database error before an attempt's request: nothing is sent, and the
/// claim is left to lapse, so the delivery is tried again then.
fn unavailable(d: &crate::db::OutboxDelivery, e: &sqlx::Error) {
    warn!(generation_id = %d.generation_id, delivery_id = %d.id, error = %e, "could not read a webhook delivery's generation or key; it is tried again once its claim lapses");
}

/// This replica's dispatcher of the outbox: it claims as this replica, and
/// sends on this process's deliveries ([`deliveries`]).
pub fn outbox() -> &'static Arc<Outbox> {
    static OUTBOX: std::sync::OnceLock<Arc<Outbox>> = std::sync::OnceLock::new();
    OUTBOX.get_or_init(|| Arc::new(Outbox::new(crate::shared::replica_id(), deliveries().clone(), WEBHOOK_RETRY_DELAYS.to_vec())))
}

/// Whether the webhook of a key owned by `owner` may reach the deployment's
/// own network: private, loopback and link-local addresses (review 2026-11-11
/// SEC-STWEBHOOK). Checked when a key's webhook is saved, tested and sent.
///
/// Hosted: never, a tenant's webhook goes to public addresses only.
/// Single-tenant, with `webhooks.internal_targets: everyone`: any key's.
/// Otherwise (the default) only the deployment's own: a key with no owner
/// (made with the master key), or one whose owner is active and holds
/// `system:config` or `key:write:any` in the deployment's org. A role in
/// another org counts for nothing: a side org's owner holds no authority over
/// the deployment's network, and a demoted or removed Admin's keys lose it.
pub async fn may_reach_inside(
    db: &dyn DatabaseStore,
    mode: crate::config::Mode,
    internal: crate::config::InternalTargets,
    owner: Option<&str>,
) -> Result<bool, sqlx::Error> {
    if mode == crate::config::Mode::Hosted {
        return Ok(false);
    }
    if internal == crate::config::InternalTargets::Everyone {
        return Ok(true);
    }
    let Some(user_id) = owner else { return Ok(true) };
    let role = match db.get_user_by_id(user_id).await? {
        Some(user) if user.is_active => db.get_membership(crate::api::middleware::DEFAULT_ORG_ID, user_id).await?,
        _ => None,
    };
    use crate::auth::permissions::{role_has, Permission};
    Ok(role.is_some_and(|r| role_has(r, Permission::SystemConfig) || role_has(r, Permission::KeyWriteAny)))
}

/// A stored key whose webhook deliveries are refused: what the startup notice
/// names (FU-STWEBHOOK). Never the URL itself, which may carry a secret in its
/// path or query: only its host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedKeyWebhook {
    pub key_id: uuid::Uuid,
    pub key_prefix: String,
    pub owner: Option<String>,
    pub host: String,
}

/// How long the startup notice waits for one webhook host's name to resolve.
const NOTICE_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The stored keys whose webhook deliveries are refused since SEC-STWEBHOOK,
/// found by the check dispatch makes (`dispatch_terminal_webhook`): the key may
/// not reach inside the deployment ([`may_reach_inside`], an owner lookup that
/// fails counting as no, as there) and its URL is no public address
/// (`util::ssrf::check_public_url`). Single-tenant only: hosted, nothing
/// changed, as a tenant's webhook was always held to public addresses, at save
/// and at dispatch. A revoked key starts no job, so it isn't named. A host
/// whose name doesn't resolve now, or not within 5 s, says nothing either way
/// and isn't named.
pub async fn refused_key_webhooks(
    db: &dyn DatabaseStore,
    mode: crate::config::Mode,
    internal: crate::config::InternalTargets,
) -> Result<Vec<RefusedKeyWebhook>, sqlx::Error> {
    if mode == crate::config::Mode::Hosted || internal == crate::config::InternalTargets::Everyone {
        return Ok(Vec::new());
    }
    let mut refused = Vec::new();
    for key in db.list_api_keys().await? {
        let Some(url) = key.webhook_url.as_deref().filter(|_| key.is_active) else { continue };
        if may_reach_inside(db, mode, internal, key.owner_user_id.as_deref()).await.unwrap_or(false) {
            continue;
        }
        let checked = tokio::time::timeout(NOTICE_LOOKUP_TIMEOUT, crate::util::ssrf::check_public_url(url)).await;
        if let Ok(Err(crate::util::ssrf::UrlCheckError::Refused(_))) = checked {
            let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| "-".to_string());
            refused.push(RefusedKeyWebhook { key_id: key.id, key_prefix: key.key_prefix, owner: key.owner_user_id, host });
        }
    }
    Ok(refused)
}

/// Name, in the log, each stored key whose webhook deliveries are refused
/// ([`refused_key_webhooks`]): one warning per key, with its id and prefix,
/// its owner and the URL's host (never the URL, whose path or query may hold
/// a secret), and one that sums them up. Nothing when no key is named. So an
/// operator upgrading past SEC-STWEBHOOK sees which keys lost their internal
/// webhook (docs/management-api.md, "Key webhooks").
pub async fn name_refused_key_webhooks(db: &dyn DatabaseStore, mode: crate::config::Mode, internal: crate::config::InternalTargets) {
    let refused = match refused_key_webhooks(db, mode, internal).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "could not list the stored keys to check their webhooks");
            return;
        }
    };
    if refused.is_empty() {
        return;
    }
    for k in &refused {
        warn!(
            key_id = %k.key_id,
            key_prefix = %k.key_prefix,
            owner = k.owner.as_deref().unwrap_or("-"),
            host = %k.host,
            "SECURITY: this key's webhook goes to an address inside the deployment, which only the deployment's own keys and \
             keys of its Owners and Admins may reach; its deliveries are refused"
        );
    }
    warn!(
        refused_keys = refused.len(),
        "SECURITY: stored keys whose webhook goes inside the deployment get no deliveries (each named above): give each a public \
         webhook URL, have an Owner or Admin of the deployment's org make it, or set webhooks.internal_targets: everyone \
         (docs/management-api.md, Key webhooks)"
    );
}

/// [`name_refused_key_webhooks`] on a task of its own, so startup never waits
/// for it (its name lookups are bounded too).
pub fn spawn_refused_key_webhook_notice(db: Arc<dyn DatabaseStore>, mode: crate::config::Mode, internal: crate::config::InternalTargets) {
    tokio::spawn(async move { name_refused_key_webhooks(&*db, mode, internal).await });
}

/// The most of an endpoint's reply a delivery reads and keeps: its first 16 KiB
/// (review 2026-11-11 SEC-WHBODY). The status, known before the body, decides
/// the delivery, so the rest is never read.
pub const WEBHOOK_REPLY_KEPT_BYTES: usize = 16 * 1024;

/// Reads at most [`WEBHOOK_REPLY_KEPT_BYTES`] of `resp`'s body, then drops the
/// response, which closes the connection: a huge or endless reply costs neither
/// memory nor storage. A multi-byte character cut by the limit is dropped whole,
/// so the text never grows past it. `None` when nothing could be read.
async fn kept_reply(mut resp: reqwest::Response) -> Option<String> {
    let mut kept: Vec<u8> = Vec::new();
    while kept.len() < WEBHOOK_REPLY_KEPT_BYTES {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let room = WEBHOOK_REPLY_KEPT_BYTES - kept.len();
                kept.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            Ok(None) => break,
            Err(_) if kept.is_empty() => return None,
            Err(_) => break,
        }
    }
    drop_a_cut_character(&mut kept);
    let text = String::from_utf8_lossy(&kept);
    // Bytes that are not UTF-8 read as U+FFFD, as `Response::text` gives them,
    // which can lengthen the text; it still stops at the limit.
    let mut end = text.len().min(WEBHOOK_REPLY_KEPT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(text[..end].to_string())
}

/// Drops an incomplete UTF-8 sequence at the end of `bytes`: the character the
/// read limit cut.
fn drop_a_cut_character(bytes: &mut Vec<u8>) {
    let len = bytes.len();
    for back in 1..=len.min(3) {
        let b = bytes[len - back];
        if b & 0xC0 != 0x80 {
            // The last lead byte (or ASCII): how long its character is.
            let need = match b {
                0xF0.. => 4,
                0xE0.. => 3,
                0xC0.. => 2,
                _ => 1,
            };
            if need > back {
                bytes.truncate(len - back);
            }
            return;
        }
    }
}

fn compute_signature(secret: &str, body: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(body.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::ssrf::refused_by_the_guard;
    use crate::types::{Generation, GenerationStatus};
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_generation(id: &str) -> Generation {
        Generation {
            id: id.to_string(),
            key_id: None,
            model: "mock/video-gen".to_string(),
            provider: "mock".to_string(),
            media_type: "video".to_string(),
            status: GenerationStatus::Completed,
            progress: 100,
            provider_job_id: Some("job-1".to_string()),
            result_url: Some("https://example.com/video.mp4".to_string()),
            error_message: None,
            cost_usd: 0.0,
            created_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            metadata: None,
            org_id: None,
            app_id: None,
            files_deleted_at: None,
            files_deleted_reason: None,
        }
    }

    #[tokio::test]
    async fn delivers_webhook_with_signature_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .and(header_exists("x-litegen-signature"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let gen = make_generation("litegen-vid-sig-test");
        let url = format!("{}/hook", server.uri());

        let result = dispatch_webhook(&client, &url, Some("test-secret"), &gen).await;
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        server.verify().await;
    }

    #[tokio::test]
    async fn retries_on_503_then_succeeds() {
        let server = MockServer::start().await;
        // Use up_to(2) + priority ordering to simulate 2x 503 then 200.
        // wiremock processes highest-priority last, so put the 503 last to avoid
        // it consuming the third request. Use up_to to bound the 503 mock.
        Mock::given(method("POST"))
            .and(path("/retry-hook"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/retry-hook"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let gen = make_generation("litegen-vid-retry-test");
        let url = format!("{}/retry-hook", server.uri());

        let result = deliver(&client, &url, None, &gen, None, at_once).await;
        assert!(result.is_ok(), "should succeed on third attempt: {:?}", result);
    }

    // ─── retries over minutes, and the delivery id (leftovers 2026-11-14) ───

    /// The back-off of a test that waits for nothing: each retry goes at once.
    fn at_once(_: std::time::Duration) -> std::future::Ready<()> {
        std::future::ready(())
    }

    /// A failing receiver gets seven attempts: the first at once, each later
    /// one after the wait the docs give (10 s, 30 s, 1 min, 2 min, 5 min,
    /// 10 min), and then no more: the delivery has failed.
    #[tokio::test]
    async fn a_failing_delivery_is_tried_seven_times_on_the_documented_schedule() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/down")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
        let url = format!("{}/down", server.uri());
        // Each wait asked for, in seconds, with how many attempts were sent by then.
        let waits: Arc<std::sync::Mutex<Vec<(u64, usize)>>> = Default::default();
        let receiver = &server;
        let wait = |delay: std::time::Duration| {
            let waits = waits.clone();
            async move {
                let sent = arrivals(receiver, "/down").await;
                waits.lock().unwrap().push((delay.as_secs(), sent));
            }
        };

        let result = deliver(&reqwest::Client::new(), &url, None, &make_generation("litegen-vid-schedule"), None, wait).await;
        assert!(result.is_err(), "every attempt was answered 503: {result:?}");
        assert_eq!(arrivals(&server, "/down").await, 7, "seven attempts in all");
        let waited = waits.lock().unwrap().clone();
        assert_eq!(waited, vec![(10, 1), (30, 2), (60, 3), (120, 4), (300, 5), (600, 6)], "(seconds waited, attempts sent by then)");
    }

    /// A receiver that answers its `n`-th request as `script[n]` says: `None`
    /// hangs up without an answer (a network error for the sender), a status
    /// answers with it, and every request past the script gets 200. Its URL,
    /// and how many requests it has had.
    async fn scripted_receiver(script: Vec<Option<u16>>) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let had = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = had.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let answer = script.get(n).copied().unwrap_or(Some(200));
                // The whole request is read first, so a hang-up is a clean close.
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                while let Ok(read @ 1..) = sock.read(&mut buf).await {
                    req.extend_from_slice(&buf[..read]);
                    let text = String::from_utf8_lossy(&req).to_ascii_lowercase();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if req.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                if let Some(status) = answer {
                    let reply = format!("HTTP/1.1 {status} Scripted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                    let _ = sock.write_all(reply.as_bytes()).await;
                }
            }
        });
        (format!("http://{addr}/hook"), had)
    }

    /// Whatever goes wrong is retried, a hang-up as much as a 5xx, a 4xx or a
    /// 429: a receiver whose first five attempts fail gets the sixth.
    #[tokio::test]
    async fn a_receiver_that_fails_its_first_five_attempts_gets_the_sixth() {
        let (url, had) = scripted_receiver(vec![None, None, Some(500), Some(404), Some(429)]).await;
        let result = deliver(&reqwest::Client::new(), &url, None, &make_generation("litegen-vid-late"), None, at_once).await;
        assert!(result.is_ok(), "delivered at the sixth attempt: {result:?}");
        assert_eq!(had.load(std::sync::atomic::Ordering::SeqCst), 6, "five failed attempts, then the one answered 200");
    }

    /// A delivery sent from memory (where no outbox delivery was recorded)
    /// stops at a 410 Gone, as an outbox delivery does (review 2026-11-15
    /// round 2, finding 8): one attempt, recorded, and the log says why.
    #[tokio::test]
    async fn a_delivery_sent_from_memory_stops_at_a_410() {
        use crate::db::sqlite::SqliteDatabase;
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/gone")).respond_with(ResponseTemplate::new(410)).mount(&server).await;
        let db: Arc<dyn DatabaseStore> = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let url = format!("{}/gone", server.uri());
        let mut result = Ok(());
        let warned = warnings_of(async {
            let log = AttemptLog { db: &*db, key_id: "key-gone" };
            result = deliver(&reqwest::Client::new(), &url, None, &make_generation("litegen-vid-gone"), Some(log), at_once).await;
        })
        .await;

        assert!(result.as_ref().is_err_and(|e| e.contains("410")), "{result:?}");
        assert_eq!(arrivals(&server, "/gone").await, 1, "a 410 is not retried");
        let (rows, total) = db.list_webhook_deliveries("key-gone", 1, 50).await.unwrap();
        assert_eq!((total, rows[0].status_code, rows[0].success), (1, Some(410), false), "the one attempt is recorded");
        assert!(warned.iter().any(|l| l.contains("410 Gone, so it is not retried") && l.contains("litegen-vid-gone")), "{warned:#?}");
    }

    /// Whether `id` is a delivery id: `whd-` and a UUID.
    fn is_delivery_id(id: &str) -> bool {
        id.strip_prefix("whd-").is_some_and(|u| uuid::Uuid::parse_str(u).is_ok())
    }

    /// The `X-Litegen-Delivery-Id` of each request `server` has had, in
    /// order ("" for one without it).
    async fn delivery_ids(server: &MockServer) -> Vec<String> {
        let requests = server.received_requests().await.unwrap_or_default();
        requests
            .iter()
            .map(|r| r.headers.get("x-litegen-delivery-id").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string())
            .collect()
    }

    /// Every attempt of one delivery carries the same delivery id, so a
    /// receiver can drop a repeat, and the next delivery has another.
    #[tokio::test]
    async fn every_attempt_of_a_delivery_carries_its_id_and_the_next_delivery_has_another() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let url = format!("{}/hook", server.uri());
        let client = reqwest::Client::new();
        for id in ["litegen-vid-id-1", "litegen-vid-id-2"] {
            let result = deliver(&client, &url, None, &make_generation(id), None, at_once).await;
            assert!(result.is_ok(), "{result:?}");
        }

        let ids = delivery_ids(&server).await;
        assert_eq!(ids.len(), 3, "the first delivery's two attempts, then the second's one: {ids:?}");
        assert!(ids.iter().all(|id| is_delivery_id(id)), "each request carries a delivery id, whd- and a UUID: {ids:?}");
        assert_eq!(ids[0], ids[1], "a retry carries its delivery's id");
        assert_ne!(ids[1], ids[2], "the next delivery has an id of its own");
    }

    /// A test webhook (`POST /v1/keys/{id}/test-webhook`) is a delivery of
    /// its own: it carries a delivery id, a new one each time.
    #[tokio::test]
    async fn each_test_webhook_carries_a_delivery_id_of_its_own() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let url = format!("{}/hook", server.uri());
        let client = reqwest::Client::new();
        for _ in 0..2 {
            let sent = dispatch_webhook_once(&client, &url, None, &make_generation("webhook-test-1")).await;
            assert_eq!(sent.expect("answered").status(), 200);
        }

        let ids = delivery_ids(&server).await;
        assert!(ids.len() == 2 && ids.iter().all(|id| is_delivery_id(id)), "each test carries a delivery id: {ids:?}");
        assert_ne!(ids[0], ids[1], "each test is a delivery of its own");
    }

    /// Each log line of a delivery names its delivery id, the one the
    /// receiver got, so the two sides' logs can be matched.
    #[tokio::test]
    async fn a_deliverys_log_lines_name_its_delivery_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let url = format!("{}/hook", server.uri());
        let lines = log_lines_of(async {
            let result = deliver(&reqwest::Client::new(), &url, None, &make_generation("litegen-vid-logged-id"), None, at_once).await;
            assert!(result.is_ok(), "{result:?}");
        })
        .await;

        let id = delivery_ids(&server).await.into_iter().next().unwrap_or_default();
        let ours: Vec<&String> = lines.iter().filter(|l| l.contains("litegen-vid-logged-id")).collect();
        assert_eq!(ours.len(), 4, "dispatching, the 503, dispatching again, delivered: {lines:#?}");
        let named = format!("delivery_id={id}");
        assert!(is_delivery_id(&id) && ours.iter().all(|l| l.contains(&named)), "each line names delivery {id:?}: {ours:#?}");
    }

    /// A delivery waiting for its retry holds no slot: with one slot in all
    /// and one per org, org a's delivery fails its first attempt and waits,
    /// and meanwhile org a's next delivery and org b's go out. Its retry goes
    /// once its wait is over.
    #[tokio::test]
    async fn a_delivery_waiting_for_its_retry_holds_no_slot() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/a-first")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let deliveries = Arc::new(WebhookDeliveries::new(1, 1));
        let wait_over = Arc::new(tokio::sync::Notify::new());
        let (url, over) = (format!("{}/a-first", server.uri()), wait_over.clone());
        deliveries.spawn("a", "a-first", async move {
            let wait = move |_: std::time::Duration| {
                let over = over.clone();
                async move { over.notified().await }
            };
            let _ = deliver(&reqwest::Client::new(), &url, None, &make_generation("a-first"), None, wait).await;
        });
        until_arrivals(&server, "/a-first", 1).await;

        let send = |org: &str, at: &str| {
            let url = format!("{}/{at}", server.uri());
            deliveries.spawn(org, at, async move {
                let _ = reqwest::Client::new().post(url).send().await;
            });
        };
        send("a", "a-next");
        until_arrivals(&server, "/a-next", 1).await; // org a's one slot was free
        send("b", "b-first");
        until_arrivals(&server, "/b-first", 1).await; // and so was the one slot of all
        wait_over.notify_one();
        until_arrivals(&server, "/a-first", 2).await;
        let left = deliveries.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(10)).await;
        assert!(left.is_empty(), "every delivery ended: {left:?}");
    }

    /// A retry waits for its slots like any attempt: while another delivery
    /// holds org a's one slot (then, in the second round, the one slot of
    /// all), org a's delivery whose wait is over doesn't send its retry. It
    /// sends it once that delivery ends.
    #[tokio::test]
    async fn a_retry_waits_for_its_slots_like_any_attempt() {
        use tokio::sync::Notify;
        for holder_org in ["a", "b"] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
            Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
            let deliveries = Arc::new(WebhookDeliveries::new(1, 1));
            let (wait_over, holding, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()), Arc::new(Notify::new()));
            let (url, over) = (format!("{}/a-first", server.uri()), wait_over.clone());
            deliveries.spawn("a", "a-first", async move {
                let wait = move |_: std::time::Duration| {
                    let over = over.clone();
                    async move { over.notified().await }
                };
                let _ = deliver(&reqwest::Client::new(), &url, None, &make_generation("a-first"), None, wait).await;
            });
            until_arrivals(&server, "/a-first", 1).await;
            // Another delivery takes the slots a-first gave back, and keeps them.
            let (has, go) = (holding.clone(), release.clone());
            deliveries.spawn(holder_org, "holder", async move {
                has.notify_one();
                go.notified().await;
            });
            tokio::time::timeout(std::time::Duration::from_secs(10), holding.notified()).await.expect("the holder never got the slots");

            wait_over.notify_one();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            assert_eq!(arrivals(&server, "/a-first").await, 1, "the retry went while org {holder_org}'s delivery held a slot it needs");
            release.notify_one();
            until_arrivals(&server, "/a-first", 2).await;
            let left = deliveries.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(10)).await;
            assert!(left.is_empty(), "every delivery ended: {left:?}");
        }
    }

    // ─── Webhook delivery log tests ───────────────────────────────────────────

    #[tokio::test]
    async fn dispatch_logged_records_successful_delivery() {
        use crate::db::sqlite::SqliteDatabase;
        use crate::db::DatabaseStore;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/logged-hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let db: Arc<dyn DatabaseStore> =
            Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let client = reqwest::Client::new();
        let gen = make_generation("litegen-vid-log-ok");
        let url = format!("{}/logged-hook", server.uri());

        let result = dispatch_webhook_logged(&client, &url, None, &gen, db.clone(), "key-123").await;
        assert!(result.is_ok(), "expected Ok: {:?}", result);
        server.verify().await;

        // Assert 1 row inserted with success=true
        let (deliveries, total) = db
            .list_webhook_deliveries("key-123", 1, 50)
            .await
            .unwrap();
        assert_eq!(total, 1, "expected 1 delivery row");
        assert!(deliveries[0].success, "delivery should be marked successful");
        assert_eq!(deliveries[0].status_code, Some(200));
        assert_eq!(deliveries[0].attempt_number, 1);
    }

    #[tokio::test]
    async fn dispatch_logged_records_retry_attempts() {
        use crate::db::sqlite::SqliteDatabase;
        use crate::db::DatabaseStore;

        let server = MockServer::start().await;
        // 3 failures then 1 success
        Mock::given(method("POST"))
            .and(path("/logged-retry"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/logged-retry"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let db: Arc<dyn DatabaseStore> =
            Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let client = reqwest::Client::new();
        let gen = make_generation("litegen-vid-log-retry");
        let url = format!("{}/logged-retry", server.uri());

        let log = AttemptLog { db: &*db, key_id: "key-retry" };
        let result = deliver(&client, &url, None, &gen, Some(log), at_once).await;
        assert!(result.is_ok(), "expected Ok after 3 attempts: {:?}", result);

        // Assert 3 rows: 2 with success=false status_code=503, 1 with success=true status_code=200
        let (deliveries, total) = db
            .list_webhook_deliveries("key-retry", 1, 50)
            .await
            .unwrap();
        assert_eq!(total, 3, "expected 3 delivery rows (2 failures + 1 success)");

        // Sort by attempt number (list returns DESC by created_at, so reverse)
        let mut deliveries = deliveries;
        deliveries.sort_by_key(|d| d.attempt_number);

        assert!(!deliveries[0].success, "attempt 1 should be failed");
        assert_eq!(deliveries[0].status_code, Some(503));
        assert!(!deliveries[1].success, "attempt 2 should be failed");
        assert_eq!(deliveries[1].status_code, Some(503));
        assert!(deliveries[2].success, "attempt 3 should succeed");
        assert_eq!(deliveries[2].status_code, Some(200));
    }

    // ─── the endpoint's reply: only its start is read and kept (SEC-WHBODY) ──

    /// One delivery to `url` through `client`, logged in a fresh database; returns
    /// the result and the rows it recorded.
    async fn deliver_logged(client: &reqwest::Client, url: &str) -> (Result<(), String>, Vec<WebhookDelivery>) {
        use crate::db::sqlite::SqliteDatabase;
        let db: Arc<dyn DatabaseStore> = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let gen = make_generation("litegen-vid-reply");
        let result = dispatch_webhook_logged(client, url, None, &gen, db.clone(), "key-reply").await;
        let (rows, _) = db.list_webhook_deliveries("key-reply", 1, 50).await.unwrap();
        (result, rows)
    }

    #[tokio::test]
    async fn dispatch_logged_keeps_only_the_start_of_a_large_reply() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/big-reply"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(1 << 20)))
            .mount(&server)
            .await;
        let (result, rows) = deliver_logged(&reqwest::Client::new(), &format!("{}/big-reply", server.uri())).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(rows.len(), 1);
        let kept = rows[0].response_body.as_deref().expect("the reply's start is kept");
        assert!(kept.len() <= 16 * 1024, "a delivery keeps at most the first 16 KiB of the reply, kept {} bytes", kept.len());
        assert!(!kept.is_empty() && kept.bytes().all(|b| b == b'x'), "what is kept is the reply's own start");
    }

    /// A host that answers any request `200` with a chunked body: `total` bytes,
    /// then nothing for two minutes with the connection held open.
    async fn stalling_reply_host(total: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut req = [0u8; 8192];
                    let _ = sock.read(&mut req).await;
                    let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n").await;
                    let chunk = vec![b'y'; 64 * 1024];
                    for _ in 0..total / chunk.len() {
                        if sock.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).await.is_err()
                            || sock.write_all(&chunk).await.is_err()
                            || sock.write_all(b"\r\n").await.is_err()
                        {
                            return;
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                });
            }
        });
        format!("http://{addr}/hook")
    }

    #[tokio::test]
    async fn dispatch_logged_stops_reading_an_endless_reply() {
        // About 1 MiB, then a stall: a reply that never ends. A client with no
        // timeout of its own shows that the delivery stops reading by itself.
        let url = stalling_reply_host(1024 * 1024).await;
        let delivered = tokio::time::timeout(std::time::Duration::from_secs(10), deliver_logged(&reqwest::Client::new(), &url))
            .await
            .expect("kept reading an endless reply");
        let (result, rows) = delivered;
        assert!(result.is_ok(), "the status decides the delivery: {result:?}");
        assert_eq!(rows.len(), 1, "one attempt, recorded");
        let kept = rows[0].response_body.as_deref().unwrap_or_default();
        assert!(kept.len() <= 16 * 1024 && kept.starts_with('y'), "kept {} bytes", kept.len());
    }

    #[tokio::test]
    async fn a_kept_reply_is_cut_at_a_character_boundary() {
        // 3-byte characters: 16 KiB falls inside one, which is dropped whole
        // rather than replaced by U+FFFD (which would push past 16 KiB).
        let sent = "€".repeat(100_000);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/euro-reply"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sent.clone()))
            .mount(&server)
            .await;
        let (_, rows) = deliver_logged(&reqwest::Client::new(), &format!("{}/euro-reply", server.uri())).await;
        let kept = rows[0].response_body.as_deref().expect("kept");
        assert!(kept.len() <= 16 * 1024, "at most 16 KiB, kept {} bytes", kept.len());
        assert!(sent.starts_with(kept) && !kept.contains('\u{FFFD}'), "a prefix of the reply, whole characters only");
        assert_eq!(kept.len(), (16 * 1024 / 3) * 3, "every whole character that fits is kept");
    }

    #[tokio::test]
    async fn a_hosted_webhook_client_reaches_only_public_addresses() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let by_name = format!("http://localhost:{}/hook", server.address().port());

        let hosted = webhook_client(crate::config::Mode::Hosted);
        let err = hosted.post(&by_name).send().await.expect_err("a tenant's webhook can't reach inside the deployment");
        assert!(refused_by_the_guard(&err), "{err:?}");
        // The single-tenant client connects anywhere: it is the one for the
        // keys that may reach inside (`may_reach_inside`).
        let single = webhook_client(crate::config::Mode::SingleTenant);
        assert_eq!(single.post(&by_name).send().await.expect("reachable").status(), 200);
    }

    // ─── the process's deliveries: bounded, and drained at a stop (P13) ──────

    /// How many requests `server` has had at `at`. wiremock records a
    /// request when it arrives, before any `set_delay` answer.
    async fn arrivals(server: &MockServer, at: &str) -> usize {
        server.received_requests().await.unwrap_or_default().iter().filter(|r| r.url.path() == at).count()
    }

    /// Waits (10 s at most) until `server` has had `n` requests at `at`.
    async fn until_arrivals(server: &MockServer, at: &str, n: usize) {
        let wait = async {
            while arrivals(server, at).await < n {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), wait)
            .await
            .unwrap_or_else(|_| panic!("{n} requests never arrived at {at}"));
    }

    /// A stop waits for a webhook in its back-off: the receiver's 503 is
    /// retried before the drain returns, not dropped with the process. The
    /// back-off here is a real second, not the schedule's 10 s.
    #[tokio::test]
    async fn a_drain_waits_for_a_delivery_in_its_back_off() {
        use crate::db::sqlite::SqliteDatabase;
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/drain-hook")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
        Mock::given(method("POST")).and(path("/drain-hook")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let db: Arc<dyn DatabaseStore> = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let deliveries = Arc::new(WebhookDeliveries::new(8, 4));
        let url = format!("{}/drain-hook", server.uri());
        deliveries.spawn("org-a", "litegen-vid-drain", async move {
            let gen = make_generation("litegen-vid-drain");
            let log = AttemptLog { db: &*db, key_id: "key-drain" };
            let a_second = |_: std::time::Duration| tokio::time::sleep(std::time::Duration::from_secs(1));
            let _ = deliver(&reqwest::Client::new(), &url, None, &gen, Some(log), a_second).await;
        });
        until_arrivals(&server, "/drain-hook", 1).await;

        let left = deliveries.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(10)).await;
        assert_eq!(arrivals(&server, "/drain-hook").await, 2, "the drain returned before the retry was sent");
        assert!(left.is_empty(), "nothing was left running: {left:?}");
    }

    /// One org's slow receiver holds at most its share of the deliveries,
    /// and the deliveries in all are bounded too: with 2 per org and 3 in all,
    /// org a's five deliveries go two at a time, org b's goes at once, and
    /// org c's waits for a slot. Each answer takes 3 s.
    #[tokio::test]
    async fn one_orgs_slow_receiver_holds_at_most_its_share_open() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(3)))
            .mount(&server)
            .await;
        let deliveries = Arc::new(WebhookDeliveries::new(3, 2));
        let client = reqwest::Client::new();
        let send = |org: &'static str, i: usize| {
            let (client, url) = (client.clone(), format!("{}/{org}", server.uri()));
            deliveries.spawn(org, &format!("{org}-{i}"), async move {
                let _ = client.post(url).send().await;
            });
        };
        // In order, so task scheduling can't reorder who takes the slots.
        for i in 0..5 {
            send("a", i);
        }
        until_arrivals(&server, "/a", 2).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(arrivals(&server, "/a").await, 2, "org a has at most its 2 deliveries open at once");
        send("b", 0);
        until_arrivals(&server, "/b", 1).await; // another org is not held up behind a's queue
        send("c", 0);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(arrivals(&server, "/c").await, 0, "a's 2 and b's 1 hold all 3 slots");

        let left = deliveries.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(30)).await;
        assert!(left.is_empty(), "every delivery ended within the drain: {left:?}");
        let all = (arrivals(&server, "/a").await, arrivals(&server, "/b").await, arrivals(&server, "/c").await);
        assert_eq!(all, (5, 1, 1), "every delivery was sent");
    }

    // ─── the startup notice of refused key webhooks (FU-STWEBHOOK) ───────────

    /// A person holding `role` in the deployment's org: their id.
    async fn person(db: &crate::db::sqlite::SqliteDatabase, role: crate::types::Role) -> String {
        let now = chrono::Utc::now();
        let id = format!("u-{}", uuid::Uuid::new_v4());
        let user = crate::types::User {
            id: id.clone(),
            email: format!("{id}@example.com"),
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
        db.create_user(&user).await.unwrap();
        db.add_org_member(crate::api::middleware::DEFAULT_ORG_ID, &id, role).await.unwrap();
        id
    }

    /// A key called `name` whose webhook is `url`, owned by `owner` (none: made
    /// with the master key).
    async fn hooked_key(db: &crate::db::sqlite::SqliteDatabase, name: &str, owner: Option<&str>, url: &str) -> crate::types::ApiKey {
        let key = db.create_api_key(name, &format!("hash-{name}"), &format!("lg-{name}"), None, None, "generate,read", Some(url)).await.unwrap();
        if let Some(owner) = owner {
            db.set_api_key_owner(&key.id, owner).await.unwrap();
        }
        key
    }

    /// FU-STWEBHOOK (follow-ups 2026-11-13): litegen names the stored keys
    /// whose webhook deliveries are refused, by the check dispatch makes
    /// (`dispatch_terminal_webhook`): a key that may not reach inside the
    /// deployment (`may_reach_inside`) whose URL is no public address. A
    /// Member's key with an internal webhook is named, by IP or by a name that
    /// resolves inside; an Admin's key, a master-key key and a public URL are
    /// not. With `internal_targets: everyone` nothing is refused, and in
    /// hosted mode nothing changed (a tenant's webhook was always held to
    /// public addresses, at save and at dispatch), so no key is named.
    #[tokio::test]
    async fn the_stored_keys_whose_internal_webhook_is_refused_are_named() {
        use crate::config::{InternalTargets, Mode};
        let db = crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        let member = person(&db, crate::types::Role::Member).await;
        let admin = person(&db, crate::types::Role::Admin).await;
        let by_ip = hooked_key(&db, "member-ip", Some(&member), "http://127.0.0.1:9/hook?token=s3cret").await;
        let by_name = hooked_key(&db, "member-name", Some(&member), "http://localhost:9/hook").await;
        hooked_key(&db, "admin-ip", Some(&admin), "http://10.0.0.5/hook").await;
        hooked_key(&db, "master-ip", None, "http://127.0.0.1:9/hook").await;
        hooked_key(&db, "member-public", Some(&member), "https://8.8.8.8/hook").await;

        let mut named = refused_key_webhooks(&db, Mode::SingleTenant, InternalTargets::Admins).await.unwrap();
        named.sort_by(|a, b| a.key_prefix.cmp(&b.key_prefix));
        let refused = |key: &crate::types::ApiKey, host: &str| RefusedKeyWebhook {
            key_id: key.id,
            key_prefix: key.key_prefix.clone(),
            owner: Some(member.clone()),
            host: host.to_string(),
        };
        assert_eq!(named, vec![refused(&by_ip, "127.0.0.1"), refused(&by_name, "localhost")], "only the Member's internal webhooks");
        let everyone = refused_key_webhooks(&db, Mode::SingleTenant, InternalTargets::Everyone).await.unwrap();
        assert!(everyone.is_empty(), "with internal_targets: everyone dispatch refuses none: {everyone:?}");
        let hosted = refused_key_webhooks(&db, Mode::Hosted, InternalTargets::Admins).await.unwrap();
        assert!(hosted.is_empty(), "hosted: nothing changed, so nothing is named: {hosted:?}");
    }

    #[derive(Clone, Default)]
    struct Logs(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Logs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
        type Writer = Logs;
        fn make_writer(&'a self) -> Logs {
            self.clone()
        }
    }

    /// The warnings `run` logs on this thread, one per line.
    async fn warnings_of(run: impl std::future::Future<Output = ()>) -> Vec<String> {
        log_lines_of(run).await.into_iter().filter(|l| l.contains(" WARN ")).collect()
    }

    /// The lines `run` logs on this thread, at any level.
    async fn log_lines_of(run: impl std::future::Future<Output = ()>) -> Vec<String> {
        // So no callsite is off because another test's thread reached it
        // first (FU-SPANFLAKE).
        crate::util::test_logs::hold_process_subscriber();
        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt().with_writer(logs.clone()).with_ansi(false).with_max_level(tracing::Level::TRACE).finish();
        {
            let _guard = tracing::subscriber::set_default(subscriber);
            run.await;
        }
        let text = String::from_utf8_lossy(&logs.0.lock().unwrap()).into_owned();
        text.lines().map(str::to_string).collect()
    }

    /// FU-STWEBHOOK: the startup notice is one warning per named key, with its
    /// id and prefix, its owner and the URL's host (never the URL's path or
    /// query, which may hold a secret), and one that sums them up. Nothing at
    /// all when no key is named.
    #[tokio::test]
    async fn the_startup_notice_names_each_refused_key_and_sums_them_up() {
        use crate::config::{InternalTargets, Mode};
        let db = crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        let member = person(&db, crate::types::Role::Member).await;
        let admin = person(&db, crate::types::Role::Admin).await;
        let refused = hooked_key(&db, "member-ip", Some(&member), "http://127.0.0.1:9/hook?token=s3cret").await;
        hooked_key(&db, "admin-ip", Some(&admin), "http://10.0.0.5/hook").await;

        let warned = warnings_of(name_refused_key_webhooks(&db, Mode::SingleTenant, InternalTargets::Admins)).await;
        assert_eq!(warned.len(), 2, "one line for the key and one summing up: {warned:#?}");
        let names = [refused.id.to_string(), refused.key_prefix.clone(), member.clone(), "127.0.0.1".to_string()];
        assert!(names.iter().all(|n| warned[0].contains(n.as_str())), "the key's line names {names:?}: {}", warned[0]);
        assert!(warned[1].contains("refused_keys=1"), "the summary counts the keys: {}", warned[1]);
        assert!(warned.iter().all(|l| !l.contains("s3cret") && !l.contains("token") && !l.contains("/hook")), "no URL path or query: {warned:#?}");

        let quiet = warnings_of(name_refused_key_webhooks(&db, Mode::SingleTenant, InternalTargets::Everyone)).await;
        assert!(quiet.is_empty(), "nothing to name, nothing said: {quiet:#?}");
    }

    // ─── the durable outbox (migration 029) ──────────────────────────────────

    /// A dispatcher claiming as `name`, on a dispatcher of its own (8 at once
    /// in all, 4 for one org), waiting `schedule` (in seconds) between attempts.
    fn an_outbox(name: &str, schedule: &[u64]) -> Arc<Outbox> {
        let schedule = schedule.iter().map(|s| std::time::Duration::from_secs(*s)).collect();
        Arc::new(Outbox::new(name, Arc::new(WebhookDeliveries::new(8, 4)), schedule))
    }

    /// What the tests' attempts are sent with: single-tenant, the URLs as
    /// stored, and the client a master-key key's webhook goes out on.
    fn plain_context() -> SendContext {
        SendContext {
            mode: crate::config::Mode::SingleTenant,
            internal: Default::default(),
            presenting: None,
            inside_client: crate::util::ssrf::no_redirect_client(),
        }
    }

    /// A fresh database, as the store and as its SQLite self (to move a
    /// delivery's times, standing in for the clock).
    async fn outbox_db() -> (Arc<dyn DatabaseStore>, Arc<crate::db::sqlite::SqliteDatabase>) {
        let sqlite = Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        (sqlite.clone(), sqlite)
    }

    /// Delivery `id`'s next attempt is due now: its wait is over.
    async fn make_due(sqlite: &crate::db::sqlite::SqliteDatabase, id: &str) {
        sqlx::query("UPDATE webhook_outbox SET next_attempt_at = strftime('%Y-%m-%d %H:%M:%f', 'now', '-1 seconds') WHERE id = ?")
            .bind(id)
            .execute(sqlite.pool())
            .await
            .unwrap();
    }

    /// The lease on delivery `id` has run out: its holder died.
    async fn lapse_lease(sqlite: &crate::db::sqlite::SqliteDatabase, id: &str) {
        sqlx::query("UPDATE webhook_outbox SET claimed_until = strftime('%Y-%m-%d %H:%M:%f', 'now', '-1 seconds') WHERE id = ?")
            .bind(id)
            .execute(sqlite.pool())
            .await
            .unwrap();
    }

    /// A key whose webhook is `url`, and a video job of it, `id`, that has
    /// just completed: its terminal write recorded the delivery.
    async fn just_finished_job(db: &crate::db::sqlite::SqliteDatabase, id: &str, url: &str) -> crate::types::ApiKey {
        let key = db.create_api_key(&format!("k-{id}"), &format!("hash-{id}"), "lg-k", None, None, "generate,read", Some(url)).await.unwrap();
        db.insert_generation(id, Some(&key.id), "mock/video-gen", "mock", "video", Some("job"), 0.0, None, None).await.unwrap();
        db.update_generation_if_active(id, "completed", 100, Some("https://cdn.example/v.mp4"), None, Some(chrono::Utc::now()), None).await.unwrap();
        key
    }

    /// The same job, ended longer ago than its delivery's first-attempt grace
    /// ([`OUTBOX_FIRST_ATTEMPT_GRACE`]): the delivery is due now, the
    /// dispatcher's to send (the test moves its due time, standing in for
    /// the grace).
    async fn finished_job(db: &crate::db::sqlite::SqliteDatabase, id: &str, url: &str) -> crate::types::ApiKey {
        let key = just_finished_job(db, id, url).await;
        let recorded = db.webhook_outbox_of(id).await.unwrap().expect("its terminal write recorded the delivery");
        make_due(db, &recorded.id).await;
        key
    }

    /// One tick of `outbox`, and the attempts it started run to their end:
    /// how many it started.
    async fn tick_and_wait(outbox: &Arc<Outbox>, db: &Arc<dyn DatabaseStore>, ctx: &SendContext) -> usize {
        let started = outbox.tick(db, ctx).await;
        let left = outbox.deliveries.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(10)).await;
        assert!(left.is_empty(), "attempts still running: {left:?}");
        started
    }

    /// The value of header `name` on `request` ("" when it has none).
    fn header_of(request: &wiremock::Request, name: &str) -> String {
        request.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string()
    }

    /// A store that counts the outbox claims made through it and tells when
    /// a key is read; everything else is the SQLite database inside's.
    struct CountingStore {
        inner: Arc<crate::db::sqlite::SqliteDatabase>,
        claims_due: std::sync::atomic::AtomicUsize,
        claims_of: std::sync::atomic::AtomicUsize,
        key_read: tokio::sync::Notify,
    }

    impl CountingStore {
        fn over(inner: &Arc<crate::db::sqlite::SqliteDatabase>) -> Arc<Self> {
            Arc::new(Self { inner: inner.clone(), claims_due: Default::default(), claims_of: Default::default(), key_read: tokio::sync::Notify::new() })
        }

        /// The dispatchers' claims of due deliveries made so far.
        fn claims_due(&self) -> usize {
            self.claims_due.load(std::sync::atomic::Ordering::SeqCst)
        }

        /// The terminal moment's claims of one generation's delivery so far.
        fn claims_of(&self) -> usize {
            self.claims_of.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl DatabaseStore for CountingStore {
        async fn insert_generation(
            &self, id: &str, key_id: Option<&uuid::Uuid>, model: &str, provider: &str, media_type: &str,
            provider_job_id: Option<&str>, cost_usd: f64, org_id: Option<&str>, app_id: Option<&str>,
        ) -> Result<(), sqlx::Error> {
            self.inner.insert_generation(id, key_id, model, provider, media_type, provider_job_id, cost_usd, org_id, app_id).await
        }
        async fn update_generation_status(
            &self, id: &str, status: &str, progress: i32, result_url: Option<&str>, error: Option<&str>,
            completed_at: Option<chrono::DateTime<chrono::Utc>>,
        ) -> Result<(), sqlx::Error> {
            self.inner.update_generation_status(id, status, progress, result_url, error, completed_at).await
        }
        async fn update_generation_metadata(&self, id: &str, metadata: &serde_json::Value) -> Result<(), sqlx::Error> {
            self.inner.update_generation_metadata(id, metadata).await
        }
        async fn get_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error> {
            self.inner.get_generation(id).await
        }
        async fn list_active_generations(&self, limit: u32) -> Result<Vec<Generation>, sqlx::Error> {
            self.inner.list_active_generations(limit).await
        }
        async fn list_generations(&self, key_id: Option<&uuid::Uuid>, page: u32, per_page: u32) -> Result<Vec<Generation>, sqlx::Error> {
            self.inner.list_generations(key_id, page, per_page).await
        }
        async fn count_generations(&self, key_id: Option<&uuid::Uuid>) -> Result<i64, sqlx::Error> {
            self.inner.count_generations(key_id).await
        }
        async fn cancel_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error> {
            self.inner.cancel_generation(id).await
        }
        async fn log_request(
            &self, id: &str, model: &str, provider: &str, status: &str, media_type: &str, cost_usd: f64, latency_ms: i64,
            error: Option<&str>, metadata: Option<&serde_json::Value>, org_id: Option<&str>, app_id: Option<&str>,
        ) -> Result<(), sqlx::Error> {
            self.inner.log_request(id, model, provider, status, media_type, cost_usd, latency_ms, error, metadata, org_id, app_id).await
        }
        async fn update_request_log_status(&self, id: &str, status: &str, error: Option<&str>) -> Result<(), sqlx::Error> {
            self.inner.update_request_log_status(id, status, error).await
        }
        async fn get_request_logs(&self, page: u32, per_page: u32) -> Result<(Vec<crate::types::RequestLog>, u64), sqlx::Error> {
            self.inner.get_request_logs(page, per_page).await
        }
        async fn get_request_logs_filtered(
            &self, filters: &crate::types::LogFilters, page: u32, per_page: u32,
        ) -> Result<(Vec<crate::types::RequestLog>, u64), sqlx::Error> {
            self.inner.get_request_logs_filtered(filters, page, per_page).await
        }
        async fn create_api_key(
            &self, name: &str, key_hash: &str, key_prefix: &str, token_quota: Option<f64>, rpm_limit: Option<u32>, scopes: &str,
            webhook_url: Option<&str>,
        ) -> Result<crate::types::ApiKey, sqlx::Error> {
            self.inner.create_api_key(name, key_hash, key_prefix, token_quota, rpm_limit, scopes, webhook_url).await
        }
        async fn get_api_key(&self, id: &uuid::Uuid) -> Result<Option<crate::types::ApiKey>, sqlx::Error> {
            let key = self.inner.get_api_key(id).await;
            self.key_read.notify_one();
            key
        }
        async fn update_api_key(&self, id: &uuid::Uuid, req: &crate::types::UpdateApiKeyRequest) -> Result<Option<crate::types::ApiKey>, sqlx::Error> {
            self.inner.update_api_key(id, req).await
        }
        async fn lookup_api_key_by_hash(&self, key_hash: &str) -> Result<Option<crate::types::ApiKey>, sqlx::Error> {
            self.inner.lookup_api_key_by_hash(key_hash).await
        }
        async fn atomic_charge_tokens(&self, id: &uuid::Uuid, cost_usd: f64) -> Result<f64, sqlx::Error> {
            self.inner.atomic_charge_tokens(id, cost_usd).await
        }
        async fn validate_api_key(&self, key_hash: &str) -> Result<Option<crate::types::ApiKey>, sqlx::Error> {
            self.inner.validate_api_key(key_hash).await
        }
        async fn list_api_keys(&self) -> Result<Vec<crate::types::ApiKey>, sqlx::Error> {
            self.inner.list_api_keys().await
        }
        async fn revoke_api_key(&self, id: &uuid::Uuid) -> Result<bool, sqlx::Error> {
            self.inner.revoke_api_key(id).await
        }
        async fn get_stats(&self) -> Result<crate::types::ProxyStats, sqlx::Error> {
            self.inner.get_stats().await
        }

        async fn insert_webhook_delivery(&self, delivery: &WebhookDelivery) -> Result<(), sqlx::Error> {
            self.inner.insert_webhook_delivery(delivery).await
        }
        fn keeps_webhook_outbox(&self) -> bool {
            self.inner.keeps_webhook_outbox()
        }
        async fn webhook_outbox_of(&self, generation_id: &str) -> Result<Option<crate::db::OutboxRecord>, sqlx::Error> {
            self.inner.webhook_outbox_of(generation_id).await
        }
        async fn claim_webhook_delivery_of(
            &self, generation_id: &str, claimant: &str, hold: chrono::Duration,
        ) -> Result<Option<crate::db::OutboxDelivery>, sqlx::Error> {
            self.claims_of.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.claim_webhook_delivery_of(generation_id, claimant, hold).await
        }
        async fn claim_due_webhook_deliveries(
            &self, claimant: &str, limit: u32, per_org: u32, hold: chrono::Duration,
        ) -> Result<Vec<crate::db::OutboxDelivery>, sqlx::Error> {
            self.claims_due.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.claim_due_webhook_deliveries(claimant, limit, per_org, hold).await
        }
        async fn renew_webhook_claims(&self, claimant: &str, ids: &[String], hold: chrono::Duration) -> Result<u64, sqlx::Error> {
            self.inner.renew_webhook_claims(claimant, ids, hold).await
        }
        async fn webhook_delivery_due(&self) -> Result<bool, sqlx::Error> {
            self.inner.webhook_delivery_due().await
        }
        async fn webhook_answered_for(&self, generation_id: &str) -> Result<bool, sqlx::Error> {
            self.inner.webhook_answered_for(generation_id).await
        }
        async fn finish_webhook_attempt(&self, id: &str, claimant: &str, attempt: &crate::db::OutboxAttempt<'_>) -> Result<bool, sqlx::Error> {
            self.inner.finish_webhook_attempt(id, claimant, attempt).await
        }
    }

    /// An idle dispatcher writes nothing (review 2026-11-15 round 2, finding
    /// 6): each tick looks for a due delivery first, with a read, and claims
    /// (an UPDATE, which on SQLite takes the write lock) only once one is due.
    #[tokio::test]
    async fn an_idle_tick_claims_nothing_until_a_delivery_is_due() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (_, sqlite) = outbox_db().await;
        let counting = CountingStore::over(&sqlite);
        let db: Arc<dyn DatabaseStore> = counting.clone();
        let (outbox, ctx) = (an_outbox("replica-idle", &[10]), plain_context());
        just_finished_job(&sqlite, "g-in-its-grace", &format!("{}/hook", server.uri())).await;
        for _ in 0..3 {
            assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 0, "nothing is due");
        }
        assert_eq!(counting.claims_due(), 0, "no claim while nothing is due");

        make_due(&sqlite, &db.webhook_outbox_of("g-in-its-grace").await.unwrap().unwrap().id).await;
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 1, "claimed once due");
        assert_eq!((counting.claims_due(), arrivals(&server, "/hook").await), (1, 1));
    }

    /// The terminal moment of a job whose key has no webhook costs one read
    /// of the key and no outbox claim (review 2026-11-15 round 2, finding 7).
    #[tokio::test]
    async fn a_job_whose_key_has_no_webhook_claims_no_outbox_delivery() {
        let (_, sqlite) = outbox_db().await;
        let counting = CountingStore::over(&sqlite);
        let db: Arc<dyn DatabaseStore> = counting.clone();
        let key = sqlite.create_api_key("k-plain", "hash-plain", "lg-k", None, None, "generate,read", None).await.unwrap();
        sqlite.insert_generation("g-plain", Some(&key.id), "mock/video-gen", "mock", "video", Some("job"), 0.0, None, None).await.unwrap();
        sqlite.update_generation_if_active("g-plain", "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
        let row = sqlite.get_generation("g-plain").await.unwrap().unwrap();

        crate::proxy::poller::dispatch_terminal_webhook(&db, &crate::util::ssrf::no_redirect_client(), crate::config::Mode::SingleTenant, row, None).await;
        // The key is read either way; by then any claim has been made.
        tokio::time::timeout(std::time::Duration::from_secs(10), counting.key_read.notified()).await.expect("the key was never read");
        assert_eq!(counting.claims_of(), 0, "no outbox claim for a key with no webhook");
    }

    /// The dispatcher claims a due delivery and sends it: signed with the
    /// key's secret, under the delivery's id, the generation as stored. The
    /// attempt is recorded, and the delivery is done.
    #[tokio::test]
    async fn the_dispatcher_sends_a_due_delivery_and_records_the_attempt() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        let key = finished_job(&sqlite, "g-due", &format!("{}/hook", server.uri())).await;
        let delivery = db.webhook_outbox_of("g-due").await.unwrap().expect("recorded by the terminal write");

        let outbox = an_outbox("replica-a", &[10]);
        assert_eq!(tick_and_wait(&outbox, &db, &plain_context()).await, 1, "the due delivery is claimed");

        let received = server.received_requests().await.unwrap_or_default();
        assert_eq!(received.len(), 1, "one attempt");
        assert_eq!(header_of(&received[0], "x-litegen-delivery-id"), delivery.id, "sent under the delivery's id");
        let body = std::str::from_utf8(&received[0].body).unwrap();
        assert_eq!(header_of(&received[0], "x-litegen-signature"), format!("sha256={}", compute_signature(&key.key_hash, body)), "signed with the key's secret");
        let sent: Generation = serde_json::from_str(body).unwrap();
        assert_eq!((sent.id.as_str(), sent.status), ("g-due", GenerationStatus::Completed));
        let (rows, total) = db.list_webhook_deliveries(&key.id.to_string(), 1, 50).await.unwrap();
        assert_eq!(total, 1, "the attempt is recorded");
        assert!(rows[0].success && rows[0].status_code == Some(200) && rows[0].attempt_number == 1, "{:?}", rows[0]);
        let after = db.webhook_outbox_of("g-due").await.unwrap().unwrap();
        assert_eq!((after.status.as_str(), after.attempts, after.claimed_by.as_deref()), ("delivered", 1, None), "{after:?}");
        assert!(after.finished_at.is_some(), "{after:?}");
    }

    /// A new delivery is left to the replica that ended its job for its
    /// first-attempt grace (review 2026-11-15 round 2, finding 2): that
    /// replica settles the job (a cancel's refund comes up to 5 s after its
    /// terminal write) and then sends the first attempt at once, so the
    /// webhook carries the settled row. No other replica's tick takes it
    /// meanwhile. Once the grace is over (the test moves the due time), the
    /// dispatcher sends one whose replica died before sending it.
    #[tokio::test]
    async fn a_new_delivery_is_its_terminal_replicas_to_send_for_its_grace() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        let url = format!("{}/hook", server.uri());
        just_finished_job(&sqlite, "g-just-ended", &url).await;
        just_finished_job(&sqlite, "g-its-replica-died", &url).await;
        let (another_replica, ctx) = (an_outbox("replica-b", &[10]), plain_context());
        assert_eq!(tick_and_wait(&another_replica, &db, &ctx).await, 0, "no tick takes a delivery within its grace");

        let ended_it = an_outbox("replica-a", &[10]);
        assert_eq!(ended_it.send_first(&db, &ctx, "g-just-ended").await, FirstAttempt::Sent, "the replica that ended the job sends it at once");
        assert_eq!(arrivals(&server, "/hook").await, 1);
        assert_eq!(db.webhook_outbox_of("g-just-ended").await.unwrap().unwrap().status, "delivered");

        let orphan = db.webhook_outbox_of("g-its-replica-died").await.unwrap().unwrap();
        make_due(&sqlite, &orphan.id).await;
        assert_eq!(tick_and_wait(&another_replica, &db, &ctx).await, 1, "after its grace the dispatcher sends it");
        assert_eq!(arrivals(&server, "/hook").await, 2);
    }

    /// During a rolling upgrade a replica from before the outbox sends its
    /// jobs' webhooks from memory, recording each attempt with no delivery
    /// id, and the trigger records each of those webhooks here as well
    /// (review 2026-11-15 round 2, finding 3). A delivery whose job's webhook
    /// was answered 2xx already ends `delivered` without a request; one whose
    /// earlier attempts all failed is sent.
    #[tokio::test]
    async fn a_webhook_an_older_replica_delivered_is_not_sent_again() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        let url = format!("{}/hook", server.uri());
        for (id, answered) in [("g-sent-by-an-old-replica", true), ("g-failed-on-an-old-replica", false)] {
            let key = finished_job(&sqlite, id, &url).await;
            // An attempt as a replica from before migration 029 records it.
            let attempt = WebhookDelivery {
                id: format!("wh-{}", uuid::Uuid::new_v4()),
                key_id: key.id.to_string(),
                generation_id: id.to_string(),
                url: url.clone(),
                attempt_number: 1,
                status_code: Some(if answered { 200 } else { 503 }),
                success: answered,
                response_body: None,
                error_message: None,
                payload_json: "{}".to_string(),
                created_at: chrono::Utc::now(),
                delivery_id: None,
            };
            db.insert_webhook_delivery(&attempt).await.unwrap();
        }

        let (outbox, ctx) = (an_outbox("replica-upgraded", &[10]), plain_context());
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 2, "both are claimed");
        assert_eq!(arrivals(&server, "/hook").await, 1, "only the webhook no replica delivered is sent");
        let skipped = db.webhook_outbox_of("g-sent-by-an-old-replica").await.unwrap().unwrap();
        assert_eq!((skipped.status.as_str(), skipped.attempts, skipped.last_status_code), ("delivered", 0, None), "{skipped:?}");
        assert!(skipped.last_error.as_deref().is_some_and(|e| e.contains("2xx")), "it says why: {skipped:?}");
        let sent = db.webhook_outbox_of("g-failed-on-an-old-replica").await.unwrap().unwrap();
        assert_eq!((sent.status.as_str(), sent.attempts), ("delivered", 1), "{sent:?}");
    }

    /// A redelivered delivery (`POST .../redeliver`) is sent again under its
    /// id, a fresh series that starts at attempt 1, although an earlier
    /// attempt for its job was answered 2xx: the redeliver asked for it.
    #[tokio::test]
    async fn a_redelivered_delivery_is_sent_again_under_its_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        let key = finished_job(&sqlite, "g-again", &format!("{}/hook", server.uri())).await;
        let (outbox, ctx) = (an_outbox("replica-a", &[10]), plain_context());
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 1);
        let delivered = db.webhook_outbox_of("g-again").await.unwrap().unwrap();
        assert_eq!(delivered.status, "delivered");

        let asked = db.redeliver_webhook_delivery(&key.id.to_string(), &delivered.id, chrono::Duration::seconds(60)).await.unwrap();
        assert!(matches!(asked, crate::db::Redelivery::Redelivered(_)), "{asked:?}");
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 1, "due at once");
        let ids: Vec<String> = server.received_requests().await.unwrap_or_default().iter().map(|r| header_of(r, "x-litegen-delivery-id")).collect();
        assert_eq!(ids, [delivered.id.clone(), delivered.id.clone()], "sent again under its id");
        let (rows, _) = db.list_webhook_deliveries(&key.id.to_string(), 1, 50).await.unwrap();
        let mut numbers: Vec<i32> = rows.iter().map(|r| r.attempt_number).collect();
        numbers.sort();
        assert_eq!(numbers, [1, 1], "the new series starts at attempt 1");
        let again = db.webhook_outbox_of("g-again").await.unwrap().unwrap();
        assert_eq!((again.status.as_str(), again.attempts), ("delivered", 1), "{again:?}");
    }

    /// A delivery outlives the process that recorded it: the job ends and
    /// the process dies before any attempt (a crash right after the terminal
    /// write), and a new dispatcher on the same database sends it.
    #[tokio::test]
    async fn a_pending_delivery_is_sent_after_a_restart() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite:{}?mode=rwc", dir.path().join("litegen.db").display());
        {
            let sqlite = crate::db::sqlite::SqliteDatabase::connect(&url).await.unwrap();
            finished_job(&sqlite, "g-crash", &format!("{}/hook", server.uri())).await;
        }
        assert!(server.received_requests().await.unwrap_or_default().is_empty(), "nothing was sent before the crash");

        let db: Arc<dyn DatabaseStore> = Arc::new(crate::db::sqlite::SqliteDatabase::connect(&url).await.unwrap());
        let restarted = an_outbox("replica-after-the-restart", &[10]);
        assert_eq!(tick_and_wait(&restarted, &db, &plain_context()).await, 1, "the pending delivery is claimed");
        assert_eq!(server.received_requests().await.unwrap_or_default().len(), 1, "and sent");
        assert_eq!(db.webhook_outbox_of("g-crash").await.unwrap().unwrap().status, "delivered");
    }

    /// A failed attempt is not the end: the delivery stays pending, its next
    /// attempt due after the schedule's first wait (on the database's
    /// clock), its lease given back, and no tick sends it before then. Once
    /// it is due (the test moves its due time, standing in for the wait), the
    /// next tick sends it again.
    #[tokio::test]
    async fn a_failed_attempt_is_sent_again_when_its_next_attempt_is_due() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        finished_job(&sqlite, "g-retry", &format!("{}/hook", server.uri())).await;
        let (outbox, ctx) = (an_outbox("replica-a", &[30, 60]), plain_context());

        let before = chrono::Utc::now();
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 1);
        let waiting = db.webhook_outbox_of("g-retry").await.unwrap().unwrap();
        assert_eq!((waiting.status.as_str(), waiting.attempts, waiting.last_status_code), ("pending", 1, Some(503)), "{waiting:?}");
        let wait = waiting.next_attempt_at - before;
        assert!(wait >= chrono::Duration::seconds(29) && wait <= chrono::Duration::seconds(35), "due after the first wait, 30 s: {wait:?}");
        assert!(waiting.claimed_by.is_none() && waiting.claimed_until.is_none(), "its lease is given back: {waiting:?}");
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 0, "not due yet");
        assert_eq!(server.received_requests().await.unwrap_or_default().len(), 1);

        make_due(&sqlite, &waiting.id).await;
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 1, "due now");
        assert_eq!(server.received_requests().await.unwrap_or_default().len(), 2, "the second attempt");
        let done = db.webhook_outbox_of("g-retry").await.unwrap().unwrap();
        assert_eq!((done.status.as_str(), done.attempts, done.last_status_code), ("delivered", 2, Some(200)), "{done:?}");
    }

    /// After its last attempt fails, a delivery is failed for good, and kept
    /// for the log: no tick sends it again.
    #[tokio::test]
    async fn a_delivery_whose_last_attempt_fails_is_failed_for_good() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        finished_job(&sqlite, "g-down", &format!("{}/hook", server.uri())).await;
        // One wait: two attempts in all.
        let (outbox, ctx) = (an_outbox("replica-a", &[30]), plain_context());
        tick_and_wait(&outbox, &db, &ctx).await;
        let id = db.webhook_outbox_of("g-down").await.unwrap().unwrap().id;
        make_due(&sqlite, &id).await;
        tick_and_wait(&outbox, &db, &ctx).await;

        let failed = db.webhook_outbox_of("g-down").await.unwrap().unwrap();
        assert_eq!((failed.status.as_str(), failed.attempts, failed.last_status_code), ("failed", 2, Some(500)), "{failed:?}");
        assert!(failed.finished_at.is_some() && failed.claimed_by.is_none(), "{failed:?}");
        make_due(&sqlite, &id).await;
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 0, "a failed delivery is never claimed again");
        assert_eq!(server.received_requests().await.unwrap_or_default().len(), 2);
    }

    /// Every attempt of one delivery carries the delivery's id, the one
    /// stored with it, so a receiver can drop a repeat.
    #[tokio::test]
    async fn every_attempt_of_an_outbox_delivery_carries_its_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        finished_job(&sqlite, "g-same-id", &format!("{}/hook", server.uri())).await;
        let (outbox, ctx) = (an_outbox("replica-a", &[30]), plain_context());
        let id = db.webhook_outbox_of("g-same-id").await.unwrap().unwrap().id;
        tick_and_wait(&outbox, &db, &ctx).await;
        make_due(&sqlite, &id).await;
        tick_and_wait(&outbox, &db, &ctx).await;

        let ids: Vec<String> = server.received_requests().await.unwrap_or_default().iter().map(|r| header_of(r, "x-litegen-delivery-id")).collect();
        assert_eq!(ids, vec![id.clone(), id], "both attempts carry the stored delivery id");
    }

    /// Two dispatchers on one database (two replicas) never send one
    /// delivery at once: of two ticks at the same moment one claims it, and
    /// while its attempt runs (the receiver answers after 500 ms) the other
    /// replica's next tick doesn't take it either.
    #[tokio::test]
    async fn two_dispatchers_never_send_the_same_delivery_at_once() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_millis(500)))
            .mount(&server)
            .await;
        let (db, sqlite) = outbox_db().await;
        finished_job(&sqlite, "g-once", &format!("{}/hook", server.uri())).await;
        let (a, b, ctx) = (an_outbox("replica-a", &[10]), an_outbox("replica-b", &[10]), plain_context());

        let (by_a, by_b) = tokio::join!(a.tick(&db, &ctx), b.tick(&db, &ctx));
        assert_eq!(by_a + by_b, 1, "one of them claims it: a {by_a}, b {by_b}");
        let other = if by_a == 1 { &b } else { &a };
        assert_eq!(other.tick(&db, &ctx).await, 0, "the lease of the one sending holds it");
        for o in [&a, &b] {
            let left = o.deliveries.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(10)).await;
            assert!(left.is_empty(), "{left:?}");
        }
        assert_eq!(server.received_requests().await.unwrap_or_default().len(), 1, "sent once");
    }

    /// A delivery whose lease ran out (the replica holding it died before
    /// sending) is taken by another dispatcher; until then it isn't.
    #[tokio::test]
    async fn a_delivery_whose_lease_ran_out_is_taken_by_another_dispatcher() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        finished_job(&sqlite, "g-orphan", &format!("{}/hook", server.uri())).await;
        let held = db.claim_due_webhook_deliveries("replica-that-dies", 10, 10, chrono::Duration::seconds(60)).await.unwrap();
        assert_eq!(held.len(), 1, "the replica that dies claimed it");
        let (b, ctx) = (an_outbox("replica-b", &[10]), plain_context());
        assert_eq!(tick_and_wait(&b, &db, &ctx).await, 0, "its lease holds it");

        lapse_lease(&sqlite, &held[0].id).await;
        assert_eq!(tick_and_wait(&b, &db, &ctx).await, 1, "the lease ran out: b takes it");
        assert_eq!(server.received_requests().await.unwrap_or_default().len(), 1, "and sends it");
        let done = db.webhook_outbox_of("g-orphan").await.unwrap().unwrap();
        assert_eq!((done.status.as_str(), done.attempts), ("delivered", 1), "{done:?}");
    }

    /// An endpoint that answers 410 Gone says it is gone for good: its
    /// delivery stops at once, failed. Every other failure is retried, a 404
    /// too (a receiver mid-deploy can answer one).
    #[tokio::test]
    async fn a_410_stops_a_delivery_at_once_and_a_404_is_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/gone")).respond_with(ResponseTemplate::new(410)).mount(&server).await;
        Mock::given(method("POST")).and(path("/missing")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        finished_job(&sqlite, "g-gone-410", &format!("{}/gone", server.uri())).await;
        finished_job(&sqlite, "g-missing-404", &format!("{}/missing", server.uri())).await;
        let (outbox, ctx) = (an_outbox("replica-a", &[30, 60]), plain_context());
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 2);

        let gone = db.webhook_outbox_of("g-gone-410").await.unwrap().unwrap();
        assert_eq!((gone.status.as_str(), gone.attempts, gone.last_status_code), ("failed", 1, Some(410)), "{gone:?}");
        let missing = db.webhook_outbox_of("g-missing-404").await.unwrap().unwrap();
        assert_eq!((missing.status.as_str(), missing.attempts, missing.last_status_code), ("pending", 1, Some(404)), "{missing:?}");
        make_due(&sqlite, &gone.id).await;
        make_due(&sqlite, &missing.id).await;
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 1, "only the 404's delivery is tried again");
        assert_eq!((arrivals(&server, "/gone").await, arrivals(&server, "/missing").await), (1, 2));
    }

    /// Each attempt reads the key as it is then, as the in-memory delivery
    /// did once: a key whose webhook_url was cleared after its job ended
    /// (to "" by a PATCH, or to none) gets nothing, and its delivery ends
    /// failed at once, naming why. So does one whose generation is gone.
    #[tokio::test]
    async fn a_delivery_with_nowhere_to_go_ends_failed_without_a_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        let url = format!("{}/hook", server.uri());
        for (id, cleared) in [("g-emptied", Some("")), ("g-unset", None)] {
            let key = finished_job(&sqlite, id, &url).await;
            sqlx::query("UPDATE api_keys SET webhook_url = ? WHERE id = ?").bind(cleared).bind(key.id.to_string()).execute(sqlite.pool()).await.unwrap();
        }
        finished_job(&sqlite, "g-gone", &url).await;
        sqlx::query("DELETE FROM generations WHERE id = 'g-gone'").execute(sqlite.pool()).await.unwrap();

        let (outbox, ctx) = (an_outbox("replica-a", &[10]), plain_context());
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 3, "all three are claimed");
        assert!(server.received_requests().await.unwrap_or_default().is_empty(), "nothing was sent");
        for (id, why) in [("g-emptied", "webhook_url"), ("g-unset", "webhook_url"), ("g-gone", "generation")] {
            let d = db.webhook_outbox_of(id).await.unwrap().unwrap();
            assert_eq!((d.status.as_str(), d.attempts, d.last_status_code), ("failed", 0, None), "{id}: {d:?}");
            assert!(d.last_error.as_deref().is_some_and(|e| e.contains(why)), "{id}: the reason names its {why}: {d:?}");
        }
    }

    /// The attempt holds the key to the rules of the moment it is sent
    /// (SEC-STWEBHOOK): in hosted mode a webhook goes to public addresses
    /// only, so one that points inside the deployment ends failed without a
    /// request, as the in-memory delivery skipped it.
    #[tokio::test]
    async fn an_outbox_delivery_to_an_address_it_may_not_reach_ends_failed_without_a_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        finished_job(&sqlite, "g-inside", &format!("{}/hook", server.uri())).await;
        let hosted = SendContext { mode: crate::config::Mode::Hosted, inside_client: webhook_client(crate::config::Mode::Hosted), ..plain_context() };

        assert_eq!(tick_and_wait(&an_outbox("replica-a", &[10]), &db, &hosted).await, 1);
        assert!(server.received_requests().await.unwrap_or_default().is_empty(), "the deployment's network was reached");
        let d = db.webhook_outbox_of("g-inside").await.unwrap().unwrap();
        assert_eq!((d.status.as_str(), d.attempts), ("failed", 0), "{d:?}");
        assert!(d.last_error.as_deref().is_some_and(|e| e.contains("not allowed")), "{d:?}");
    }

    /// A delivery the outbox can't send shows in its key's list all the same:
    /// an attempt row under the delivery's id, with no status and the reason
    /// as its error. So the key's managers see why the webhook never came,
    /// and once the cause is fixed they redeliver it by that id and it is
    /// sent. Here a Member's key points inside the deployment, which only the
    /// deployment's own keys may reach; the fix is the Member becoming an
    /// Admin.
    #[tokio::test]
    async fn a_delivery_refused_at_send_time_is_listed_and_sent_once_redelivered() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        let member = person(&sqlite, crate::types::Role::Member).await;
        let url = format!("{}/hook", server.uri());
        let key = hooked_key(&sqlite, "member-hook", Some(&member), &url).await;
        sqlite.insert_generation("g-refused", Some(&key.id), "mock/video-gen", "mock", "video", Some("job"), 0.0, None, None).await.unwrap();
        sqlite.update_generation_if_active("g-refused", "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
        let delivery = db.webhook_outbox_of("g-refused").await.unwrap().expect("recorded");
        make_due(&sqlite, &delivery.id).await;
        let (outbox, ctx) = (an_outbox("replica-a", &[10]), plain_context());
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 1);
        assert!(server.received_requests().await.unwrap_or_default().is_empty(), "a Member's webhook may not reach inside");

        let (rows, total) = db.list_webhook_deliveries(&key.id.to_string(), 1, 50).await.unwrap();
        assert_eq!(total, 1, "the refusal is in the key's list");
        let row = &rows[0];
        assert_eq!((row.delivery_id.as_deref(), row.status_code, row.success, row.url.as_str()), (Some(delivery.id.as_str()), None, false, url.as_str()), "{row:?}");
        assert!(row.error_message.as_deref().is_some_and(|e| e.contains("not allowed")), "it says why: {row:?}");

        // The cause is fixed: the key's owner is an Admin of the deployment's org now.
        sqlite.update_member_role(crate::api::middleware::DEFAULT_ORG_ID, &member, crate::types::Role::Admin).await.unwrap();
        let asked = db.redeliver_webhook_delivery(&key.id.to_string(), &delivery.id, chrono::Duration::seconds(60)).await.unwrap();
        assert!(matches!(asked, crate::db::Redelivery::Redelivered(_)), "{asked:?}");
        assert_eq!(tick_and_wait(&outbox, &db, &ctx).await, 1, "due at once");
        let received = server.received_requests().await.unwrap_or_default();
        assert_eq!(received.len(), 1, "sent once the cause is fixed");
        assert_eq!(header_of(&received[0], "x-litegen-delivery-id"), delivery.id, "under its delivery id");
        assert_eq!(db.webhook_outbox_of("g-refused").await.unwrap().unwrap().status, "delivered");
    }

    /// Guard: a delivery whose generation or key is gone writes no attempt
    /// row: the list goes by key, so nothing would show it, and a row could
    /// trip a foreign key. It would catch `give_up` recording a row for every
    /// reason alike.
    #[tokio::test]
    async fn a_delivery_whose_generation_or_key_is_gone_writes_no_attempt_row() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        let url = format!("{}/hook", server.uri());
        finished_job(&sqlite, "g-gone", &url).await;
        sqlx::query("DELETE FROM generations WHERE id = 'g-gone'").execute(sqlite.pool()).await.unwrap();
        let gone = finished_job(&sqlite, "g-of-a-gone-key", &url).await;
        sqlx::query("UPDATE generations SET key_id = NULL WHERE id = 'g-of-a-gone-key'").execute(sqlite.pool()).await.unwrap();
        sqlx::query("DELETE FROM api_keys WHERE id = ?").bind(gone.id.to_string()).execute(sqlite.pool()).await.unwrap();

        assert_eq!(tick_and_wait(&an_outbox("replica-a", &[10]), &db, &plain_context()).await, 2);
        for (id, why) in [("g-gone", "generation"), ("g-of-a-gone-key", "key")] {
            let d = db.webhook_outbox_of(id).await.unwrap().unwrap();
            assert!(d.status == "failed" && d.last_error.as_deref().is_some_and(|e| e.contains(why)), "{id}: {d:?}");
        }
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM webhook_deliveries").fetch_one(sqlite.pool()).await.unwrap();
        assert_eq!(rows, 0, "no attempt row for a gone generation or key");
    }

    /// When the link in a webhook body was signed: the `issued` time (unix
    /// ms) of its `result_url`, a link litegen signed itself.
    fn issued_in(body: &[u8]) -> i64 {
        let sent: Generation = serde_json::from_slice(body).unwrap();
        let url = sent.result_url.unwrap_or_default();
        let issued = url.split(['?', '&']).find_map(|p| p.strip_prefix("issued="));
        issued.and_then(|t| t.parse().ok()).unwrap_or_else(|| panic!("not a link litegen signed: {url}"))
    }

    /// Each attempt is built when it is sent: its files' links are signed
    /// then, in the app's access mode, so a retry after the first link's time
    /// is up still carries working links (review 2026-11-15, finding 2). The
    /// delivery keeps the latest attempt's payload.
    #[tokio::test]
    async fn a_later_attempt_carries_freshly_signed_links() {
        use crate::api::middleware::{DEFAULT_APP_ID, DEFAULT_ORG_ID};
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        // The default app hands its files out as signed links that work for 60 s.
        let signed = crate::types::AppFileAccessUpsert {
            app_id: DEFAULT_APP_ID.into(),
            mode: Some("signed".into()),
            owner_scope: None,
            signed_url_ttl_seconds: Some(60),
            keep_outputs: None,
            retention_days: None,
            signed_links: None,
        };
        db.put_app_file_access(&signed).await.unwrap();
        let key = db.create_api_key("k-signed", "hash-signed", "lg-k", None, None, "generate,read", Some(&format!("{}/hook", server.uri()))).await.unwrap();
        db.insert_generation("g-signed", Some(&key.id), "mock/video-gen", "mock", "video", Some("job"), 0.0, Some(DEFAULT_ORG_ID), Some(DEFAULT_APP_ID)).await.unwrap();
        // A file no store of the app's can sign (the vendor's own): litegen signs its link.
        let file = "https://vendor.example/v.mp4";
        let assets = serde_json::json!({ "assets": [{ "index": 0, "kind": "video", "format": "mp4", "url": file }] });
        db.update_generation_if_active("g-signed", "completed", 100, Some(file), None, Some(chrono::Utc::now()), Some(&assets)).await.unwrap();
        make_due(&sqlite, &db.webhook_outbox_of("g-signed").await.unwrap().unwrap().id).await;
        let router = Arc::new(crate::proxy::router::ProxyRouter::new(
            Arc::new(crate::proxy::registry::ProviderRegistry::new()),
            Arc::new(crate::proxy::cache::GenerationCache::new(&Default::default())),
            Arc::new(crate::config::AppConfig::default()),
            Arc::new(crate::proxy::storage::LocalStore),
        ));
        let presenting = crate::access::present::Presenting { router, db: db.clone(), secrets_key: None, mode: crate::config::Mode::SingleTenant };
        let ctx = SendContext { presenting: Some(presenting), ..plain_context() };
        let outbox = an_outbox("replica-a", &[30]);

        tick_and_wait(&outbox, &db, &ctx).await;
        let first = issued_in(&server.received_requests().await.unwrap_or_default()[0].body);
        // The clock moves on past the first link's signing.
        while chrono::Utc::now().timestamp_millis() <= first {
            tokio::task::yield_now().await;
        }
        let retried_at = chrono::Utc::now().timestamp_millis();
        let id = db.webhook_outbox_of("g-signed").await.unwrap().unwrap().id;
        make_due(&sqlite, &id).await;
        tick_and_wait(&outbox, &db, &ctx).await;

        let received = server.received_requests().await.unwrap_or_default();
        assert_eq!(received.len(), 2, "two attempts");
        let second = issued_in(&received[1].body);
        assert!(second >= retried_at, "the retry's link was signed when the retry was sent: issued {second}, retried at {retried_at}, first {first}");
        let kept = db.webhook_outbox_of("g-signed").await.unwrap().unwrap().payload_json.unwrap_or_default();
        assert_eq!(kept.as_bytes(), &received[1].body[..], "the delivery keeps the latest attempt's payload");
    }

    /// The payload is kept once per delivery, not on each attempt's row
    /// (review 2026-11-15, finding 4). The list of a key's deliveries shows
    /// each attempt its delivery's id and its delivery's latest payload.
    #[tokio::test]
    async fn an_attempts_row_keeps_no_copy_of_the_payload_and_the_list_shows_its_deliverys() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        let key = finished_job(&sqlite, "g-payload-once", &format!("{}/hook", server.uri())).await;
        let (outbox, ctx) = (an_outbox("replica-a", &[30]), plain_context());
        tick_and_wait(&outbox, &db, &ctx).await;
        make_due(&sqlite, &db.webhook_outbox_of("g-payload-once").await.unwrap().unwrap().id).await;
        tick_and_wait(&outbox, &db, &ctx).await;

        let delivery = db.webhook_outbox_of("g-payload-once").await.unwrap().unwrap();
        let stored: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT payload_json, delivery_id FROM webhook_deliveries WHERE generation_id = 'g-payload-once' ORDER BY attempt_number")
                .fetch_all(sqlite.pool())
                .await
                .unwrap();
        assert_eq!(stored, vec![(String::new(), Some(delivery.id.clone())); 2], "each attempt's row names its delivery and keeps no payload");
        let (rows, total) = db.list_webhook_deliveries(&key.id.to_string(), 1, 50).await.unwrap();
        assert_eq!(total, 2);
        for row in &rows {
            assert_eq!(row.delivery_id.as_deref(), Some(delivery.id.as_str()), "{row:?}");
            assert_eq!(Some(&row.payload_json), delivery.payload_json.as_ref(), "the delivery's latest payload: {row:?}");
        }
    }

    /// Each tick renews the leases of the deliveries its dispatcher holds, so
    /// one that waits for a slot past its lease (every slot busy with a slow
    /// receiver) is not taken by another replica meanwhile.
    #[tokio::test]
    async fn a_tick_renews_the_leases_of_the_deliveries_its_dispatcher_holds() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        finished_job(&sqlite, "g-queued", &format!("{}/hook", server.uri())).await;
        let slots = Arc::new(WebhookDeliveries::new(1, 1));
        let outbox = Arc::new(Outbox::new("replica-a", slots.clone(), vec![std::time::Duration::from_secs(10)]));
        let ctx = plain_context();
        // Another delivery holds the one slot.
        let (holding, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
        let (has, go) = (holding.clone(), release.clone());
        slots.spawn("another-org", "holder", async move {
            has.notify_one();
            go.notified().await;
        });
        tokio::time::timeout(std::time::Duration::from_secs(10), holding.notified()).await.expect("the holder never got the slot");
        assert_eq!(outbox.tick(&db, &ctx).await, 1, "claimed: it waits for the slot");
        let id = db.webhook_outbox_of("g-queued").await.unwrap().unwrap().id;
        sqlx::query("UPDATE webhook_outbox SET claimed_until = strftime('%Y-%m-%d %H:%M:%f', 'now', '+1 seconds') WHERE id = ?")
            .bind(&id)
            .execute(sqlite.pool())
            .await
            .unwrap();

        let before = chrono::Utc::now();
        assert_eq!(outbox.tick(&db, &ctx).await, 0, "nothing more to claim");
        let held = db.webhook_outbox_of("g-queued").await.unwrap().unwrap();
        assert_eq!(held.claimed_by.as_deref(), Some("replica-a"), "{held:?}");
        let left = held.claimed_until.expect("held") - before;
        assert!(left > chrono::Duration::seconds(50), "the lease about to run out was renewed: {left:?}");
        release.notify_one();
        let still = slots.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(10)).await;
        assert!(still.is_empty(), "{still:?}");
        assert_eq!(db.webhook_outbox_of("g-queued").await.unwrap().unwrap().status, "delivered");
    }

    /// A graceful stop: the dispatcher claims nothing more, the attempt
    /// already sending finishes, and a delivery still waiting for its slot
    /// gives its turn up without sending. It stays pending, its lease given
    /// back with the replica's others, for another replica (or this one
    /// after its restart) to send: nothing holds the stop up, nothing is lost.
    #[tokio::test]
    async fn a_stopping_dispatcher_finishes_what_is_sending_and_starts_nothing_new() {
        let server = MockServer::start().await;
        // Long enough that the stop comes while the first attempt sends.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(2)))
            .mount(&server)
            .await;
        let (db, sqlite) = outbox_db().await;
        let url = format!("{}/hook", server.uri());
        finished_job(&sqlite, "g-first", &url).await;
        finished_job(&sqlite, "g-second", &url).await;
        let slots = Arc::new(WebhookDeliveries::new(1, 1));
        let outbox = Arc::new(Outbox::new("replica-a", slots.clone(), vec![std::time::Duration::from_secs(10)]));
        let ctx = plain_context();
        assert_eq!(outbox.tick(&db, &ctx).await, 2, "both claimed: one sends, one waits for the slot");
        until_arrivals(&server, "/hook", 1).await;

        outbox.stop();
        let left = slots.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(10)).await;
        assert!(left.is_empty(), "{left:?}");
        assert_eq!(arrivals(&server, "/hook").await, 1, "the delivery waiting for its slot sent nothing");
        let mut statuses = Vec::new();
        for id in ["g-first", "g-second"] {
            statuses.push(db.webhook_outbox_of(id).await.unwrap().unwrap().status);
        }
        statuses.sort();
        assert_eq!(statuses, ["delivered", "pending"]);
        assert_eq!(db.release_webhook_claims("replica-a", &[]).await.unwrap(), 1, "its lease goes back with the replica's others");
        assert_eq!(outbox.tick(&db, &ctx).await, 0, "a stopped dispatcher claims nothing");
    }

    /// A receiver that holds each request's answer (a 200) until `release`
    /// is notified once for it; `arrived` is notified as each request is in.
    async fn held_receiver() -> (String, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (arrived, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
        let (came, go) = (arrived.clone(), release.clone());
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let (came, go) = (came.clone(), go.clone());
                tokio::spawn(async move {
                    let mut req = Vec::new();
                    let mut buf = [0u8; 4096];
                    while let Ok(read @ 1..) = sock.read(&mut buf).await {
                        req.extend_from_slice(&buf[..read]);
                        let text = String::from_utf8_lossy(&req).to_ascii_lowercase();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let length = text[..end]
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            if req.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    came.notify_one();
                    go.notified().await;
                    let _ = sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
                });
            }
        });
        (format!("http://{addr}/hook"), arrived, release)
    }

    /// A stop gives back the leases of the deliveries this replica holds and
    /// isn't sending, so other replicas send those at their next tick, and
    /// keeps the lease of an attempt still on the wire until it ends (review
    /// 2026-11-15 round 2, finding 5): no other replica sends that delivery
    /// while this one may still be sending it.
    #[tokio::test]
    async fn a_stop_keeps_the_lease_of_an_attempt_still_sending() {
        let (url, arrived, release) = held_receiver().await;
        let (db, sqlite) = outbox_db().await;
        finished_job(&sqlite, "g-a", &url).await;
        finished_job(&sqlite, "g-b", &url).await;
        let slots = Arc::new(WebhookDeliveries::new(1, 1));
        let outbox = Arc::new(Outbox::new("replica-a", slots.clone(), vec![std::time::Duration::from_secs(10)]));
        assert_eq!(outbox.tick(&db, &plain_context()).await, 2, "one sends, one waits for the slot");
        tokio::time::timeout(std::time::Duration::from_secs(10), arrived.notified()).await.expect("no attempt arrived");

        outbox.stop();
        let released = outbox.release_claims(&db).await.unwrap();
        let mut holders = Vec::new();
        for id in ["g-a", "g-b"] {
            holders.push(db.webhook_outbox_of(id).await.unwrap().unwrap().claimed_by);
        }
        holders.sort();
        assert_eq!(holders, [None, Some("replica-a".to_string())], "the sending one's lease is kept, the other's given back");
        assert_eq!(released, ReleasedClaims { freed: 1, kept: 1 });

        release.notify_one();
        let left = slots.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(10)).await;
        assert!(left.is_empty(), "{left:?}");
        let mut statuses = Vec::new();
        for id in ["g-a", "g-b"] {
            statuses.push(db.webhook_outbox_of(id).await.unwrap().unwrap().status);
        }
        statuses.sort();
        assert_eq!(statuses, ["delivered", "pending"], "the attempt's outcome is recorded; the other waits, due, for any replica");
    }

    /// The loop every replica runs: it sends what is due without being
    /// asked, and ends at the stop.
    #[tokio::test]
    async fn the_dispatchers_loop_sends_due_deliveries_until_the_stop() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let (db, sqlite) = outbox_db().await;
        finished_job(&sqlite, "g-looped", &format!("{}/hook", server.uri())).await;
        let outbox = an_outbox("replica-loop", &[10]);
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let looping = outbox.clone().spawn(db.clone(), plain_context(), async move {
            let _ = stopped.await;
        });

        until_arrivals(&server, "/hook", 1).await;
        let _ = stop.send(());
        tokio::time::timeout(std::time::Duration::from_secs(10), looping).await.expect("the loop ended at the stop").unwrap();
        assert!(outbox.is_stopping(), "and it claims nothing more");
    }

    /// The outbox retries for about a day, as the docs say: yesterday's
    /// minutes first (10 s to 10 min), then 30 min, 1, 2, 4, 8 and 8 hours.
    /// 13 attempts, the last about 23 h 49 min after the first. This
    /// replica's dispatcher waits that schedule.
    #[test]
    fn the_outbox_retries_for_about_a_day() {
        let seconds: Vec<u64> = WEBHOOK_RETRY_DELAYS.iter().map(|d| d.as_secs()).collect();
        assert_eq!(seconds, [10, 30, 60, 120, 300, 600, 1800, 3600, 7200, 14_400, 28_800, 28_800], "the waits, in seconds");
        let all: u64 = seconds.iter().sum();
        assert_eq!(all, 23 * 3600 + 48 * 60 + 40, "the last attempt's wait from the first");
        assert_eq!(outbox().schedule, WEBHOOK_RETRY_DELAYS.to_vec());
    }
}
