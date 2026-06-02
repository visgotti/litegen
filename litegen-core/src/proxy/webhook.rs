use std::sync::Arc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tracing::{info, warn};

use crate::db::DatabaseStore;
use crate::types::{Generation, WebhookDelivery};

type HmacSha256 = Hmac<Sha256>;

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
}
