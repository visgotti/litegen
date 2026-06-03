use std::sync::Arc;
use tracing::{info, warn};

use crate::db::DatabaseStore;
use crate::proxy::registry::ProviderRegistry;
use crate::providers::VideoGenerationHandle;
use crate::proxy::webhook::dispatch_webhook_logged;
use crate::types::GenerationStatus;

/// Run one polling iteration.
///
/// Queries up to 100 `pending`/`processing` rows, polls each provider,
/// updates the DB row, and dispatches webhooks on terminal transitions.
///
/// Made `pub(crate)` so tests can call it directly.
pub(crate) async fn poll_once(
    db: &Arc<dyn DatabaseStore>,
    registry: &Arc<ProviderRegistry>,
    http: &reqwest::Client,
    secrets_key: Option<[u8; 32]>,
) {
    let rows = match db.list_active_generations(100).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "poller: failed to list active generations");
            return;
        }
    };

    for gen in rows {
        // Resolve this generation's per-app BYO credential, if the app stored one.
        // Any failure (no secrets key, lookup error, decrypt/parse error) falls back
        // to `None` (→ the platform default global instance). The poller must never
        // crash on a single bad credential, so all errors are logged and swallowed.
        let app_creds = resolve_gen_credential(db, secrets_key, &gen).await;

        let provider = match registry
            .video_provider_for_request(&gen.provider, app_creds)
            .await
        {
            Some(p) => p,
            None => {
                warn!(
                    generation_id = %gen.id,
                    provider = %gen.provider,
                    "poller: provider not found, skipping"
                );
                continue;
            }
        };

        let provider_job_id = match &gen.provider_job_id {
            Some(id) => id.clone(),
            None => {
                warn!(generation_id = %gen.id, "poller: no provider_job_id, skipping");
                continue;
            }
        };

        let handle = VideoGenerationHandle {
            provider_job_id,
            provider: gen.provider.clone(),
            model: gen.model.clone(),
        };

        let poll = match provider.poll_status(&handle).await {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    generation_id = %gen.id,
                    error = %e,
                    "poller: poll_status failed"
                );
                continue;
            }
        };

        let is_terminal = matches!(
            poll.status,
            GenerationStatus::Completed | GenerationStatus::Failed | GenerationStatus::Cancelled
        );
        let completed_at = if is_terminal {
            Some(chrono::Utc::now())
        } else {
            None
        };

        let status_str = poll.status.to_string();
        if let Err(e) = db.update_generation_status(
            &gen.id,
            &status_str,
            poll.progress as i32,
            poll.video_url.as_deref(),
            poll.error.as_deref(),
            completed_at,
        ).await {
            warn!(generation_id = %gen.id, error = %e, "poller: update_generation_status failed");
            continue;
        }

        info!(
            generation_id = %gen.id,
            status = %status_str,
            progress = %poll.progress,
            "poller: updated generation status"
        );

        // Dispatch webhook if terminal and the key has a webhook_url
        if is_terminal {
            if let Some(key_id) = gen.key_id {
                let db2 = db.clone();
                let http2 = http.clone();
                let gen_id = gen.id.clone();
                // Build the updated generation for the webhook payload
                let updated_gen = crate::types::Generation {
                    status: poll.status,
                    progress: poll.progress as i32,
                    result_url: poll.video_url,
                    error_message: poll.error,
                    completed_at,
                    ..gen
                };

                tokio::spawn(async move {
                    match db2.get_api_key(&key_id).await {
                        Ok(Some(key)) if key.webhook_url.is_some() => {
                            let url = key.webhook_url.unwrap();
                            let secret = key.key_hash.clone();
                            let key_id_str = key_id.to_string();
                            if let Err(e) = dispatch_webhook_logged(
                                &http2,
                                &url,
                                Some(&secret),
                                &updated_gen,
                                db2,
                                &key_id_str,
                            ).await {
                                warn!(generation_id = %gen_id, error = %e, "webhook dispatch failed");
                            }
                        }
                        Ok(Some(_)) => {}  // no webhook_url
                        Ok(None) => {
                            warn!(generation_id = %gen_id, key_id = %key_id, "key not found for webhook");
                        }
                        Err(e) => {
                            warn!(generation_id = %gen_id, error = %e, "failed to lookup key for webhook");
                        }
                    }
                });
            }
        }
    }
}

