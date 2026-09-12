use std::sync::Arc;
use tracing::{info, warn};

use crate::db::DatabaseStore;
use crate::proxy::registry::ProviderRegistry;
use crate::providers::VideoGenerationHandle;
use crate::proxy::webhook::dispatch_webhook_logged;
use crate::types::GenerationStatus;

/// Maximum time a generation may sit in `pending`/`processing` before the
/// poller reaps it as `failed`. Without this, a row that never reports a
/// terminal status — revoked key, purged job, removed provider, missing job id,
/// or an endlessly-retryable upstream error — sits at the head of the
/// oldest-first active window forever and starves every newer generation from
/// ever being polled. Set generously so a genuinely slow provider job is never
/// reaped prematurely.
const MAX_ACTIVE_AGE_SECS: i64 = 2 * 60 * 60; // 2 hours

/// Terminalise a stuck generation as `failed` so it leaves the active window.
///
/// Guarded on the row still being in flight: a reap decision is made from a row
/// list fetched at the top of the tick, and `GET /v1/models3d/{id}` can have
/// terminalised the row since. Without the guard a generation that already
/// returned a mesh to its caller is flipped to `failed` with `result_url` NULL.
async fn reap_generation(db: &Arc<dyn DatabaseStore>, gen_id: &str, reason: &str) {
    match db
        .update_generation_if_active(
            gen_id, "failed", 0, None, Some(reason), Some(chrono::Utc::now()), None,
        )
        .await
    {
        Ok(w) if w.owns_outcome() => {}
        Ok(_) => {
            // Another observer terminalised it first; it owns the log update.
            info!(generation_id = %gen_id, "poller: not reaping an already-terminal generation");
            return;
        }
        Err(e) => {
            // Still active, so the next tick reaps it again — and marks the log then.
            warn!(generation_id = %gen_id, error = %e, "poller: failed to reap stuck generation");
            return;
        }
    }
    // A reap is a terminal transition like any other: the request log (same
    // id) must not keep reading `pending` for a generation that has failed.
    if let Err(e) = db.update_request_log_status(gen_id, "failed", Some(reason)).await {
        warn!(generation_id = %gen_id, error = %e, "poller: failed to mark the reaped request log failed");
    }
}

/// Result of polling one row, normalised across modalities so the reaper, DB
/// update, and webhook dispatch stay modality-agnostic. Video never populates
/// `assets` (it has nothing to re-host); 3D never leaves it `None` on a
/// completed row (see the mesh guard in the `model3d` branch below).
struct PollOutcome {
    status: GenerationStatus,
    progress: u8,
    result_url: Option<String>,
    error: Option<String>,
    /// 3D only: the full re-hosted asset list, persisted to `metadata.assets`.
    assets: Option<serde_json::Value>,
}

/// Shared poll-error policy: a non-retryable upstream error fails the row now
/// (the job is gone and retrying cannot recover it); a retryable one is left
/// alone unless the row is past MAX_ACTIVE_AGE.
async fn handle_poll_error(
    db: &Arc<dyn DatabaseStore>,
    gen: &crate::types::Generation,
    e: &crate::providers::ProviderError,
    over_age: bool,
) {
    if !e.is_retryable() {
        warn!(generation_id = %gen.id, error = %e, "poller: non-retryable poll error, marking failed");
        reap_generation(db, &gen.id, &e.to_string()).await;
    } else if over_age {
        warn!(generation_id = %gen.id, error = %e, "poller: poll failing past max age, reaping");
        reap_generation(db, &gen.id, "timed out awaiting provider").await;
    } else {
        warn!(generation_id = %gen.id, error = %e, "poller: poll_status failed, will retry");
    }
}

