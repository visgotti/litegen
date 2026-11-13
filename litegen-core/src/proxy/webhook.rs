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
/// on every retry. A single-tenant operator may target internal hosts
/// (`validate_tenant_webhook_url`).
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
                let response_body = resp.text().await.ok();

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
        // A single-tenant operator may target internal hosts (validate_tenant_webhook_url).
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