/// Resolve a single generation's stored per-app BYO credential, decrypted.
///
/// Returns `None` (→ platform default) when the generation has no app, no secrets
/// key is configured, the app stored no credential, or anything fails to look up /
/// decrypt / parse. Errors are logged but never propagated — the poller must keep
/// running across a bad credential on one row.
async fn resolve_gen_credential(
    db: &Arc<dyn DatabaseStore>,
    secrets_key: Option<[u8; 32]>,
    gen: &crate::types::Generation,
) -> Option<crate::providers::ProviderCredentials> {
    let app_id = gen.app_id.as_deref()?;
    let key = secrets_key?;
    match db.get_provider_credential(app_id, &gen.provider).await {
        Ok(Some((ct, nonce))) => match crate::auth::secrets::decrypt(&key, &ct, &nonce) {
            Ok(plaintext) => match serde_json::from_slice::<serde_json::Value>(&plaintext) {
                Ok(val) => Some(crate::providers::ProviderCredentials::from_json(&val)),
                Err(e) => {
                    warn!(
                        generation_id = %gen.id,
                        provider = %gen.provider,
                        error = %e,
                        "poller: stored provider credential is corrupt, using platform default"
                    );
                    None
                }
            },
            Err(e) => {
                warn!(
                    generation_id = %gen.id,
                    provider = %gen.provider,
                    error = %e,
                    "poller: failed to decrypt provider credential, using platform default"
                );
                None
            }
        },
        Ok(None) => None,
        Err(e) => {
            warn!(
                generation_id = %gen.id,
                provider = %gen.provider,
                error = %e,
                "poller: provider credential lookup failed, using platform default"
            );
            None
        }
    }
}

/// Spawn a background task that polls every 5 seconds. The returned
/// JoinHandle can be awaited at shutdown to drain in-flight work; cancellation
/// happens via the `shutdown` future (e.g. tokio-util's CancellationToken).
pub fn spawn_poller(
    db: Arc<dyn DatabaseStore>,
    registry: Arc<ProviderRegistry>,
    http: reqwest::Client,
    secrets_key: Option<[u8; 32]>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    poll_once(&db, &registry, &http, secrets_key).await;
                }
                _ = &mut shutdown => {
                    tracing::info!("poller received shutdown signal, exiting loop");
                    return;
                }
            }
        }
    })
}

// ─── Poller integration tests ─────────────────────────────────────────────────

#[cfg(test)]
mod poller_tests {
    use super::*;
    use std::sync::Arc;
    use crate::db::sqlite::SqliteDatabase;
    use crate::db::DatabaseStore;
    use crate::proxy::registry::ProviderRegistry;
    use crate::providers::{ProviderInstanceConfig, VideoProvider};
    use crate::providers::video::mock::MockVideoProvider;

    async fn in_memory_db() -> Arc<SqliteDatabase> {
        Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory sqlite"))
    }

    async fn make_registry() -> Arc<ProviderRegistry> {
        let reg = Arc::new(ProviderRegistry::new());
        let mut vp = MockVideoProvider::new();
        vp.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: String::new(),
            api_keys: vec![],
            api_base: None,
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });
        reg.register_mock_video(Arc::new(vp)).await;
        reg
    }

    #[tokio::test]
    async fn poll_once_flips_pending_to_completed() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_registry().await;
        let client = reqwest::Client::new();

        // Insert a pending generation
        db.insert_generation(
            "litegen-vid-poll-test-1",
            None,
            "mock/video-gen",
            "mock",
            "video",
            Some("mock-video-job-1"),
            0.0,
            None,
            None,
        ).await.unwrap();

        // Verify it's pending
        let before = db.get_generation("litegen-vid-poll-test-1").await.unwrap().unwrap();
        assert_eq!(before.status, crate::types::GenerationStatus::Pending);

        // Run one poller iteration
        poll_once(&db, &registry, &client, None).await;

        // Should now be completed
        let after = db.get_generation("litegen-vid-poll-test-1").await.unwrap().unwrap();
        assert_eq!(after.status, crate::types::GenerationStatus::Completed);
        assert!(after.result_url.is_some());
        assert_eq!(after.progress, 100);
    }

    #[tokio::test]
    async fn poll_once_skips_unknown_provider() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = Arc::new(ProviderRegistry::new()); // no providers registered
        let client = reqwest::Client::new();

        db.insert_generation(
            "litegen-vid-poll-skip-1",
            None,
            "unknown/model",
            "nonexistent",
            "video",
            Some("job-x"),
            0.0,
            None,
            None,
        ).await.unwrap();

        poll_once(&db, &registry, &client, None).await;

        // Status should remain pending (provider not found, skipped)
        let row = db.get_generation("litegen-vid-poll-skip-1").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Pending);
    }

    #[tokio::test]
    async fn poll_once_dispatches_webhook_on_terminal() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/wh"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let db: Arc<dyn DatabaseStore> = in_memory_db().await;

        // Create a key with a webhook_url
        let key = db.create_api_key(
            "wh-key",
            "wh-hash-poll",
            "lg-wh",
            None, None,
            "generate,read",
            Some(&format!("{}/wh", server.uri())),
        ).await.unwrap();

        let registry = make_registry().await;
        let client = reqwest::Client::new();

        db.insert_generation(
            "litegen-vid-wh-poll-1",
            Some(&key.id),
            "mock/video-gen",
            "mock",
            "video",
            Some("mock-video-job-1"),
            0.0,
            None,
            None,
        ).await.unwrap();

        poll_once(&db, &registry, &client, None).await;

        // Give the spawned webhook task a moment
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        server.verify().await;
    }
}