/// Run one polling iteration.
///
/// Queries up to 100 `pending`/`processing` rows, polls each provider,
/// updates the DB row, and dispatches webhooks on terminal transitions.
///
/// Made `pub(crate)` so tests can call it directly.
pub(crate) async fn poll_once(
    db: &Arc<dyn DatabaseStore>,
    registry: &Arc<ProviderRegistry>,
    secrets_key: Option<[u8; 32]>,
    mode: crate::config::Mode,
    model3d_store: &Arc<dyn crate::proxy::storage::ImageStorage>,
) {
    let rows = match db.list_active_generations(100).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "poller: failed to list active generations");
            return;
        }
    };

    // One hardened (no-redirect) webhook client per tick, cloned into each
    // dispatch task — `reqwest::Client` clones share a connection pool, so this
    // avoids building (and discarding) a fresh pool per terminal generation.
    let wh_client = crate::util::ssrf::no_redirect_client();

    for gen in rows {
        // Age of this row in the active window. Rows that can never advance are
        // reaped once they exceed MAX_ACTIVE_AGE so they stop starving the
        // oldest-first poll window (see reap_generation).
        let over_age =
            (chrono::Utc::now() - gen.created_at).num_seconds() > MAX_ACTIVE_AGE_SECS;

        // Resolve this generation's per-app BYO credential, if the app stored one.
        // Any failure (no secrets key, lookup error, decrypt/parse error) falls back
        // to `None` (→ the platform default global instance). The poller must never
        // crash on a single bad credential, so all errors are logged and swallowed.
        let app_creds = resolve_gen_credential(db, secrets_key, &gen).await;

        let outcome: Option<PollOutcome> = match gen.media_type.as_str() {
            "video" => {
                let provider = match registry
                    .video_provider_for_request(&gen.provider, app_creds)
                    .await
                {
                    Some(p) => p,
                    None => {
                        if over_age {
                            warn!(generation_id = %gen.id, provider = %gen.provider, "poller: provider not found past max age, reaping");
                            reap_generation(db, &gen.id, "provider no longer configured").await;
                        } else {
                            warn!(generation_id = %gen.id, provider = %gen.provider, "poller: provider not found, skipping");
                        }
                        continue;
                    }
                };

                let provider_job_id = match &gen.provider_job_id {
                    Some(id) => id.clone(),
                    None => {
                        if over_age {
                            warn!(generation_id = %gen.id, "poller: no provider_job_id past max age, reaping");
                            reap_generation(db, &gen.id, "no provider job id was recorded").await;
                        } else {
                            warn!(generation_id = %gen.id, "poller: no provider_job_id, skipping");
                        }
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
                        handle_poll_error(db, &gen, &e, over_age).await;
                        continue;
                    }
                };

                Some(PollOutcome {
                    status: poll.status,
                    progress: poll.progress,
                    result_url: poll.video_url,
                    error: poll.error,
                    assets: None,
                })
            }
            "model3d" => {
                let provider = match registry
                    .model3d_provider_for_request(&gen.provider, app_creds)
                    .await
                {
                    Some(p) => p,
                    None => {
                        if over_age {
                            warn!(generation_id = %gen.id, provider = %gen.provider, "poller: 3d provider not found past max age, reaping");
                            reap_generation(db, &gen.id, "provider no longer configured").await;
                        } else {
                            warn!(generation_id = %gen.id, provider = %gen.provider, "poller: 3d provider not found, skipping");
                        }
                        continue;
                    }
                };
                let provider_job_id = match gen.provider_job_id.clone() {
                    Some(id) => id,
                    None => {
                        if over_age {
                            warn!(generation_id = %gen.id, "poller: no provider_job_id past max age, reaping");
                            reap_generation(db, &gen.id, "no provider job id was recorded").await;
                        } else {
                            warn!(generation_id = %gen.id, "poller: no provider_job_id, skipping");
                        }
                        continue;
                    }
                };
                let handle = crate::providers::Model3dGenerationHandle {
                    provider_job_id,
                    provider: gen.provider.clone(),
                    model: gen.model.clone(),
                    stage_context: gen
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("stage_context").cloned()),
                };
                let poll = match provider.poll_status(&handle).await {
                    Ok(p) => p,
                    Err(e) => {
                        handle_poll_error(db, &gen, &e, over_age).await;
                        continue;
                    }
                };

                // Re-host NOW, in the same tick that observed completion: several
                // vendors expire their download URLs within minutes, so the bytes
                // `poll` already carries must be moved to litegen-owned storage
                // before the next poll cycle rather than deferred.
                let assets = if poll.status == GenerationStatus::Completed && !poll.files.is_empty() {
                    // Per-app BYO bucket (and its configured prefix) if the app set
                    // one up, else the shared global store passed in by the caller.
                    let (store, prefix) = match gen.app_id.as_deref() {
                        Some(app_id) => crate::api::handlers::resolve_app_model3d_store(
                            db, secrets_key, app_id,
                        )
                        .await
                        .unwrap_or_else(|| (model3d_store.clone(), None)),
                        None => (model3d_store.clone(), None),
                    };
                    match crate::proxy::router::rehost_model3d_files(
                        &store, prefix.as_deref(), &gen.id, &poll.files,
                    ).await {
                        Ok(a) => Some(a),
                        Err(e) => {
                            // Storage failure is retryable — leave the row
                            // non-terminal so the next tick tries again rather
                            // than reporting a completed generation with no mesh.
                            warn!(generation_id = %gen.id, error = %e, "poller: 3d asset upload failed, will retry");
                            continue;
                        }
                    }
                } else {
                    None
                };

                // Contract: a completed generation ALWAYS carries a mesh. If the
                // provider reported success without one, that is a failure, not a
                // degraded success — a paid generation the caller cannot use. This
                // mirrors `get_3d_status` exactly so the two paths that can observe
                // completion never disagree about the same job.
                //
                // This is routed through the normal terminal `PollOutcome` tail
                // (status: Failed) rather than `reap_generation` + `continue`.
                // `reap_generation` is for rows that are stuck or unreachable —
                // no provider configured, no job id recorded, over-age with no
                // terminal status — which genuinely have no provider verdict to
                // report. A provider that answered `Completed` without a mesh DID
                // give a verdict; it's just a failed one. Treating it as a normal
                // terminal `Failed` (rather than a reap) is the more correct
                // classification on its own, and it has the side effect of
                // running the same webhook-dispatch path a provider-reported
                // `Failed` gets — a caller watching webhooks, not polling, must
                // still learn this generation failed. Do not "simplify" this back
                // to `reap_generation`; that would silently drop the webhook.
                // GLB first, matching `Model3dGenerationResponse::mesh()` — with
                // several containers delivered, `result_url` must not depend on
                // which one the adapter happened to push first.
                let mesh_url = assets.as_ref().and_then(|a| {
                    let meshes = || a.iter().filter(|x| x.kind == crate::types::Model3dAssetKind::Mesh);
                    meshes()
                        .find(|x| x.format.eq_ignore_ascii_case(crate::types::DEFAULT_MODEL3D_FORMAT))
                        .or_else(|| meshes().next())
                        .map(|m| m.url.clone())
                });
                let mesh_missing = poll.status == GenerationStatus::Completed && mesh_url.is_none();

                // The second half of the same contract: every container the
                // caller was promised at submit must be present, not merely one
                // mesh. Read back off the row because the poller cannot see the
                // router's in-flight job map. Same reasoning as the mesh guard
                // above — a partial delivery is a paid generation the caller
                // cannot use, so it takes the terminal `Failed` tail (and thus
                // the webhook) rather than being reported as success.
                let requested = crate::api::handlers::row_requested_formats(&gen);
                let missing = if poll.status == GenerationStatus::Completed {
                    let delivered: Vec<String> = assets.as_ref().map(|a| {
                        a.iter()
                            .filter(|x| x.kind == crate::types::Model3dAssetKind::Mesh)
                            .map(|x| x.format.to_ascii_lowercase())
                            .collect()
                    }).unwrap_or_default();
                    crate::types::missing_model3d_formats(&requested, &delivered)
                } else {
                    Vec::new()
                };

                if mesh_missing || !missing.is_empty() {
                    let reason = if mesh_missing {
                        "provider reported success without a mesh asset".to_string()
                    } else {
                        format!(
                            "provider did not return the requested format(s): {}",
                            missing.join(", ")
                        )
                    };
                    warn!(generation_id = %gen.id, reason = %reason, "poller: failing the 3d row");
                    Some(PollOutcome {
                        status: GenerationStatus::Failed,
                        progress: poll.progress,
                        result_url: None,
                        error: Some(reason),
                        assets: None,
                    })
                } else {
                    Some(PollOutcome {
                        status: poll.status,
                        progress: poll.progress,
                        result_url: mesh_url,
                        error: poll.error,
                        // `requested_formats` rides along because the terminal
                        // write REPLACES metadata — dropping it here would erase
                        // what submit recorded and leave every later read of this
                        // row enforcing only the GLB floor.
                        assets: assets.map(|a| serde_json::json!({
                            "assets": a,
                            "requested_formats": requested,
                        })),
                    })
                }
            }
            other => {
                // Only video and 3D are async today. An `image` row here means
                // something upstream is wrong; log and skip rather than reap.
                warn!(generation_id = %gen.id, media_type = %other, "poller: no async provider for media type, skipping");
                continue;
            }
        };
        let Some(outcome) = outcome else { continue };

        let is_terminal = matches!(
            outcome.status,
            GenerationStatus::Completed | GenerationStatus::Failed | GenerationStatus::Cancelled
        );

        // A row that is reachable and polling cleanly but still hasn't reached a
        // terminal state after MAX_ACTIVE_AGE is treated as stuck and reaped.
        if !is_terminal && over_age {
            warn!(generation_id = %gen.id, "poller: still non-terminal past max age, reaping");
            reap_generation(db, &gen.id, "timed out: no terminal status within max active age").await;
            continue;
        }
        let completed_at = if is_terminal {
            Some(chrono::Utc::now())
        } else {
            None
        };

        let status_str = outcome.status.to_string();
        // ONE guarded, atomic write: status + result_url + assets together, and
        // only while the row is still in flight. `GET /v1/models3d/{id}` is the
        // other terminal observer and the SDK polls it faster than this loop
        // ticks, so an unguarded write here would overturn a decided outcome
        // (resurrect a cancel, revert a completed row to `processing`) and a
        // split write could leave a row `completed` with no mesh.
        let write = match db.update_generation_if_active(
            &gen.id,
            &status_str,
            outcome.progress as i32,
            outcome.result_url.as_deref(),
            outcome.error.as_deref(),
            completed_at,
            outcome.assets.as_ref(),
        ).await {
            Ok(w) => w,
            Err(e) => {
                warn!(generation_id = %gen.id, error = %e, "poller: update_generation_if_active failed");
                continue;
            }
        };
        if !write.owns_outcome() {
            // The other observer terminalised this row between our
            // `list_active_generations` and now. It owns the log update and the
            // webhook; doing either here would duplicate them.
            info!(generation_id = %gen.id, "poller: row already terminal, leaving it to the observer that wrote it");
            continue;
        }

        // The submit handler logged this request `pending` under the same id;
        // move that row to the outcome too, or `/v1/logs` reads `pending`
        // forever. Best-effort: the generation row above is the source of truth.
        if is_terminal {
            if let Err(e) = db
                .update_request_log_status(&gen.id, &status_str, outcome.error.as_deref())
                .await
            {
                warn!(generation_id = %gen.id, error = %e, "poller: failed to update request log status");
            }
        }

        info!(
            generation_id = %gen.id,
            status = %status_str,
            progress = %outcome.progress,
            "poller: updated generation status"
        );

        // Dispatch webhook if terminal and the key has a webhook_url. Reaching
        // here means THIS observer wrote the terminal row (`wrote` above), so
        // the delivery fires exactly once whichever path won the race.
        if is_terminal {
            // The webhook payload must carry the assets the same row now has:
            // building it from the pre-poll snapshot with `..gen` shipped
            // `metadata: None`, so a webhook consumer got `result_url` and no
            // preview/polycount while `GET /v1/generations/{id}` had both.
            let metadata = outcome.assets.clone().or_else(|| gen.metadata.clone());
            let updated_gen = crate::types::Generation {
                status: outcome.status,
                progress: outcome.progress as i32,
                result_url: outcome.result_url,
                error_message: outcome.error,
                completed_at,
                metadata,
                ..gen
            };
            dispatch_terminal_webhook(db, &wh_client, mode, updated_gen).await;
        }
    }
}

