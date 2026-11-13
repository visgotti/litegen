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

/// Webhook deliveries running at once for one org, per replica.
pub const DELIVERIES_PER_ORG: usize = 16;
/// Webhook deliveries running at once in all, per replica.
pub const DELIVERIES_IN_ALL: usize = 256;
/// One org's deliveries waiting or running: every this many, a warning.
const ORG_BACKLOG_WARNING: usize = 1000;

/// The terminal webhooks this process is delivering. Each runs on its own
/// task once it holds a slot of its org's ([`DELIVERIES_PER_ORG`]) and then
/// one of the process's ([`DELIVERIES_IN_ALL`]): one org whose receiver is
/// slow or hangs (a delivery can take about 93 s: three attempts of up to
/// 30 s) holds at most its own share, never every slot, and its queue holds
/// none of the process's. The rest wait in memory, in order. A graceful stop
/// [drains](Self::drain) them within what is left of its grace; a crash loses
/// those still waiting or running.
pub struct WebhookDeliveries {
    all: tokio::sync::Semaphore,
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
    slots: tokio::sync::Semaphore,
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
            all: tokio::sync::Semaphore::new(bound(all)),
            per_org: bound(per_org),
            orgs: Default::default(),
            running: Default::default(),
            next: Default::default(),
            idle: tokio::sync::Notify::new(),
        }
    }

    /// Run `delivery`, the webhook of generation `generation_id` of `org`,
    /// on its own task once it holds its org's slot and then one of all.
    /// Returns at once.
    pub fn spawn(self: &Arc<Self>, org: &str, generation_id: &str, delivery: impl std::future::Future<Output = ()> + Send + 'static) {
        let number = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        locked(&self.running).insert(number, generation_id.to_string());
        let bucket = {
            let mut orgs = locked(&self.orgs);
            let bucket = orgs
                .entry(org.to_string())
                .or_insert_with(|| {
                    Arc::new(OrgDeliveries { slots: tokio::sync::Semaphore::new(self.per_org), count: Default::default() })
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
            let Ok(_org_slot) = ticket.bucket.slots.acquire().await else { return };
            let Ok(_slot) = ticket.owner.all.acquire().await else { return };
            delivery.await;
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

/// This process's webhook deliveries.
pub fn deliveries() -> &'static Arc<WebhookDeliveries> {
    static DELIVERIES: std::sync::OnceLock<Arc<WebhookDeliveries>> = std::sync::OnceLock::new();
    DELIVERIES.get_or_init(|| Arc::new(WebhookDeliveries::new(DELIVERIES_IN_ALL, DELIVERIES_PER_ORG)))
}

/// POST `payload` as JSON to `url`, signed with HMAC-SHA256.
///
/// The signature is included as `X-Litegen-Signature: sha256=<hex>`.
/// If `secret` is None the generation id is used as a placeholder secret so
/// the receiver can at least verify the id hasn't been tampered with.
///
/// Retries up to 3 times total (exponential back-off: 1 s, 2 s, 4 s).
/// Returns `Ok(())` if any attempt succeeds, `Err(String)` describing the
/// final failure after all retries are exhausted.
pub async fn dispatch_webhook(
    client: &reqwest::Client,
    url: &str,
    secret: Option<&str>,
    payload: &Generation,
) -> Result<(), String> {
    let body = match serde_json::to_string(payload) {
        Ok(b) => b,
        Err(e) => return Err(format!("serialise payload: {e}")),
    };

    let effective_secret = secret.unwrap_or(&payload.id);
    let signature = compute_signature(effective_secret, &body);
    let sig_header = format!("sha256={signature}");

    const MAX_ATTEMPTS: u32 = 3;
    let mut last_err = String::new();

    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            // exponential back-off: 1 s, 2 s
            let delay = std::time::Duration::from_secs(1 << (attempt - 1));
            tokio::time::sleep(delay).await;
        }

        info!(
            generation_id = %payload.id,
            url = %url,
            attempt = attempt + 1,
            "Dispatching webhook"
        );

        let result = client
            .post(url)
            .header("content-type", "application/json")
            .header("x-litegen-signature", &sig_header)
            .body(body.clone())
            .send()
            .await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                info!(
                    generation_id = %payload.id,
                    url = %url,
                    status = %resp.status(),
                    "Webhook delivered"
                );
                return Ok(());
            }
            Ok(resp) => {
                last_err = format!("HTTP {}", resp.status());
                warn!(
                    generation_id = %payload.id,
                    url = %url,
                    status = %resp.status(),
                    attempt = attempt + 1,
                    "Webhook non-2xx response"
                );
            }
            Err(e) => {
                last_err = e.to_string();
                warn!(
                    generation_id = %payload.id,
                    url = %url,
                    error = %e,
                    attempt = attempt + 1,
                    "Webhook request error"
                );
            }
        }
    }

    warn!(
        generation_id = %payload.id,
        url = %url,
        last_error = %last_err,
        "Webhook delivery failed after all retries"
    );
    Err(format!("webhook failed after {MAX_ATTEMPTS} attempts: {last_err}"))
}

/// POST `payload` as JSON to `url`, signed with HMAC-SHA256 — ONE attempt, no retries.
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

    client
        .post(url)
        .header("content-type", "application/json")
        .header("x-litegen-signature", &sig_header)
        .body(body)
        .send()
        .await
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
    let body = match serde_json::to_string(payload) {
        Ok(b) => b,
        Err(e) => return Err(format!("serialise payload: {e}")),
    };

    let effective_secret = secret.unwrap_or(&payload.id);
    let signature = compute_signature(effective_secret, &body);
    let sig_header = format!("sha256={signature}");

    const MAX_ATTEMPTS: u32 = 3;
    let mut last_err = String::new();

    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            let delay = std::time::Duration::from_secs(1 << (attempt - 1));
            tokio::time::sleep(delay).await;
        }

        info!(
            generation_id = %payload.id,
            url = %url,
            attempt = attempt + 1,
            "Dispatching webhook"
        );

        let result = client
            .post(url)
            .header("content-type", "application/json")
            .header("x-litegen-signature", &sig_header)
            .body(body.clone())
            .send()
            .await;

        match result {
            Ok(resp) => {
                let status_code = resp.status().as_u16() as i32;
                let success = resp.status().is_success();
                let response_body = kept_reply(resp).await;

                // Record this attempt.
                let delivery = WebhookDelivery {
                    id: format!("wh-{}", uuid::Uuid::new_v4()),
                    key_id: key_id.to_string(),
                    generation_id: payload.id.clone(),
                    url: url.to_string(),
                    attempt_number: (attempt + 1) as i32,
                    status_code: Some(status_code),
                    success,
                    response_body: response_body.clone(),
                    error_message: None,
                    payload_json: body.clone(),
                    created_at: chrono::Utc::now(),
                };
                if let Err(e) = db.insert_webhook_delivery(&delivery).await {
                    warn!(error = %e, "Failed to record webhook delivery");
                }

                if success {
                    info!(
                        generation_id = %payload.id,
                        url = %url,
                        status = status_code,
                        "Webhook delivered"
                    );
                    return Ok(());
                } else {
                    last_err = format!("HTTP {status_code}");
                    warn!(
                        generation_id = %payload.id,
                        url = %url,
                        status = status_code,
                        attempt = attempt + 1,
                        "Webhook non-2xx response"
                    );
                }
            }
            Err(e) => {
                last_err = e.to_string();

                // Record the network error.
                let delivery = WebhookDelivery {
                    id: format!("wh-{}", uuid::Uuid::new_v4()),
                    key_id: key_id.to_string(),
                    generation_id: payload.id.clone(),
                    url: url.to_string(),
                    attempt_number: (attempt + 1) as i32,
                    status_code: None,
                    success: false,
                    response_body: None,
                    error_message: Some(e.to_string()),
                    payload_json: body.clone(),
                    created_at: chrono::Utc::now(),
                };
                if let Err(db_err) = db.insert_webhook_delivery(&delivery).await {
                    warn!(error = %db_err, "Failed to record webhook delivery");
                }

                warn!(
                    generation_id = %payload.id,
                    url = %url,
                    error = %e,
                    attempt = attempt + 1,
                    "Webhook request error"
                );
            }
        }
    }

    warn!(
        generation_id = %payload.id,
        url = %url,
        last_error = %last_err,
        "Webhook delivery failed after all retries"
    );
    Err(format!("webhook failed after {MAX_ATTEMPTS} attempts: {last_err}"))
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

        let result = dispatch_webhook(&client, &url, None, &gen).await;
        assert!(result.is_ok(), "should succeed on third attempt: {:?}", result);
    }

    #[tokio::test]
    async fn gives_up_after_three_failures() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fail-hook"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let gen = make_generation("litegen-vid-fail-test");
        let url = format!("{}/fail-hook", server.uri());

        let result = dispatch_webhook(&client, &url, None, &gen).await;
        assert!(result.is_err(), "should fail after 3 attempts");
        server.verify().await;
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

        let result = dispatch_webhook_logged(&client, &url, None, &gen, db.clone(), "key-retry").await;
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
    /// retried before the drain returns, not dropped with the process.
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
            let _ = dispatch_webhook_logged(&reqwest::Client::new(), &url, None, &gen, db, "key-drain").await;
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
}