/// Deliver the terminal-generation webhook for `gen`, if its key has one.
///
/// Extracted from the poller because the poller is no longer the only terminal
/// observer: `GET /v1/models3d/{id}` terminalises a 3D row too, and whenever it
/// won the race the row never appeared in `list_active_generations` again, so
/// the only dispatch site never ran and the caller got no delivery at all.
/// Both callers dispatch only when their guarded UPDATE actually wrote the row,
/// which is what makes "at most once" hold.
///
/// Spawns and returns immediately: a webhook endpoint's latency must not stall
/// a poll tick or an HTTP response.
pub(crate) async fn dispatch_terminal_webhook(
    db: &Arc<dyn DatabaseStore>,
    wh_client: &reqwest::Client,
    mode: crate::config::Mode,
    gen: crate::types::Generation,
) {
    let Some(key_id) = gen.key_id else { return };
    let db2 = db.clone();
    let wh_client = wh_client.clone();
    let gen_id = gen.id.clone();

    tokio::spawn(async move {
        match db2.get_api_key(&key_id).await {
            Ok(Some(key)) if key.webhook_url.is_some() => {
                let url = key.webhook_url.unwrap();
                // SSRF re-validation (hosted only): re-check the URL at
                // dispatch time so a key whose webhook_url predates the
                // create/patch validation — or a host that now resolves
                // to an internal address — can't be used as an SSRF
                // vector. Single-tenant operators may target internal
                // hosts, so they're not re-validated.
                if mode == crate::config::Mode::Hosted {
                    if let Err(reason) = crate::util::ssrf::validate_public_url(&url).await {
                        warn!(generation_id = %gen_id, reason = %reason, "skipping webhook dispatch: disallowed url");
                        return;
                    }
                }
                let secret = key.key_hash.clone();
                let key_id_str = key_id.to_string();
                // SSRF hardening: dispatch with the no-redirect client so a
                // public webhook host can't 3xx-redirect the POST into an
                // internal target.
                if let Err(e) = dispatch_webhook_logged(
                    &wh_client,
                    &url,
                    Some(&secret),
                    &gen,
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

/// Resolve a single generation's stored per-org BYO credential, decrypted.
///
/// Returns `None` (→ platform default) when the generation has no org, no secrets
/// key is configured, the org stored no credential, or anything fails to look up /
/// decrypt / parse. Errors are logged but never propagated — the poller must keep
/// running across a bad credential on one row.
async fn resolve_gen_credential(
    db: &Arc<dyn DatabaseStore>,
    secrets_key: Option<[u8; 32]>,
    gen: &crate::types::Generation,
) -> Option<crate::providers::ProviderCredentials> {
    let org_id = gen.org_id.as_deref()?;
    let key = secrets_key?;
    match db.get_org_provider_credential(org_id, &gen.provider).await {
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
    secrets_key: Option<[u8; 32]>,
    mode: crate::config::Mode,
    model3d_store: Arc<dyn crate::proxy::storage::ImageStorage>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    poll_once(&db, &registry, secrets_key, mode, &model3d_store).await;
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
        let store = test_3d_store();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

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

        let store = test_3d_store();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

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

        let store = test_3d_store();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        // Give the spawned webhook task a moment
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        server.verify().await;
    }

    // ─── Reaper: stuck rows must leave the active window ──────────────────────

    use crate::capabilities::ModelSchema;
    use crate::providers::{
        HealthCheckResult, ProviderError, VideoExtras,
        VideoGenerationHandle, VideoGenerationPollResult,
    };
    use crate::proxy::materializer::MaterializedRequest;
    use crate::types::{BaseGenerationRequest, CostEstimate, GenerationStatus, VideoGenerationRequest};

    enum PollBehavior {
        /// Never reaches a terminal state — always reports "processing".
        StuckProcessing,
        /// Returns a non-retryable upstream error (e.g. HTTP 401 / job purged).
        TerminalError,
    }

    struct ScriptedVideoProvider {
        behavior: PollBehavior,
    }

    #[async_trait::async_trait]
    impl VideoProvider for ScriptedVideoProvider {
        fn name(&self) -> &str { "mock" }
        fn configure(&mut self, _c: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }

        async fn generate(
            &self,
            _m: &ModelSchema,
            _b: &BaseGenerationRequest,
            _e: &VideoExtras,
            _mat: &MaterializedRequest,
        ) -> Result<VideoGenerationHandle, ProviderError> {
            unimplemented!("poller tests never call generate")
        }

        async fn poll_status(
            &self,
            _h: &VideoGenerationHandle,
        ) -> Result<VideoGenerationPollResult, ProviderError> {
            match self.behavior {
                PollBehavior::StuckProcessing => Ok(VideoGenerationPollResult {
                    status: GenerationStatus::Processing,
                    progress: 10,
                    video_url: None,
                    video_data: None,
                    content_type: None,
                    error: None,
                    metadata: Default::default(),
                }),
                PollBehavior::TerminalError => Err(ProviderError::RequestFailed {
                    message: "poll returned HTTP 401: invalid api key".into(),
                    status_code: Some(401),
                    provider_error: None,
                    retryable: false,
                }),
            }
        }

        async fn estimate_cost(
            &self,
            _m: &ModelSchema,
            _r: &VideoGenerationRequest,
        ) -> Result<CostEstimate, ProviderError> {
            unimplemented!("poller tests never estimate cost")
        }

        async fn health_check(&self) -> HealthCheckResult {
            unimplemented!("poller tests never health-check")
        }
    }

    async fn registry_with(behavior: PollBehavior) -> Arc<ProviderRegistry> {
        let reg = Arc::new(ProviderRegistry::new());
        reg.register_mock_video(Arc::new(ScriptedVideoProvider { behavior })).await;
        reg
    }

    #[tokio::test]
    async fn poll_once_fails_generation_on_non_retryable_error() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = registry_with(PollBehavior::TerminalError).await;
        db.insert_generation(
            "litegen-vid-dead", None, "mock/video-gen", "mock", "video",
            Some("job-dead"), 0.0, None, None,
        ).await.unwrap();

        let store = test_3d_store();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        let row = db.get_generation("litegen-vid-dead").await.unwrap().unwrap();
        assert_eq!(
            row.status,
            GenerationStatus::Failed,
            "a non-retryable poll error must terminalise the row so it leaves the active window"
        );
    }

    #[tokio::test]
    async fn poll_once_reaps_overage_stuck_generation() {
        let concrete = in_memory_db().await;
        let db: Arc<dyn DatabaseStore> = concrete.clone();
        let registry = registry_with(PollBehavior::StuckProcessing).await;
        db.insert_generation(
            "litegen-vid-stuck", None, "mock/video-gen", "mock", "video",
            Some("job-stuck"), 0.0, None, None,
        ).await.unwrap();
        // Backdate the row well past the max active age.
        sqlx::query("UPDATE generations SET created_at = datetime('now','-6 hours') WHERE id = ?")
            .bind("litegen-vid-stuck")
            .execute(concrete.pool())
            .await
            .unwrap();

        let store = test_3d_store();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        let row = db.get_generation("litegen-vid-stuck").await.unwrap().unwrap();
        assert_eq!(
            row.status,
            GenerationStatus::Failed,
            "a generation stuck past the max active age must be reaped so it stops starving the poll window"
        );
    }

    #[tokio::test]
    async fn poll_once_does_not_reap_fresh_processing_generation() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = registry_with(PollBehavior::StuckProcessing).await;
        db.insert_generation(
            "litegen-vid-fresh", None, "mock/video-gen", "mock", "video",
            Some("job-fresh"), 0.0, None, None,
        ).await.unwrap();

        let store = test_3d_store();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        let row = db.get_generation("litegen-vid-fresh").await.unwrap().unwrap();
        assert_eq!(
            row.status,
            GenerationStatus::Processing,
            "a fresh in-flight generation must not be reaped by the age cap"
        );
    }

    // ─── media_type dispatch: model3d ──────────────────────────────────────

    use crate::providers::model3d::mock::MockModel3dProvider;
    use crate::providers::Model3dProvider;
    use crate::proxy::storage::{ImageStorage, LocalModel3dStorage};

    async fn make_3d_registry() -> Arc<ProviderRegistry> {
        let reg = Arc::new(ProviderRegistry::new());
        let mut p = MockModel3dProvider::new();
        p.configure(ProviderInstanceConfig::default());
        reg.register_mock_model3d(Arc::new(p)).await;
        reg
    }

    fn test_3d_store() -> Arc<dyn ImageStorage> {
        Arc::new(LocalModel3dStorage::new("https://assets.test".into()))
    }

    /// Submit through the mock provider so the row's provider_job_id refers to a
    /// job the provider actually knows about.
    /// A 3D provider stub that always reports `Completed` with a preview-only
    /// file list (no `Mesh` kind) — exercises the completed-without-mesh
    /// contract deterministically, unlike `MockModel3dProvider`, which never
    /// produces that shape. A preview-only (not merely empty) list also proves
    /// the guard checks for `kind: Mesh` specifically, not just non-emptiness.
    struct ScriptedModel3dProvider;

    #[async_trait::async_trait]
    impl Model3dProvider for ScriptedModel3dProvider {
        fn name(&self) -> &str { "mock" }
        fn configure(&mut self, _c: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }

        async fn generate(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _b: &crate::types::BaseGenerationRequest,
            _e: &crate::providers::Model3dExtras,
            _mat: &crate::proxy::materializer::MaterializedRequest,
        ) -> Result<crate::providers::Model3dGenerationHandle, crate::providers::ProviderError> {
            unimplemented!("poller tests never call generate")
        }

        async fn poll_status(
            &self,
            _h: &crate::providers::Model3dGenerationHandle,
        ) -> Result<crate::providers::Model3dGenerationPollResult, crate::providers::ProviderError> {
            Ok(crate::providers::Model3dGenerationPollResult {
                status: crate::types::GenerationStatus::Completed,
                progress: 100,
                files: vec![crate::providers::Model3dFile {
                    kind: crate::types::Model3dAssetKind::Preview,
                    format: "png".into(),
                    content_type: "image/png".into(),
                    bytes: b"fake-preview-png".to_vec(),
                    polycount: None,
                    width: Some(256),
                    height: Some(256),
                }],
                error: None,
                metadata: Default::default(),
            })
        }

        async fn estimate_cost(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _r: &crate::types::Model3dGenerationRequest,
        ) -> Result<crate::types::CostEstimate, crate::providers::ProviderError> {
            unimplemented!("poller tests never estimate cost")
        }

        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            unimplemented!("poller tests never health-check")
        }
    }

    async fn registry_with_3d_mesh_missing() -> Arc<ProviderRegistry> {
        let reg = Arc::new(ProviderRegistry::new());
        reg.register_mock_model3d(Arc::new(ScriptedModel3dProvider)).await;
        reg
    }

    async fn submit_mock_3d(model: &str, prompt: &str) -> String {
        use crate::capabilities::{MediaType, ModelCapabilityFlags, ModelPricing, PromptSpec};
        let mut p = MockModel3dProvider::new();
        p.configure(ProviderInstanceConfig::default());
        let schema = crate::capabilities::ModelSchema {
            id: model.into(), provider: "mock".into(), media_type: MediaType::Model3d,
            display_name: model.into(), description: String::new(),
            pricing: ModelPricing { base_cost_usd: 0.0, variable_pricing: None },
            capabilities: ModelCapabilityFlags { text_to_3d: true, ..Default::default() },
            prompt: PromptSpec { required: true, min_length: None, max_length: None },
            params: Default::default(), ref_inputs: None, extra_allowlist: vec![], tags: vec![],
        };
        let base = crate::types::BaseGenerationRequest {
            prompt: prompt.into(), model: model.into(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        };
        let extras = crate::providers::Model3dExtras {
            output_formats: vec![crate::types::DEFAULT_MODEL3D_FORMAT.to_string()],
            texture: None, pbr: None, target_polycount: None,
            symmetry: None, topology: None, rig: None, extra: None,
        };
        p.generate(&schema, &base, &extras, &Default::default()).await.unwrap().provider_job_id
    }

    #[tokio::test]
    async fn poll_once_drives_a_model3d_row_to_completed_with_a_rehosted_mesh() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_3d_registry().await;
        let store = test_3d_store();
        let job_id = submit_mock_3d("mock/mesh-3d", "a fox").await;

        db.insert_generation(
            "litegen-3d-poll-1", None, "mock/mesh-3d", "mock", "model3d",
            Some(&job_id), 0.0, None, None,
        ).await.unwrap();

        // The mock ramps over three polls, so drive several ticks.
        for _ in 0..5 {
            poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
            let row = db.get_generation("litegen-3d-poll-1").await.unwrap().unwrap();
            if row.status == crate::types::GenerationStatus::Completed { break; }
        }

        let row = db.get_generation("litegen-3d-poll-1").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed);
        assert_eq!(row.progress, 100);

        // result_url is the primary mesh, absolute, .glb — so every existing
        // cross-modal consumer keeps working without knowing about assets[].
        let url = row.result_url.expect("mesh url");
        assert!(url.starts_with("https://assets.test/"), "must be re-hosted, got {url}");
        assert!(url.ends_with("/model.glb"), "got {url}");

        let meta = row.metadata.expect("asset list persisted");
        let assets = meta["assets"].as_array().unwrap();
        assert_eq!(assets.iter().filter(|a| a["kind"] == "mesh").count(), 1);
        assert!(assets.iter().any(|a| a["kind"] == "preview"));
    }

    #[tokio::test]
    async fn poll_once_still_drives_video_rows_through_the_video_provider() {
        // Regression guard for the media_type branch: adding 3D must not send
        // video rows down the `_ => skip` arm.
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_registry().await; // video mock
        let store = test_3d_store();

        db.insert_generation(
            "litegen-vid-branch-1", None, "mock/video-gen", "mock", "video",
            Some("mock-video-job-1"), 0.0, None, None,
        ).await.unwrap();

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        let row = db.get_generation("litegen-vid-branch-1").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed);
        assert!(row.result_url.is_some());
    }

    #[tokio::test]
    async fn poll_once_marks_a_failed_3d_row_without_assets() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_3d_registry().await;
        let store = test_3d_store();
        let job_id = submit_mock_3d("mock/fail-3d", "anything").await;

        db.insert_generation(
            "litegen-3d-fail-1", None, "mock/fail-3d", "mock", "model3d",
            Some(&job_id), 0.0, None, None,
        ).await.unwrap();

        for _ in 0..5 {
            poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
            let row = db.get_generation("litegen-3d-fail-1").await.unwrap().unwrap();
            if row.status == crate::types::GenerationStatus::Failed { break; }
        }

        let row = db.get_generation("litegen-3d-fail-1").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Failed);
        assert!(row.error_message.is_some_and(|e| !e.is_empty()));
        assert!(row.result_url.is_none(), "a failed generation must not carry a mesh url");
    }

    #[tokio::test]
    async fn poll_once_fails_a_completed_3d_row_that_has_no_mesh_asset() {
        // Finding 1 coverage: `mock/fail-3d` reports `Failed` directly and never
        // exercises the completed-without-mesh guard. This drives a genuine
        // `Completed` poll whose only file is a `Preview`, so the guard's
        // `poll.status == Completed && no Mesh asset` condition is actually hit.
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = registry_with_3d_mesh_missing().await;
        let store = test_3d_store();

        db.insert_generation(
            "litegen-3d-no-mesh-1", None, "mock/mesh-3d", "mock", "model3d",
            Some("job-no-mesh"), 0.0, None, None,
        ).await.unwrap();

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        let row = db.get_generation("litegen-3d-no-mesh-1").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Failed);
        assert_eq!(
            row.error_message.as_deref(),
            Some("provider reported success without a mesh asset"),
            "must match get_3d_status's wording exactly so the two paths never disagree"
        );
        assert!(row.result_url.is_none(), "a mesh-less completion must not carry a result url");
    }

    #[tokio::test]
    async fn poll_once_dispatches_webhook_when_3d_completes_without_a_mesh() {
        // Finding 2 coverage: the mesh guard must route through the normal
        // terminal `PollOutcome` tail (which dispatches webhooks), not bypass it
        // via `reap_generation`.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/wh-3d-no-mesh"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let db: Arc<dyn DatabaseStore> = in_memory_db().await;

        let key = db.create_api_key(
            "wh-3d-no-mesh-key",
            "wh-3d-no-mesh-hash",
            "lg-wh3dnm",
            None, None,
            "generate,read",
            Some(&format!("{}/wh-3d-no-mesh", server.uri())),
        ).await.unwrap();

        let registry = registry_with_3d_mesh_missing().await;
        let store = test_3d_store();

        db.insert_generation(
            "litegen-3d-no-mesh-wh-1", Some(&key.id), "mock/mesh-3d", "mock", "model3d",
            Some("job-no-mesh-wh"), 0.0, None, None,
        ).await.unwrap();

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        // Give the spawned webhook task a moment
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        server.verify().await;
    }

    #[tokio::test]
    async fn poll_once_skips_a_row_whose_media_type_has_no_async_provider() {
        // `image` rows are synchronous and never land in the active window; if
        // one ever does, it must be skipped, not crash the tick.
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_3d_registry().await;
        let store = test_3d_store();

        db.insert_generation(
            "litegen-img-stray-1", None, "mock/image-gen", "mock", "image",
            Some("job-x"), 0.0, None, None,
        ).await.unwrap();

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        let row = db.get_generation("litegen-img-stray-1").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Pending, "skipped, not failed");
    }

    // ─── Request-log lifecycle ────────────────────────────────────────────
    //
    // The submit handlers log an async request `pending` under the generation's
    // id. Every terminal transition the poller makes must reach that row too,
    // or `/v1/logs` shows the request `pending` forever.

    /// Seed what a video/3D submit writes: a `pending` request-log row and the
    /// generation row, sharing one id.
    async fn seed_submitted(db: &Arc<dyn DatabaseStore>, id: &str, model: &str, media_type: &str, job_id: &str) {
        db.log_request(id, model, "mock", "pending", media_type, 0.0, 5, None, None, None, None)
            .await
            .unwrap();
        db.insert_generation(id, None, model, "mock", media_type, Some(job_id), 0.0, None, None)
            .await
            .unwrap();
    }

    async fn request_log(db: &Arc<dyn DatabaseStore>, id: &str) -> crate::types::RequestLog {
        let (logs, _) = db.get_request_logs(1, 100).await.unwrap();
        logs.into_iter().find(|l| l.id == id).expect("request log row")
    }

    #[tokio::test]
    async fn poll_once_marks_the_request_log_completed_when_a_video_completes() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_registry().await;
        seed_submitted(&db, "litegen-vid-log-1", "mock/video-gen", "video", "mock-video-job-1").await;

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;

        let log = request_log(&db, "litegen-vid-log-1").await;
        assert_eq!(log.status, GenerationStatus::Completed);
        assert_eq!(log.error, None);
    }

    #[tokio::test]
    async fn poll_once_marks_the_request_log_failed_with_the_error_when_a_3d_job_fails() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_3d_registry().await;
        let store = test_3d_store();
        let job_id = submit_mock_3d("mock/fail-3d", "anything").await;
        seed_submitted(&db, "litegen-3d-log-fail", "mock/fail-3d", "model3d", &job_id).await;

        for _ in 0..5 {
            poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
            let row = db.get_generation("litegen-3d-log-fail").await.unwrap().unwrap();
            if row.status == GenerationStatus::Failed { break; }
        }

        let gen = db.get_generation("litegen-3d-log-fail").await.unwrap().unwrap();
        assert_eq!(gen.status, GenerationStatus::Failed);
        let log = request_log(&db, "litegen-3d-log-fail").await;
        assert_eq!(log.status, GenerationStatus::Failed);
        assert!(log.error.as_deref().is_some_and(|e| !e.is_empty()), "{:?}", log.error);
        assert_eq!(log.error, gen.error_message, "the log carries the provider's error");
    }

    #[tokio::test]
    async fn poll_once_marks_the_request_log_failed_when_a_3d_completion_has_no_mesh() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = registry_with_3d_mesh_missing().await;
        seed_submitted(&db, "litegen-3d-log-no-mesh", "mock/mesh-3d", "model3d", "job-no-mesh").await;

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;

        let log = request_log(&db, "litegen-3d-log-no-mesh").await;
        assert_eq!(log.status, GenerationStatus::Failed);
        assert_eq!(log.error.as_deref(), Some("provider reported success without a mesh asset"));
    }

    #[tokio::test]
    async fn reaping_a_generation_marks_its_request_log_failed() {
        // A non-retryable poll error terminalises through `reap_generation`,
        // the poller's other terminal path (next to the shared tail).
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = registry_with(PollBehavior::TerminalError).await;
        seed_submitted(&db, "litegen-vid-log-dead", "mock/video-gen", "video", "job-dead").await;

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;

        let gen = db.get_generation("litegen-vid-log-dead").await.unwrap().unwrap();
        assert_eq!(gen.status, GenerationStatus::Failed);
        let log = request_log(&db, "litegen-vid-log-dead").await;
        assert_eq!(log.status, GenerationStatus::Failed);
        assert!(log.error.as_deref().is_some_and(|e| e.contains("401")), "{:?}", log.error);
    }
}
