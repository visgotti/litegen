use std::sync::Arc;
use futures::StreamExt;
use tracing::{info, warn};

use crate::db::DatabaseStore;
use crate::proxy::registry::ProviderRegistry;
use crate::providers::VideoGenerationHandle;
use crate::proxy::webhook::dispatch_webhook_logged;
use crate::types::GenerationStatus;

/// Maximum time a generation may sit in `pending`/`processing` before the
/// poller reaps it as `failed`. Without this, a row that never reports a
/// terminal status — revoked key, purged job, removed provider, missing job id,
/// or an endlessly-retryable upstream error — stays active forever and takes a
/// share of every tick's claim (each tick takes the rows polled longest ago).
/// Set generously so a genuinely slow provider job is never reaped
/// prematurely.
const MAX_ACTIVE_AGE_SECS: i64 = 2 * 60 * 60; // 2 hours

/// What a terminal transition notifies with: the key's webhook, its URLs
/// handed out in the app's access mode when `presenting` is given.
pub(crate) struct Notify<'a> {
    pub wh_client: &'a reqwest::Client,
    pub mode: crate::config::Mode,
    pub presenting: Option<&'a crate::access::present::Presenting>,
}

/// Terminalise a stuck generation as `failed` so it leaves the active window.
///
/// Guarded on the row still being in flight: a reap decision is made from a row
/// list fetched at the top of the tick, and `GET /v1/models3d/{id}` can have
/// terminalised the row since. Without the guard a generation that already
/// returned a mesh to its caller is flipped to `failed` with `result_url` NULL.
async fn reap_generation(db: &Arc<dyn DatabaseStore>, gen_id: &str, reason: &str, notify: &Notify<'_>) {
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
    // id) must not keep reading `pending` for a generation that has failed,
    // and what it was billed goes back.
    settle_terminal(db, gen_id, "failed", Some(reason)).await;
    forget_router_job(notify.presenting, gen_id).await;
    // And its webhook goes out, as for any other end: a caller waiting on it
    // would otherwise wait forever.
    if let Ok(Some(row)) = db.get_generation(gen_id).await {
        dispatch_terminal_webhook(db, notify.wh_client, notify.mode, row, notify.presenting.cloned()).await;
    }
}

/// Result of polling one row, normalised across modalities so the reaper, DB
/// update, and webhook dispatch stay modality-agnostic. A completed video or
/// 3D row always carries `assets`: the clip `rehost_video` kept, or every
/// mesh (see the mesh guard in the `model3d` branch below).
struct PollOutcome {
    status: GenerationStatus,
    progress: u8,
    result_url: Option<String>,
    error: Option<String>,
    /// Video and 3D: the re-hosted asset list, persisted to `metadata.assets`.
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
    notify: &Notify<'_>,
) {
    if !e.is_retryable() {
        warn!(generation_id = %gen.id, error = %e, "poller: non-retryable poll error, marking failed");
        reap_generation(db, &gen.id, &e.to_string(), notify).await;
    } else if over_age {
        warn!(generation_id = %gen.id, error = %e, "poller: poll failing past max age, reaping");
        reap_generation(db, &gen.id, "timed out awaiting provider", notify).await;
    } else {
        warn!(generation_id = %gen.id, error = %e, "poller: poll_status failed, will retry");
    }
}

/// Run one polling iteration.
///
/// Claims up to 100 `pending`/`processing` rows, the ones polled longest ago,
/// polls each provider, updates the DB row, and dispatches webhooks on
/// terminal transitions.
///
/// Made `pub(crate)` so tests can call it directly. Its webhooks carry the
/// URLs as they were stored; [`spawn_poller`] passes a [`Presenting`] so
/// they follow each app's access mode (`docs/storage-access.md`).
///
/// [`Presenting`]: crate::access::present::Presenting
#[cfg(test)]
pub(crate) async fn poll_once(
    db: &Arc<dyn DatabaseStore>,
    registry: &Arc<ProviderRegistry>,
    secrets_key: Option<[u8; 32]>,
    mode: crate::config::Mode,
    model3d_store: &Arc<dyn crate::proxy::storage::ImageStorage>,
) {
    poll_once_presenting(db, registry, secrets_key, mode, model3d_store, None).await
}

/// [`poll_once`], with webhooks' file URLs handed out by `presenting`: one
/// tick of this replica's poller (tests drive it directly).
pub async fn poll_once_presenting(
    db: &Arc<dyn DatabaseStore>,
    registry: &Arc<ProviderRegistry>,
    secrets_key: Option<[u8; 32]>,
    mode: crate::config::Mode,
    model3d_store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    presenting: Option<&crate::access::present::Presenting>,
) {
    poll_once_as(crate::shared::replica_id(), db, registry, secrets_key, mode, model3d_store, presenting).await
}

/// How long a replica's claim on an active row holds. Each tick claims the rows
/// it polls and, while it runs, renews the ones it is polling every third of
/// the hold, so a replica keeps a row for as long as it is polling it; a dead
/// one's rows pass to another replica this long after its last claim or
/// renewal. A row still waiting for a slot a third of the hold into the tick is
/// re-claimed before it is polled, and skipped if another replica took it once
/// its hold lapsed. No row is ever stranded: a claim only delays the other
/// replicas, never blocks them for good.
pub const POLL_CLAIM_HOLD: chrono::Duration = chrono::Duration::seconds(30);

/// One tick of the poller run by `claimant` (a replica's id): it claims up to
/// 100 `pending`/`processing` rows no other replica holds, so with several
/// replicas each job's vendor is polled once per tick, not once per replica.
/// The rows polled longest ago go first (a new one counts from its creation),
/// so with more than 100 active rows every one is polled at least once every
/// `ceil(active / 100)` ticks. Tests give each simulated replica its own
/// `claimant`.
pub async fn poll_once_as(
    claimant: &str,
    db: &Arc<dyn DatabaseStore>,
    registry: &Arc<ProviderRegistry>,
    secrets_key: Option<[u8; 32]>,
    mode: crate::config::Mode,
    model3d_store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    presenting: Option<&crate::access::present::Presenting>,
) {
    poll_once_until(claimant, db, registry, secrets_key, mode, model3d_store, presenting, std::future::pending()).await
}

/// [`poll_once_as`], until `stop`: once it has fired, the tick starts no new
/// row (and claims none if it fired before the tick began). The rows already
/// being polled go on; the shutdown's grace bounds them.
// The tick's shared handles plus the stop, from its two call sites.
#[allow(clippy::too_many_arguments)]
pub async fn poll_once_until(
    claimant: &str,
    db: &Arc<dyn DatabaseStore>,
    registry: &Arc<ProviderRegistry>,
    secrets_key: Option<[u8; 32]>,
    mode: crate::config::Mode,
    model3d_store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    presenting: Option<&crate::access::present::Presenting>,
    stop: impl std::future::Future<Output = ()>,
) {
    poll_tick(claimant, POLL_CLAIM_HOLD, db, registry, secrets_key, mode, model3d_store, presenting, stop).await
}

/// [`poll_once_as`] with claims that hold for `hold` (tests shorten it).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn poll_once_with_hold(
    claimant: &str,
    hold: chrono::Duration,
    db: &Arc<dyn DatabaseStore>,
    registry: &Arc<ProviderRegistry>,
    secrets_key: Option<[u8; 32]>,
    mode: crate::config::Mode,
    model3d_store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    presenting: Option<&crate::access::present::Presenting>,
) {
    poll_tick(claimant, hold, db, registry, secrets_key, mode, model3d_store, presenting, std::future::pending()).await
}

/// One tick: claim rows for `hold`, then poll them until `stop`.
#[allow(clippy::too_many_arguments)]
async fn poll_tick(
    claimant: &str,
    hold: chrono::Duration,
    db: &Arc<dyn DatabaseStore>,
    registry: &Arc<ProviderRegistry>,
    secrets_key: Option<[u8; 32]>,
    mode: crate::config::Mode,
    model3d_store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    presenting: Option<&crate::access::present::Presenting>,
    stop: impl std::future::Future<Output = ()>,
) {
    let mut stop = std::pin::pin!(stop);
    if futures::poll!(stop.as_mut()).is_ready() {
        return;
    }
    let rows = match db.claim_active_generations(100, claimant, hold).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "poller: failed to claim active generations");
            return;
        }
    };

    // One hardened (no-redirect) webhook client per tick, cloned into each
    // dispatch task — `reqwest::Client` clones share a connection pool, so this
    // avoids building (and discarding) a fresh pool per terminal generation.
    // In hosted mode it connects only to public addresses, on every retry.
    let wh_client = crate::proxy::webhook::webhook_client(mode);

    // A row's age is judged on the database's clock, which stamped its
    // `created_at`, never on this replica's own, which may be off: read once
    // a tick and carried forward on this process's monotonic clock. If it
    // can't be read, this replica's clock stands in.
    let db_now = match db.clock_now().await {
        Ok(now) => now,
        Err(e) => {
            warn!(error = %e, "poller: could not read the database's clock; judging ages on this replica's");
            chrono::Utc::now()
        }
    };
    // A tick can outlast its claims' hold: rows wait for a slot, and one row
    // can take as long as its vendor's timeout or a large download. So the
    // rows being polled are renewed every third of the hold while the tick
    // runs, and another replica never polls a row this one still is. A row
    // that starts after that long is re-claimed first (below).
    let claimed_at = tokio::time::Instant::now();
    let now = move || db_now + chrono::Duration::from_std(claimed_at.elapsed()).unwrap_or_else(|_| chrono::Duration::zero());
    let renew_every = (hold.to_std().unwrap_or_default() / 3).max(std::time::Duration::from_millis(1));
    let in_progress = std::sync::Mutex::new(std::collections::HashSet::<String>::new());
    let in_progress = &in_progress;
    let in_progress_now = || in_progress.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

    // Rows are polled side by side, so one that is slow to answer (a vendor
    // that hangs, a large 3D delivery) does not hold every row behind it
    // until the next tick. 3D deliveries are bounded on their own
    // (`delivery_lease::delivery_slots`). A row starts only while the stop
    // has not fired: `take_until` asks it before handing out each row.
    let wh_client = &wh_client;
    let polling = futures::stream::iter(rows)
        .take_until(stop)
        .for_each_concurrent(POLL_CONCURRENCY, |gen| async move {
            let id = gen.id.clone();
            // A row that waited for a slot this long may have passed its
            // hold and been taken by another replica, or have ended (a
            // client's poll): 0 rows re-claimed means it is not this
            // replica's to poll any more. A database error polls it as
            // before; its writes are guarded.
            if claimed_at.elapsed() > renew_every {
                match db.renew_poll_claims(claimant, std::slice::from_ref(&id), hold).await {
                    Ok(0) => {
                        info!(generation_id = %id, "poller: this generation was taken by another replica or ended while it waited; skipping it");
                        return;
                    }
                    Ok(_) => {}
                    Err(e) => warn!(generation_id = %id, error = %e, "poller: could not re-claim a generation that waited; polling it all the same"),
                }
            }
            in_progress_now().insert(id.clone());
            let over_age = (now() - gen.created_at).num_seconds() > MAX_ACTIVE_AGE_SECS;
            let polled = std::panic::AssertUnwindSafe(poll_row(db, registry, secrets_key, mode, model3d_store, wh_client, presenting, gen, over_age));
            // A panic polling one row (an adapter, a rehost, a delivery) is
            // that row's alone: the other rows, and the next ticks, go on.
            if let Err(payload) = futures::FutureExt::catch_unwind(polled).await {
                tracing::error!(generation_id = %id, panic = %crate::supervise::panic_text(&*payload), "poller: polling this generation panicked");
                // A row that panics on every poll would never reach a terminal
                // status: past the maximum age it is failed like a stuck one
                // (refunded, its webhook sent). A younger one is tried again.
                if over_age {
                    let notify = Notify { wh_client, mode, presenting };
                    reap_generation(db, &id, "polling this generation kept failing inside litegen", &notify).await;
                }
            }
            in_progress_now().remove(&id);
        });
    let renewing = async {
        let mut every = tokio::time::interval_at(claimed_at + renew_every, renew_every);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            let ids: Vec<String> = in_progress_now().iter().cloned().collect();
            if ids.is_empty() {
                continue;
            }
            if let Err(e) = db.renew_poll_claims(claimant, &ids, hold).await {
                warn!(error = %e, rows = ids.len(), "poller: could not renew the claims on the rows being polled");
            }
        }
    };
    // The renewal never ends: the tick ends when its rows are done.
    tokio::select! {
        _ = polling => {}
        _ = renewing => {}
    }
}

/// Rows one tick polls at once.
const POLL_CONCURRENCY: usize = 8;

/// One row's poll: its vendor's status, and on a terminal one the guarded
/// write, the log update and the webhook.
// The tick's shared handles plus the row, from its one call site.
#[allow(clippy::too_many_arguments)]
async fn poll_row(
    db: &Arc<dyn DatabaseStore>,
    registry: &Arc<ProviderRegistry>,
    secrets_key: Option<[u8; 32]>,
    mode: crate::config::Mode,
    model3d_store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    wh_client: &reqwest::Client,
    presenting: Option<&crate::access::present::Presenting>,
    gen: crate::types::Generation,
    // Whether the row is older than MAX_ACTIVE_AGE on the database's clock.
    // Rows that can never advance are reaped once they are, so they stop
    // taking a share of every tick's claim (see reap_generation).
    over_age: bool,
) {
    // A reap notifies as every other terminal transition does.
    let notify = &Notify { wh_client, mode, presenting };

    // Resolve this generation's per-app BYO credential, if the app stored one.
    // Any failure (no secrets key, lookup error, decrypt/parse error) falls back
    // to `None` (→ the platform default global instance). The poller must never
    // crash on a single bad credential, so all errors are logged and swallowed.
    let app_creds = resolve_gen_credential(db, secrets_key, &gen).await;

    // A finished 3D job's delivery claim, or a finished video's, held until
    // the outcome is written.
    let mut delivery_claim: Option<crate::proxy::delivery_lease::DeliveryClaim> = None;

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
                        reap_generation(db, &gen.id, "provider no longer configured", notify).await;
                    } else {
                        warn!(generation_id = %gen.id, provider = %gen.provider, "poller: provider not found, skipping");
                    }
                    return;
                }
            };

            let provider_job_id = match &gen.provider_job_id {
                Some(id) => id.clone(),
                None => {
                    if over_age {
                        warn!(generation_id = %gen.id, "poller: no provider_job_id past max age, reaping");
                        reap_generation(db, &gen.id, "no provider job id was recorded", notify).await;
                    } else {
                        warn!(generation_id = %gen.id, "poller: no provider_job_id, skipping");
                    }
                    return;
                }
            };

            let handle = VideoGenerationHandle {
                provider_job_id,
                provider: gen.provider.clone(),
                model: gen.model.clone(),
            };

            // Kept or not as the request decided at submit. A finished video
            // that is kept is claimed before any of it is downloaded: one
            // observer keeps it; the others stand aside and write `processing`.
            let keep = crate::access::row_keeps_outputs(gen.metadata.as_ref());
            let mut poll = match crate::proxy::router::poll_video_claimed(
                &provider, &handle, keep, db, &gen.id, crate::proxy::delivery_lease::Observer::Poller,
            ).await {
                Ok((p, claim)) => {
                    delivery_claim = claim;
                    p
                }
                Err(e) => {
                    handle_poll_error(db, &gen, &e, over_age, notify).await;
                    return;
                }
            };

            // Keep a finished video on litegen storage now, in the tick that
            // saw it finish: the vendor's URL may expire within the hour.
            if poll.status == GenerationStatus::Completed {
                let default_aws = crate::config::AwsConfig::default();
                let aws = presenting.map_or(&default_aws, |p| &p.router.config.aws);
                let (store, prefix) = match gen.app_id.as_deref().filter(|_| keep) {
                    Some(app_id) => match crate::api::handlers::app_store_for_writing(db, secrets_key, aws, tenant_bucket(mode, presenting), app_id).await {
                        Ok(found) => found.unwrap_or_else(|| (model3d_store.clone(), None)),
                        // Its bucket is down and litegen's store may not stand
                        // in: the video waits for it, up to the maximum age.
                        Err(reason) => {
                            if over_age {
                                reap_generation(db, &gen.id, &format!("the video finished but could not be kept: {reason}"), notify).await;
                            } else {
                                warn!(generation_id = %gen.id, reason = %reason, "poller: finished video has nowhere to be kept yet, will retry");
                            }
                            return;
                        }
                    },
                    None => (model3d_store.clone(), None),
                };
                // The bytes move on: no copy of a video that may be 1 GiB.
                let data = poll.video_data.take();
                let public_only = !presenting.is_some_and(|p| p.router.config.storage_access.unchecked_vendor_hosts);
                let keeping = crate::proxy::router::rehost_video(
                    &store, prefix.as_deref(), &gen.id,
                    data, poll.video_url.as_deref(), poll.content_type.as_deref(), keep, public_only,
                );
                let stored = match delivery_claim.as_ref() {
                    Some(claim) => claim.renewing_while(keeping).await,
                    None => keeping.await,
                };
                match stored {
                    Ok(stored) => Some(PollOutcome {
                        status: poll.status,
                        progress: poll.progress,
                        result_url: Some(stored.url),
                        error: poll.error,
                        assets: Some(serde_json::json!({ "assets": [stored.asset] })),
                    }),
                    // Nothing litegen can keep: the generation has failed, and
                    // it ends now as any vendor failure does (refund, webhook).
                    Err(crate::proxy::router::RehostError::Permanent(why)) => Some(PollOutcome {
                        status: GenerationStatus::Failed,
                        progress: poll.progress,
                        result_url: None,
                        error: Some(format!("the video finished but could not be kept: {why}")),
                        assets: None,
                    }),
                    Err(e) => {
                        if over_age {
                            warn!(generation_id = %gen.id, error = %e, "poller: video still could not be kept past max age, reaping");
                            reap_generation(db, &gen.id, &format!("the video finished but could not be kept: {e}"), notify).await;
                        } else {
                            warn!(generation_id = %gen.id, error = %e, "poller: finished video could not be kept, will retry");
                        }
                        return;
                    }
                }
            } else {
                Some(PollOutcome {
                    status: poll.status,
                    progress: poll.progress,
                    result_url: poll.video_url,
                    error: poll.error,
                    assets: None,
                })
            }
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
                        reap_generation(db, &gen.id, "provider no longer configured", notify).await;
                    } else {
                        warn!(generation_id = %gen.id, provider = %gen.provider, "poller: 3d provider not found, skipping");
                    }
                    return;
                }
            };
            let provider_job_id = match gen.provider_job_id.clone() {
                Some(id) => id,
                None => {
                    if over_age {
                        warn!(generation_id = %gen.id, "poller: no provider_job_id past max age, reaping");
                        reap_generation(db, &gen.id, "no provider job id was recorded", notify).await;
                    } else {
                        warn!(generation_id = %gen.id, "poller: no provider_job_id, skipping");
                    }
                    return;
                }
            };
            let mut handle = crate::providers::Model3dGenerationHandle {
                provider_job_id,
                provider: gen.provider.clone(),
                model: gen.model.clone(),
                stage_context: gen
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("stage_context").cloned()),
            };
            // An adapter that downloads a finished job's files asks the gate
            // first, so the poller downloads only what it delivers, and holds
            // no files while it waits for a slot.
            let gate = crate::proxy::delivery_lease::DeliveryGate::new(db, &gen.id, crate::proxy::delivery_lease::Observer::Poller);
            let mut poll = match provider.poll_status_gated(&handle, &gate).await {
                Ok(p) => p,
                Err(e) => {
                    handle_poll_error(db, &gen, &e, over_age, notify).await;
                    return;
                }
            };
            // A vendor flow's next billed stage, or its new continuation
            // state — the same bookkeeping the client's poll applies.
            crate::proxy::model3d_stages::advance(db, &provider, &gen.id, &mut handle, &mut poll, "poller").await;

            // A finished job is delivered by one observer: the poller only
            // if it claims the delivery. It waits for a slot; a lease
            // another observer holds makes it stand aside this tick. An
            // adapter that never asked the gate is claimed for here.
            match gate.into_outcome() {
                crate::proxy::delivery_lease::GateState::Claimed(claim) => delivery_claim = Some(claim),
                crate::proxy::delivery_lease::GateState::Refused => {
                    info!(generation_id = %gen.id, "poller: another observer is delivering this 3d job");
                    crate::proxy::model3d_deliver::stand_aside(&mut poll);
                }
                crate::proxy::delivery_lease::GateState::NotAsked if crate::proxy::model3d_deliver::needs_delivery(&poll) => {
                    delivery_claim = crate::proxy::delivery_lease::claim_delivery(
                        db, &gen.id, crate::proxy::delivery_lease::Observer::Poller,
                    ).await;
                    if delivery_claim.is_none() {
                        info!(generation_id = %gen.id, "poller: another observer is delivering this 3d job");
                        crate::proxy::model3d_deliver::stand_aside(&mut poll);
                    }
                }
                crate::proxy::delivery_lease::GateState::NotAsked => {}
            }

            // Re-host NOW, in the same tick that observed completion: several
            // vendors expire their download URLs within minutes, so the bytes
            // `poll` already carries must be moved to litegen-owned storage
            // before the next poll cycle rather than deferred.
            // Normalise, measure and fill in the promised containers BEFORE
            // rehost — the step the client's poll runs too. The plan is read
            // off the row: the poller cannot see the router's job map.
            let delivery = crate::proxy::model3d_deliver::DeliveryPlan::from_row(&gen);
            let requested = delivery.requested_formats.clone();
            let keep = !delivery.keep_nothing;
            crate::proxy::model3d_deliver::deliver_poll(&mut poll, delivery, &gen.id).await;
            // Keeping nothing: the vendor's own files are the answer, or there is none.
            let handed_out = if !keep && poll.status == GenerationStatus::Completed {
                match crate::proxy::router::keep_nothing_model3d_assets(&poll.files) {
                    Ok(a) => Some(a),
                    Err(why) => {
                        poll.status = GenerationStatus::Failed;
                        poll.error = Some(why);
                        None
                    }
                }
            } else {
                None
            };

            let assets = if handed_out.is_some() {
                handed_out
            } else if poll.status == GenerationStatus::Completed && !poll.files.is_empty() {
                // Per-app BYO bucket (and its configured prefix) if the app set
                // one up, else the shared global store passed in by the caller.
                let default_aws = crate::config::AwsConfig::default();
                let aws = presenting.map_or(&default_aws, |p| &p.router.config.aws);
                let (store, prefix) = match gen.app_id.as_deref() {
                    Some(app_id) => match crate::api::handlers::app_store_for_writing(db, secrets_key, aws, tenant_bucket(mode, presenting), app_id).await {
                        Ok(found) => found.unwrap_or_else(|| (model3d_store.clone(), None)),
                        // Its bucket is down and litegen's store may not stand
                        // in: the mesh waits for it, up to the maximum age.
                        Err(reason) => {
                            if over_age {
                                reap_generation(db, &gen.id, &format!("could not store the 3d assets: {reason}"), notify).await;
                            } else {
                                warn!(generation_id = %gen.id, reason = %reason, "poller: finished 3d job has nowhere to be kept yet, will retry");
                            }
                            return;
                        }
                    },
                    None => (model3d_store.clone(), None),
                };
                // The files move on to the store, never copied.
                match crate::proxy::router::rehost_model3d_files(
                    &store, prefix.as_deref(), &gen.id, std::mem::take(&mut poll.files),
                ).await {
                    Ok(a) => Some(a),
                    Err(e) => {
                        // Storage failure is retryable — leave the row
                        // non-terminal so the next tick tries again rather
                        // than reporting a completed generation with no mesh.
                        // But not forever: this `continue` skips the
                        // over-age reap at the end of the loop, so a store
                        // that never recovers (revoked BYO bucket key) would
                        // keep the row re-downloading and re-converting every
                        // tick and holding a slot in every tick's claim.
                        if over_age {
                            warn!(generation_id = %gen.id, error = %e, "poller: 3d asset upload still failing past max age, reaping");
                            reap_generation(db, &gen.id, &format!("could not store the 3d assets: {e}"), notify).await;
                        } else {
                            warn!(generation_id = %gen.id, error = %e, "poller: 3d asset upload failed, will retry");
                        }
                        return;
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
            // the webhook) rather than being reported as success. `requested`
            // is the same binding the synthesis step above read, so what is
            // enforced here is exactly what was attempted there.
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
                    // The terminal write merges into metadata, so submit's
                    // keys survive; `requested_formats` is re-stated so a row
                    // whose submit-time write was lost still carries it.
                    assets: assets.map(|a| serde_json::json!({
                        "assets": a,
                        "requested_formats": requested,
                    })),
                })
            }
        }
        "image" => {
            // A late image (docs/async-images.md): a job the vendor took
            // after the call that submitted it had answered.
            let provider = match registry.image_provider_for_request(&gen.provider, app_creds).await {
                Some(p) => p,
                None => {
                    if over_age {
                        reap_generation(db, &gen.id, "provider no longer configured", notify).await;
                    } else {
                        warn!(generation_id = %gen.id, provider = %gen.provider, "poller: image provider not found, skipping");
                    }
                    return;
                }
            };
            // A row written `completed` at once that lost that write has no
            // job to ask about: past max age it is reaped, not skipped forever.
            let Some(job) = gen.provider_job_id.clone() else {
                if over_age {
                    reap_generation(db, &gen.id, "no provider job id was recorded", notify).await;
                } else {
                    warn!(generation_id = %gen.id, "poller: image row with no vendor job, skipping");
                }
                return;
            };
            let resume = gen
                .metadata
                .as_ref()
                .and_then(|m| m.pointer("/image_job/resume"))
                .filter(|v| !v.is_null())
                .cloned();
            match provider.poll_job(&gen.model, &job, resume.as_ref()).await {
                Ok(crate::providers::ImageJobPoll::Running) => Some(PollOutcome {
                    status: GenerationStatus::Processing,
                    progress: 50,
                    result_url: None,
                    error: None,
                    assets: None,
                }),
                Ok(crate::providers::ImageJobPoll::Failed(why)) => Some(PollOutcome {
                    status: GenerationStatus::Failed,
                    progress: 0,
                    result_url: None,
                    error: Some(why),
                    assets: None,
                }),
                Ok(crate::providers::ImageJobPoll::Done(output)) => {
                    let default_aws = crate::config::AwsConfig::default();
                    let aws = presenting.map_or(&default_aws, |p| &p.router.config.aws);
                    match keep_late_image(db, secrets_key, aws, tenant_bucket(mode, presenting), model3d_store, &gen, output).await {
                        Ok(assets) => Some(PollOutcome {
                            status: GenerationStatus::Completed,
                            progress: 100,
                            result_url: assets.first().map(|a| a.url.clone()),
                            error: None,
                            assets: Some(serde_json::json!({ "assets": assets })),
                        }),
                        // Its bucket is down and litegen's store may not stand
                        // in: the image waits for it, up to the maximum age.
                        Err(reason) => {
                            if over_age {
                                reap_generation(db, &gen.id, &format!("the image finished but could not be kept: {reason}"), notify).await;
                            } else {
                                warn!(generation_id = %gen.id, reason = %reason, "poller: finished image has nowhere to be kept yet, will retry");
                            }
                            return;
                        }
                    }
                }
                Err(e) => {
                    handle_poll_error(db, &gen, &e, over_age, notify).await;
                    return;
                }
            }
        }
        other => {
            // Only video, 3D and late images are async; a row of any other
            // kind still active here lost its write, and past max age it is
            // reaped rather than skipped forever.
            if over_age {
                warn!(generation_id = %gen.id, media_type = %other, "poller: a row with no async provider is past max age, reaping");
                reap_generation(db, &gen.id, "no async provider drives this generation", notify).await;
            } else {
                warn!(generation_id = %gen.id, media_type = %other, "poller: no async provider for media type, skipping");
            }
            return;
        }
    };
    let Some(outcome) = outcome else { return };

    let is_terminal = matches!(
        outcome.status,
        GenerationStatus::Completed | GenerationStatus::Failed | GenerationStatus::Cancelled
    );

    // A row that is reachable and polling cleanly but still hasn't reached a
    // terminal state after MAX_ACTIVE_AGE is treated as stuck and reaped.
    if !is_terminal && over_age {
        warn!(generation_id = %gen.id, "poller: still non-terminal past max age, reaping");
        reap_generation(db, &gen.id, "timed out: no terminal status within max active age", notify).await;
        return;
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
            return;
        }
    };
    // The outcome is written (or lost to another observer's): the claim
    // has done its job, and releasing it now frees a row this write did
    // not settle for the next tick.
    drop(delivery_claim);
    if !write.owns_outcome() {
        // The other observer terminalised this row between this tick's claim
        // of it and now. It owns the log update and the webhook; doing
        // either here would duplicate them.
        info!(generation_id = %gen.id, "poller: row already terminal, leaving it to the observer that wrote it");
        return;
    }

    // The submit handler logged this request `pending` under the same id;
    // move that row to the outcome too, or `/v1/logs` reads `pending`
    // forever. Best-effort: the generation row above is the source of truth.
    if is_terminal {
        settle_terminal(db, &gen.id, &status_str, outcome.error.as_deref()).await;
        forget_router_job(presenting, &gen.id).await;
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
        dispatch_terminal_webhook(db, wh_client, mode, updated_gen, presenting.cloned()).await;
    }
}

/// The job has ended, by this replica's own terminal write: if this replica's
/// router took its submit, the router lets it go now. A client's poll then
/// answers from the row, as on any other replica, and the map holds no job
/// (and no provider instance) that has ended. The router also lets any job go
/// past its maximum age.
async fn forget_router_job(presenting: Option<&crate::access::present::Presenting>, id: &str) {
    if let Some(presenting) = presenting {
        presenting.router.forget_video_job(id).await;
        presenting.router.forget_model3d_job(id).await;
    }
}

/// Whose this deployment's app buckets are (`handlers::tenant_bucket`): by the
/// poller's mode, with the tests' switch read from the router's config when
/// the poller has one.
fn tenant_bucket(mode: crate::config::Mode, presenting: Option<&crate::access::present::Presenting>) -> crate::api::handlers::TenantBucket {
    crate::api::handlers::tenant_bucket(mode, presenting.map(|p| p.router.config.as_ref()))
}

/// A late image's files, kept where the call would have kept them: the app's
/// own bucket, else litegen's store, at `{prefix}/{id}/image_{i}.{format}`.
async fn keep_late_image(
    db: &Arc<dyn DatabaseStore>,
    secrets_key: Option<[u8; 32]>,
    aws: &crate::config::AwsConfig,
    tenant: crate::api::handlers::TenantBucket,
    litegen_store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    gen: &crate::types::Generation,
    mut output: crate::providers::GenerationOutput,
) -> Result<Vec<crate::types::GenerationAsset>, String> {
    let (store, prefix) = match gen.app_id.as_deref() {
        Some(app_id) => crate::api::handlers::app_store_for_writing(db, secrets_key, aws, tenant, app_id)
            .await?
            .unwrap_or_else(|| (litegen_store.clone(), None)),
        None => (litegen_store.clone(), None),
    };
    let root = prefix.as_deref().unwrap_or(crate::proxy::storage::IMAGE_RESULT_PATH_PREFIX).trim_matches('/').to_string();
    let more = std::mem::take(&mut output.more);
    let mut assets = Vec::with_capacity(1 + more.len());
    for (i, image) in std::iter::once(output).chain(more).enumerate() {
        // The bytes say what they are; an adapter's label can be wrong (L9).
        let content_type = crate::proxy::router::image_content_type(&image.data).map_or_else(|| image.content_type.clone(), str::to_string);
        let format = crate::proxy::storage::image_format(&content_type);
        let key = format!("{root}/{}/image_{i}.{format}", gen.id);
        // Measured first: the bytes then move into the store, never copied.
        let size = image::ImageReader::new(std::io::Cursor::new(&image.data))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok());
        let (len, sha256) = (image.data.len() as u64, crate::proxy::router::sha256_hex(&image.data));
        let url = store
            .put(&key, &bytes::Bytes::from(image.data), &content_type)
            .await
            .map_err(|e| format!("storing image {i}: {e}"))?;
        assets.push(crate::types::GenerationAsset {
            expires_at: None,
            index: i as u32,
            kind: "image".into(),
            format: format.into(),
            url,
            size_bytes: Some(len),
            sha256: Some(sha256),
            width: size.map(|s| s.0),
            height: size.map(|s| s.1),
            duration_s: None,
            fps: None,
            has_audio: None,
            kept: true,
        });
    }
    Ok(assets)
}

/// Deliver the terminal-generation webhook for `gen`, if its key has one.
///
/// Extracted from the poller because the poller is no longer the only terminal
/// observer: `GET /v1/models3d/{id}` terminalises a 3D row too, and whenever it
/// won the race the row never came up among the poller's active rows again, so
/// the only dispatch site never ran and the caller got no delivery at all.
/// Both callers dispatch only when their guarded UPDATE actually wrote the row,
/// which is what makes "at most once" hold.
///
/// Hands the delivery to this process's dispatcher
/// ([`deliveries`](crate::proxy::webhook::deliveries)) and returns at once: a
/// webhook endpoint's latency must not stall a poll tick or an HTTP response.
/// It runs within its org's share of the dispatcher, and a graceful stop
/// waits for it within the grace.
pub(crate) async fn dispatch_terminal_webhook(
    db: &Arc<dyn DatabaseStore>,
    wh_client: &reqwest::Client,
    mode: crate::config::Mode,
    gen: crate::types::Generation,
    presenting: Option<crate::access::present::Presenting>,
) {
    let Some(key_id) = gen.key_id else { return };
    let db2 = db.clone();
    let wh_client = wh_client.clone();
    let gen_id = gen.id.clone();
    let org = gen.org_id.clone().unwrap_or_else(|| crate::api::middleware::DEFAULT_ORG_ID.to_string());
    let delivery_of = gen.id.clone();

    crate::proxy::webhook::deliveries().spawn(&org, &delivery_of, async move {
        // The payload is the row as its terminal write and settlement left it,
        // re-read: the caller's `metadata` and every other part of the row are
        // what `GET /v1/generations/{id}` answers, and a refunded job reads
        // `cost_usd: 0`. The observer's own copy stands in if the read fails.
        let mut gen = match db2.get_generation(&gen_id).await {
            Ok(Some(row)) if row.status.is_terminal() => row,
            _ => gen,
        };
        // The key's own webhook gets the files as that key may open them:
        // signed links in a `signed` app, the file route in a `private` one.
        if let Some(presenting) = presenting {
            use crate::access::present::{FileOwner, Viewer};
            presenting.presenter(&FileOwner::of(&gen), Viewer::Owner).await.present_row(&mut gen).await;
        }
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

/// Settle a generation's terminal transition, by the observer whose guarded
/// write decided it: its request log takes the outcome, and a job that ended
/// `failed` or `cancelled` gives back what it was billed (L18). Returns the
/// amount refunded.
///
/// Every terminal path calls this — the poller, a reap, `GET /v1/videos/{id}`,
/// `GET /v1/models3d/{id}` and a cancel — and only after winning the write, so
/// a generation is settled once; the refund's own claim keeps it once even so.
pub(crate) async fn settle_terminal(db: &Arc<dyn DatabaseStore>, id: &str, status: &str, error: Option<&str>) -> f64 {
    if let Err(e) = db.update_request_log_status(id, status, error).await {
        warn!(generation_id = %id, error = %e, "failed to settle the request log");
    }
    if status != "failed" && status != "cancelled" {
        return 0.0;
    }
    refund_generation(db, id).await
}

/// Give back what a failed or cancelled generation was billed: to its key's
/// quota, and to every principal's limits it was charged against (recorded
/// on the row at submit as `billing.limits`). The row's cost and its log's
/// go to 0, so `usage`, `/v1/logs` and `/v1/stats` read what was kept.
async fn refund_generation(db: &Arc<dyn DatabaseStore>, id: &str) -> f64 {
    let gen = match db.get_generation(id).await {
        Ok(Some(gen)) if gen.cost_usd > 0.0 => gen,
        Ok(_) => return 0.0,
        Err(e) => {
            warn!(generation_id = %id, error = %e, "refund: could not read the generation");
            return 0.0;
        }
    };
    match db.claim_generation_refund(id, gen.cost_usd).await {
        Ok(true) => {}
        Ok(false) => return 0.0, // refunded already, or never billed
        Err(e) => {
            warn!(generation_id = %id, error = %e, "refund: could not claim it");
            return 0.0;
        }
    }
    if let Some(key_id) = gen.key_id {
        if let Err(e) = db.atomic_charge_tokens(&key_id, -gen.cost_usd).await {
            warn!(generation_id = %id, error = %e, "refund: the key's quota could not be credited");
        }
    }
    let charged: Vec<crate::management::enforce::ChargedLimit> = gen
        .metadata
        .as_ref()
        .and_then(|m| serde_json::from_value(m.get("billing")?.get("limits")?.clone()).ok())
        .unwrap_or_default();
    crate::management::enforce::refund_limits(db, &charged, gen.cost_usd).await;
    info!(generation_id = %id, refunded_usd = gen.cost_usd, "refunded a generation that did not deliver");
    gen.cost_usd
}

/// Resolve a single generation's stored per-org BYO credential, decrypted.
///
/// Returns `None` (→ platform default) when the generation has no org, no secrets
/// key is configured, the org stored no credential, or anything fails to look up /
/// decrypt / parse. Errors are logged but never propagated — the poller must keep
/// running across a bad credential on one row.
pub(crate) async fn resolve_gen_credential(
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
///
/// Once `shutdown` fires, no new tick and no new row starts; the rows being
/// polled finish (the caller bounds that wait). A panic in a tick, or in one
/// row's poll, is logged and the loop goes on (`crate::supervise`).
pub fn spawn_poller(
    db: Arc<dyn DatabaseStore>,
    registry: Arc<ProviderRegistry>,
    secrets_key: Option<[u8; 32]>,
    mode: crate::config::Mode,
    model3d_store: Arc<dyn crate::proxy::storage::ImageStorage>,
    presenting: Option<crate::access::present::Presenting>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    use futures::FutureExt as _;
    tokio::spawn(async move {
        // One stop, asked both between ticks and before each row of a tick.
        let stop = shutdown.boxed().shared();
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        // A tick that completes 3D jobs (downloads, conversion, uploads) can
        // run far past 5 s; the default `Burst` would then fire every missed
        // tick back to back, re-polling each active row's vendor in a burst.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                // The stop first: after a long tick an overdue tick is ready
                // at once too, and must not start another round of polls.
                biased;
                _ = stop.clone() => {
                    tracing::info!("poller received shutdown signal, exiting loop");
                    return;
                }
                _ = interval.tick() => {
                    let tick = poll_once_until(
                        crate::shared::replica_id(), &db, &registry, secrets_key, mode, &model3d_store, presenting.as_ref(), stop.clone(),
                    );
                    crate::supervise::guarded("a poller tick", tick).await;
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

    /// P8 (code review 2026-11-11): each tick polls a row on an org's own key
    /// with an adapter built for that row. Its HTTP client is the process's
    /// shared one, so two ticks reuse one vendor connection (they opened one
    /// each: a new TCP and TLS handshake per row every 5 s).
    #[tokio::test]
    async fn a_byo_row_is_polled_over_one_connection() {
        use std::sync::atomic::Ordering;
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let app = axum::Router::new().route(
            "/v1/videos/{id}",
            axum::routing::get({
                let seen = seen.clone();
                move |headers: axum::http::HeaderMap| {
                    let seen = seen.clone();
                    async move {
                        seen.lock().unwrap().push(headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string());
                        axum::Json(serde_json::json!({ "status": "in_progress", "progress": 10 }))
                    }
                }
            }),
        );
        let (base, accepted) = crate::util::counting_vendor(app).await;
        let registry = Arc::new(ProviderRegistry::new());
        let platform = ProviderInstanceConfig {
            api_key: "PLATFORM".into(),
            credentials: crate::providers::ProviderCredentials { api_key: Some("PLATFORM".into()), ..Default::default() },
            api_base: Some(format!("{base}/v1")),
            ..Default::default()
        };
        assert!(registry.configure_provider("openai", platform).await);
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let key = [7u8; 32];
        let (ct, nonce) = crate::auth::secrets::encrypt(&key, br#"{"api_keys":[{"key":"org-key"}]}"#).unwrap();
        let org = crate::api::middleware::DEFAULT_ORG_ID;
        db.upsert_org_provider_credential(org, "openai", &ct, &nonce, None).await.unwrap();
        db.insert_generation("litegen-vid-byo", None, "openai/sora-2", "openai", "video", Some("job-1"), 0.0, Some(org), None).await.unwrap();
        let store = test_3d_store();
        poll_once(&db, &registry, Some(key), crate::config::Mode::SingleTenant, &store).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        poll_once(&db, &registry, Some(key), crate::config::Mode::SingleTenant, &store).await;
        assert_eq!(*seen.lock().unwrap(), vec!["Bearer org-key"; 2], "both ticks polled on the org's key");
        let n = accepted.load(Ordering::SeqCst);
        assert_eq!(n, 1, "two ticks on one org's key opened {n} connections: each built its own HTTP client");
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
    async fn poll_once_hands_out_the_vendor_link_for_a_row_that_keeps_nothing() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_registry().await;
        let id = "litegen-vid-keep-nothing";
        db.insert_generation(id, None, "mock/video-gen", "mock", "video", Some("mock-video-job-kn"), 0.0, None, None).await.unwrap();
        db.merge_generation_metadata_if_active(id, &serde_json::json!({ "keep_outputs": false })).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn crate::proxy::storage::ImageStorage> =
            Arc::new(crate::proxy::storage::LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()));
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
        let after = db.get_generation(id).await.unwrap().unwrap();
        assert_eq!(after.status, crate::types::GenerationStatus::Completed);
        assert_eq!(after.result_url.as_deref(), Some("https://example.com/mock-video.mp4"), "the vendor's link");
        let metadata = after.metadata.expect("metadata");
        assert_eq!(metadata["assets"][0]["kept"], false, "{metadata}");
        assert_eq!(metadata["keep_outputs"], false, "the submit's decision stays: {metadata}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing written");
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

    /// L17: the webhook is the row as decided — the caller's `metadata` with
    /// it — not the observer's patch, which held only the assets.
    #[tokio::test]
    async fn the_terminal_webhook_carries_the_callers_metadata() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/wh"))
            .and(body_partial_json(serde_json::json!({
                "status": "completed",
                "metadata": { "request": { "metadata": { "project": "castle" } } },
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let key = db.create_api_key(
            "wh-key", "wh-hash-meta", "lg-wh", None, None, "generate,read", Some(&format!("{}/wh", server.uri())),
        ).await.unwrap();
        let registry = make_registry().await;
        db.insert_generation(
            "litegen-vid-wh-meta-1", Some(&key.id), "mock/video-gen", "mock", "video", Some("mock-video-job-1"), 0.0, None, None,
        ).await.unwrap();
        db.merge_generation_metadata_if_active(
            "litegen-vid-wh-meta-1",
            &serde_json::json!({ "request": { "metadata": { "project": "castle" } } }),
        ).await.unwrap();

        let store = test_3d_store();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        server.verify().await;
    }

    /// L18: a reaped job gives back what it was billed — the key's quota, the
    /// row's cost and its log's — and settling it again gives back nothing.
    #[tokio::test]
    async fn a_reaped_job_is_refunded_once() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let key = db.create_api_key("refund", "refund-hash", "lg-rf", Some(5.0), None, "generate,read", None).await.unwrap();
        db.atomic_charge_tokens(&key.id, 0.4).await.unwrap();
        db.log_request("litegen-vid-refund-1", "mock/priced-video", "mock", "pending", "video", 0.4, 5, None, None, None, None).await.unwrap();
        db.insert_generation(
            "litegen-vid-refund-1", Some(&key.id), "mock/priced-video", "mock", "video", Some("job"), 0.4, None, None,
        ).await.unwrap();

        let client = crate::util::ssrf::no_redirect_client();
        reap_generation(&db, "litegen-vid-refund-1", "timed out", &Notify { wh_client: &client, mode: crate::config::Mode::SingleTenant, presenting: None }).await;
        assert_eq!(settle_terminal(&db, "litegen-vid-refund-1", "failed", None).await, 0.0, "a second settlement refunds nothing");

        let key = db.get_api_key(&key.id).await.unwrap().unwrap();
        assert_eq!(key.tokens_used, 0.0, "the quota is back, once");
        let row = db.get_generation("litegen-vid-refund-1").await.unwrap().unwrap();
        assert_eq!((row.status, row.cost_usd), (GenerationStatus::Failed, 0.0));
        let (logs, _) = db.get_request_logs(1, 10).await.unwrap();
        assert_eq!(logs[0].cost_usd, 0.0, "spend reads what was kept");
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
        /// Reports the video finished, with neither its bytes nor a URL.
        CompletedWithNothing,
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
                PollBehavior::CompletedWithNothing => Ok(VideoGenerationPollResult {
                    status: GenerationStatus::Completed,
                    progress: 100,
                    video_url: None,
                    video_data: None,
                    content_type: None,
                    error: None,
                    metadata: Default::default(),
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

    /// A vendor that reports a video finished but hands over nothing to keep
    /// has failed: the first tick that sees it says so (refunded), rather
    /// than retrying it for the reaper's 2 h.
    #[tokio::test]
    async fn the_poller_fails_a_video_with_nothing_to_keep_at_once() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = registry_with(PollBehavior::CompletedWithNothing).await;
        db.insert_generation(
            "litegen-vid-empty", None, "mock/video-gen", "mock", "video",
            Some("job-empty"), 0.5, None, None,
        ).await.unwrap();

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;

        let row = db.get_generation("litegen-vid-empty").await.unwrap().unwrap();
        assert_eq!(row.status, GenerationStatus::Failed, "a fresh row, failed in its first tick");
        assert!(row.error_message.as_deref().unwrap_or("").contains("neither"), "{:?}", row.error_message);
        assert_eq!(row.cost_usd, 0.0, "refunded");
    }

    /// A webhook receiver expecting `deliveries` POSTs, and a key that has it.
    async fn hooked_key(db: &Arc<dyn DatabaseStore>, deliveries: u64) -> (wiremock::MockServer, uuid::Uuid) {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).expect(deliveries).mount(&server).await;
        let hash = format!("reap-hash-{}", uuid::Uuid::new_v4().simple());
        let key = db.create_api_key("reap-key", &hash, "lg-reap", None, None, "generate,read", Some(&format!("{}/wh", server.uri()))).await.unwrap();
        (server, key.id)
    }

    /// The failed generation's webhook: the reason, as the row records it.
    async fn delivered(server: &wiremock::MockServer) -> serde_json::Value {
        for _ in 0..50 {
            if let Some(req) = server.received_requests().await.unwrap_or_default().into_iter().next() {
                return serde_json::from_slice(&req.body).unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("no webhook was delivered");
    }

    #[tokio::test]
    async fn a_non_retryable_poll_error_reaps_with_a_webhook() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let (server, key) = hooked_key(&db, 1).await;
        let registry = registry_with(PollBehavior::TerminalError).await;
        db.insert_generation("litegen-vid-reap-1", Some(&key), "mock/video-gen", "mock", "video", Some("job-r1"), 0.0, None, None).await.unwrap();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;
        let payload = delivered(&server).await;
        assert_eq!(payload["status"], "failed", "{payload}");
        assert!(payload["error_message"].as_str().unwrap_or("").contains("invalid api key"), "{payload}");
        server.verify().await;
    }

    #[tokio::test]
    async fn a_row_past_max_age_is_reaped_with_a_webhook() {
        let concrete = in_memory_db().await;
        let db: Arc<dyn DatabaseStore> = concrete.clone();
        let (server, key) = hooked_key(&db, 1).await;
        let registry = registry_with(PollBehavior::StuckProcessing).await;
        db.insert_generation("litegen-vid-reap-2", Some(&key), "mock/video-gen", "mock", "video", Some("job-r2"), 0.0, None, None).await.unwrap();
        sqlx::query("UPDATE generations SET created_at = datetime('now','-6 hours') WHERE id = ?")
            .bind("litegen-vid-reap-2")
            .execute(concrete.pool())
            .await
            .unwrap();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;
        assert_eq!(delivered(&server).await["status"], "failed");
        server.verify().await;
    }

    #[tokio::test]
    async fn a_reap_that_loses_the_race_sends_nothing() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let (server, key) = hooked_key(&db, 0).await;
        db.insert_generation("litegen-vid-reap-3", Some(&key), "mock/video-gen", "mock", "video", Some("job-r3"), 0.0, None, None).await.unwrap();
        // Another observer decided first.
        db.update_generation_if_active("litegen-vid-reap-3", "completed", 100, Some("https://cdn.example/v.mp4"), None, Some(chrono::Utc::now()), None).await.unwrap();
        let client = crate::util::ssrf::no_redirect_client();
        let notify = Notify { wh_client: &client, mode: crate::config::Mode::SingleTenant, presenting: None };
        reap_generation(&db, "litegen-vid-reap-3", "timed out awaiting provider", &notify).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let row = db.get_generation("litegen-vid-reap-3").await.unwrap().unwrap();
        assert_eq!(row.status, GenerationStatus::Completed, "the decided row stands");
        server.verify().await;
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

    // ─── Supervision and the stop (P11) ──────────────────────────────────────

    /// A vendor whose polls go wrong on purpose (a fixture for the poller, not
    /// a change to it): polling `panics_for`'s job panics, and so do the first
    /// `panics_left` polls of any job. Every other poll waits for `gate`, when
    /// there is one, and answers processing at `progress`. `polls` counts the
    /// polls that started.
    struct FaultyVideoProvider {
        panics_for: Option<&'static str>,
        panics_left: std::sync::atomic::AtomicUsize,
        progress: u8,
        polls: Arc<std::sync::atomic::AtomicUsize>,
        gate: Option<tokio::sync::watch::Receiver<bool>>,
    }

    impl FaultyVideoProvider {
        fn answering(progress: u8) -> Self {
            Self {
                panics_for: None,
                panics_left: std::sync::atomic::AtomicUsize::new(0),
                progress,
                polls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                gate: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl VideoProvider for FaultyVideoProvider {
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

        async fn poll_status(&self, h: &VideoGenerationHandle) -> Result<VideoGenerationPollResult, ProviderError> {
            use std::sync::atomic::Ordering;
            self.polls.fetch_add(1, Ordering::SeqCst);
            if self.panics_for == Some(h.provider_job_id.as_str()) {
                panic!("an answer for {} that litegen cannot handle", h.provider_job_id);
            }
            if self.panics_left.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok() {
                panic!("an answer litegen cannot handle, this once");
            }
            if let Some(gate) = &self.gate {
                let _ = gate.clone().wait_for(|open| *open).await;
            }
            Ok(VideoGenerationPollResult {
                status: GenerationStatus::Processing,
                progress: self.progress,
                video_url: None,
                video_data: None,
                content_type: None,
                error: None,
                metadata: Default::default(),
            })
        }

        async fn estimate_cost(&self, _m: &ModelSchema, _r: &VideoGenerationRequest) -> Result<CostEstimate, ProviderError> {
            unimplemented!("poller tests never estimate cost")
        }

        async fn health_check(&self) -> HealthCheckResult {
            unimplemented!("poller tests never health-check")
        }
    }

    async fn registry_of(provider: FaultyVideoProvider) -> Arc<ProviderRegistry> {
        let reg = Arc::new(ProviderRegistry::new());
        reg.register_mock_video(Arc::new(provider)).await;
        reg
    }

    async fn insert_video_row(db: &Arc<dyn DatabaseStore>, id: &str, job: &str) {
        db.insert_generation(id, None, "mock/video-gen", "mock", "video", Some(job), 0.0, None, None).await.unwrap();
    }

    #[tokio::test]
    async fn a_panic_while_polling_one_generation_does_not_stop_the_others() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = registry_of(FaultyVideoProvider { panics_for: Some("job-boom"), ..FaultyVideoProvider::answering(10) }).await;
        insert_video_row(&db, "litegen-vid-boom", "job-boom").await;
        insert_video_row(&db, "litegen-vid-fine", "job-fine").await;

        let tick = {
            let (db, registry, store) = (db.clone(), registry.clone(), test_3d_store());
            tokio::spawn(async move { poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await })
        };
        assert!(tick.await.is_ok(), "a panic polling one generation escaped the tick");
        let fine = db.get_generation("litegen-vid-fine").await.unwrap().unwrap();
        assert_eq!((fine.status, fine.progress), (GenerationStatus::Processing, 10), "the other row was polled");
    }

    #[tokio::test]
    async fn a_generation_whose_poll_keeps_panicking_is_reaped_past_max_age() {
        let concrete = in_memory_db().await;
        let db: Arc<dyn DatabaseStore> = concrete.clone();
        let registry = registry_of(FaultyVideoProvider { panics_for: Some("job-poison"), ..FaultyVideoProvider::answering(10) }).await;
        insert_video_row(&db, "litegen-vid-poison", "job-poison").await;
        sqlx::query("UPDATE generations SET created_at = datetime('now','-6 hours') WHERE id = ?")
            .bind("litegen-vid-poison")
            .execute(concrete.pool())
            .await
            .unwrap();

        let tick = {
            let (db, registry, store) = (db.clone(), registry.clone(), test_3d_store());
            tokio::spawn(async move { poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await })
        };
        let _ = tick.await;
        let row = db.get_generation("litegen-vid-poison").await.unwrap().unwrap();
        assert_eq!(row.status, GenerationStatus::Failed, "a row whose poll panics every time is reaped past the max age, as a stuck one is");
        assert!(row.error_message.as_deref().unwrap_or("").contains("inside litegen"), "{:?}", row.error_message);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn once_stopping_the_poller_starts_no_new_rows() {
        use std::sync::atomic::Ordering;
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let (open_gate, gate) = tokio::sync::watch::channel(false);
        let vendor = FaultyVideoProvider { gate: Some(gate), ..FaultyVideoProvider::answering(10) };
        let polls = vendor.polls.clone();
        let registry = registry_of(vendor).await;
        for i in 0..=POLL_CONCURRENCY {
            insert_video_row(&db, &format!("litegen-vid-stop-{i}"), &format!("job-stop-{i}")).await;
        }
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let handle = spawn_poller(db.clone(), registry, None, crate::config::Mode::SingleTenant, test_3d_store(), None, async move {
            let _ = stop_rx.await;
        });
        for _ in 0..200 {
            if polls.load(Ordering::SeqCst) >= POLL_CONCURRENCY {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(polls.load(Ordering::SeqCst), POLL_CONCURRENCY, "a full tick's worth of polls is waiting at the gate");

        stop_tx.send(()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        open_gate.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), handle)
            .await
            .expect("the poller ended after the stop")
            .expect("the poller task did not panic");
        assert_eq!(polls.load(Ordering::SeqCst), POLL_CONCURRENCY, "a row the tick had not started was polled after the stop");
    }

    /// Takes about 5 s: one real tick interval.
    #[tokio::test]
    async fn the_poller_keeps_polling_after_a_poll_panicked() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let vendor = FaultyVideoProvider { panics_left: std::sync::atomic::AtomicUsize::new(1), ..FaultyVideoProvider::answering(42) };
        let registry = registry_of(vendor).await;
        insert_video_row(&db, "litegen-vid-again", "job-again").await;
        let handle = spawn_poller(db.clone(), registry, None, crate::config::Mode::SingleTenant, test_3d_store(), None, std::future::pending::<()>());
        let mut progress = 0;
        for _ in 0..150 {
            progress = db.get_generation("litegen-vid-again").await.unwrap().unwrap().progress;
            if progress == 42 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let finished = handle.is_finished();
        handle.abort();
        assert_eq!(progress, 42, "the poller stopped polling after one panic (task finished: {finished})");
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
        Arc::new(LocalModel3dStorage::new(tempfile::tempdir().unwrap().keep(), "https://assets.test".into()))
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
                stage_context: None,
                next_stage: None,
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
                    manifest: None,
                    source_url: None,
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
        let mut p = MockModel3dProvider::new();
        p.configure(ProviderInstanceConfig::default());
        p.generate(&mock_3d_schema(model), &mock_3d_base(model, prompt), &mock_3d_extras(), &Default::default()).await.unwrap().provider_job_id
    }

    /// A text-to-3D `mock` model, free.
    fn mock_3d_schema(model: &str) -> crate::capabilities::ModelSchema {
        use crate::capabilities::{MediaType, ModelCapabilityFlags, ModelPricing, PromptSpec};
        crate::capabilities::ModelSchema {
            id: model.into(), provider: "mock".into(), media_type: MediaType::Model3d,
            task: Default::default(),
            native_orientation: None,
            preserves_unmasked: None,
            display_name: model.into(), description: String::new(), docs_url: None,
            pricing: ModelPricing { base_cost_usd: 0.0, variable_pricing: Vec::new() },
            capabilities: ModelCapabilityFlags { text_to_3d: true, ..Default::default() },
            prompt: PromptSpec { required: true, min_length: None, max_length: None },
            params: Default::default(), ref_inputs: None, extra_allowlist: vec![], extra_params: Default::default(), tags: vec![],
            deprecated_at: None, retires_at: None, replaced_by: None, status: Default::default(),
        }
    }

    fn mock_3d_base(model: &str, prompt: &str) -> crate::types::BaseGenerationRequest {
        crate::types::BaseGenerationRequest {
            prompt: prompt.into(), model: model.into(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    /// One GLB, nothing else asked for.
    fn mock_3d_extras() -> crate::providers::Model3dExtras {
        crate::providers::Model3dExtras {
            output_formats: vec![crate::types::DEFAULT_MODEL3D_FORMAT.to_string()],
            texture: None, pbr: None, target_polycount: None,
            symmetry: None, topology: None, rig: None, extra: None,
            source: None,
            animations: Vec::new(), fps: None,
            skeleton: None, joint_naming: None, height_m: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs: None, usdz_root: None, usd_clips: None, format_source: None,output_packages: None, ktx2_textures: None,
            texture_resolution: None, keep_uvs: None,
            ground: None, center: None,
        }
    }

    // ─── stages after submit (3D doc §3G) ────────────────────────────────

    /// A vendor flow with a second billed stage: stage `one` finishes, then
    /// stage `two` must be started — once — before the mesh arrives. Counts
    /// every `start_stage` call, which is what a vendor would bill.
    struct StagedModel3dProvider {
        starts: std::sync::atomic::AtomicU32,
        /// How long starting stage two takes at the vendor.
        start_delay: std::time::Duration,
        /// A context the vendor hands back on stage one's poll.
        poll_context: Option<serde_json::Value>,
    }

    fn staged_provider() -> StagedModel3dProvider {
        StagedModel3dProvider { starts: Default::default(), start_delay: std::time::Duration::from_millis(30), poll_context: None }
    }

    #[async_trait::async_trait]
    impl Model3dProvider for StagedModel3dProvider {
        fn name(&self) -> &str { "mock" }
        fn configure(&mut self, _c: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }

        async fn generate(
            &self,
            m: &crate::capabilities::ModelSchema,
            _b: &crate::types::BaseGenerationRequest,
            _e: &crate::providers::Model3dExtras,
            _mat: &crate::proxy::materializer::MaterializedRequest,
        ) -> Result<crate::providers::Model3dGenerationHandle, crate::providers::ProviderError> {
            Ok(crate::providers::Model3dGenerationHandle {
                provider_job_id: "staged-job".into(),
                provider: "mock".into(),
                model: m.id.clone(),
                stage_context: Some(serde_json::json!({ "stage": "one" })),
            })
        }

        async fn poll_status(
            &self,
            h: &crate::providers::Model3dGenerationHandle,
        ) -> Result<crate::providers::Model3dGenerationPollResult, crate::providers::ProviderError> {
            let stage = h.stage_context.as_ref().and_then(|c| c["stage"].as_str()).unwrap_or("");
            Ok(match stage {
                "one" => crate::providers::Model3dGenerationPollResult {
                    status: crate::types::GenerationStatus::Processing,
                    progress: 40,
                    files: vec![],
                    error: None,
                    metadata: Default::default(),
                    stage_context: self.poll_context.clone(),
                    next_stage: self.poll_context.is_none().then(|| "two".into()),
                },
                _ => crate::providers::Model3dGenerationPollResult {
                    status: crate::types::GenerationStatus::Completed,
                    progress: 100,
                    files: vec![crate::providers::Model3dFile {
                        kind: crate::types::Model3dAssetKind::Mesh,
                        format: "glb".into(),
                        content_type: "model/gltf-binary".into(),
                        bytes: crate::providers::model3d::glb::generate_cube_glb("staged"),
                        polycount: None,
                        width: None,
                        height: None,
                        manifest: None,
                        source_url: None,
                    }],
                    error: None,
                    metadata: Default::default(),
                    stage_context: None,
                    next_stage: None,
                },
            })
        }

        async fn start_stage(
            &self,
            _h: &crate::providers::Model3dGenerationHandle,
            stage: &str,
        ) -> Result<serde_json::Value, crate::providers::ProviderError> {
            // Widen the window in which a second observer could also start it.
            tokio::time::sleep(self.start_delay).await;
            self.starts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(serde_json::json!({ "stage": stage, "task": "vendor-task-2" }))
        }

        async fn estimate_cost(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _r: &crate::types::Model3dGenerationRequest,
        ) -> Result<crate::types::CostEstimate, crate::providers::ProviderError> {
            unimplemented!()
        }

        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            unimplemented!()
        }
    }

    fn staged_schema() -> crate::capabilities::ModelSchema {
        use crate::capabilities::{MediaType, ModelCapabilityFlags, ModelPricing, PromptSpec};
        crate::capabilities::ModelSchema {
            id: "mock/staged-3d".into(), provider: "mock".into(), media_type: MediaType::Model3d,
            task: Default::default(),
            native_orientation: None,
            preserves_unmasked: None,
            display_name: "staged".into(), description: String::new(), docs_url: None,
            pricing: ModelPricing { base_cost_usd: 0.0, variable_pricing: Vec::new() },
            capabilities: ModelCapabilityFlags { text_to_3d: true, ..Default::default() },
            prompt: PromptSpec { required: true, min_length: None, max_length: None },
            params: Default::default(), ref_inputs: None, extra_allowlist: vec![], extra_params: Default::default(), tags: vec![],
            deprecated_at: None, retires_at: None, replaced_by: None, status: Default::default(),
        }
    }

    /// A router over a registry whose mock 3D provider is `provider`, and a
    /// job submitted through it whose row exists, as the submit handler leaves it.
    async fn staged_job(
        db: &Arc<dyn DatabaseStore>,
        provider: Arc<dyn Model3dProvider>,
    ) -> (Arc<ProviderRegistry>, crate::proxy::router::ProxyRouter, String) {
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_model3d(provider).await;
        let router = crate::proxy::router::ProxyRouter::new(
            registry.clone(),
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(crate::config::AppConfig::default()),
            Arc::new(crate::proxy::storage::LocalStore),
        );
        let base = crate::types::BaseGenerationRequest {
            prompt: "a fox".into(), model: "mock/staged-3d".into(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        };
        let extras = crate::providers::Model3dExtras {
            output_formats: vec!["glb".into()],
            texture: None, pbr: None, target_polycount: None,
            symmetry: None, topology: None, rig: None, extra: None,
            source: None,
            animations: Vec::new(), fps: None,
            skeleton: None, joint_naming: None, height_m: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs: None, usdz_root: None, usd_clips: None, format_source: None,output_packages: None, ktx2_textures: None,
            texture_resolution: None, keep_uvs: None,
            ground: None, center: None,
        };
        let resp = router.generate_model3d(&staged_schema(), &base, &extras, &Default::default(), None).await.unwrap();
        db.insert_generation(&resp.id, None, "mock/staged-3d", "mock", "model3d", Some("staged-job"), 0.0, None, None).await.unwrap();
        db.merge_generation_metadata_if_active(&resp.id, &serde_json::json!({
            "requested_formats": ["glb"],
            "stage_context": { "stage": "one" },
        })).await.unwrap();
        (registry, router, resp.id)
    }

    #[tokio::test]
    async fn two_observers_start_a_billed_stage_exactly_once() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let provider = Arc::new(staged_provider());
        let (registry, router, id) = staged_job(&db, provider.clone()).await;
        let store = test_3d_store();

        // The client's poll and the poller both see stage one finish.
        let (client, _) = tokio::join!(
            router.get_model3d_status(&id, None, None, &db),
            poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store),
        );
        client.expect("the client's poll answers");
        assert_eq!(provider.starts.load(std::sync::atomic::Ordering::SeqCst), 1, "stage two is billed once");

        // Both go on to completion without starting it again.
        for _ in 0..3 {
            let _ = router.get_model3d_status(&id, None, None, &db).await;
            poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
        }
        assert_eq!(provider.starts.load(std::sync::atomic::Ordering::SeqCst), 1);
        let row = db.get_generation(&id).await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed, "{:?}", row.error_message);
    }

    /// A client that hangs up while its poll starts a billed stage does not
    /// strand it half-started: the start and its record finish anyway, so
    /// the next observer polls stage two instead of failing the generation as
    /// interrupted after the vendor billed the stage.
    #[tokio::test]
    async fn a_stage_a_hung_up_poll_was_starting_is_recorded_all_the_same() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let provider = Arc::new(StagedModel3dProvider { start_delay: std::time::Duration::from_millis(400), ..staged_provider() });
        let (_registry, router, id) = staged_job(&db, provider.clone()).await;
        let hung_up = tokio::time::timeout(std::time::Duration::from_millis(150), router.get_model3d_status(&id, None, None, &db)).await;
        assert!(hung_up.is_err(), "the client gave up while stage two was starting");
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let meta = db.get_generation(&id).await.unwrap().unwrap().metadata.unwrap();
        assert_eq!(meta["stage_context"]["stage"], "two", "{meta}");
        assert_eq!(provider.starts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A context a vendor hands back on a poll (a new task id, a result URL)
    /// reaches the row, so an observer rebuilding the handle after a restart
    /// polls with it (3D skill: stage_context).
    #[tokio::test]
    async fn a_context_a_poll_hands_back_reaches_the_row() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let ctx = serde_json::json!({ "stage": "one", "result_url": "https://vendor.example/r/7" });
        let provider = Arc::new(StagedModel3dProvider { poll_context: Some(ctx.clone()), ..staged_provider() });
        let (_registry, router, id) = staged_job(&db, provider).await;
        router.get_model3d_status(&id, None, None, &db).await.unwrap();
        let meta = db.get_generation(&id).await.unwrap().unwrap().metadata.unwrap();
        assert_eq!(meta["stage_context"], ctx, "{meta}");
    }

    #[tokio::test]
    async fn a_started_stage_reaches_the_row_for_the_next_observer() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let provider = Arc::new(staged_provider());
        let (registry, router, id) = staged_job(&db, provider.clone()).await;

        // The client's poll starts stage two…
        let resp = router.get_model3d_status(&id, None, None, &db).await.unwrap();
        assert_eq!(resp.status, crate::types::GenerationStatus::Processing);
        let row = db.get_generation(&id).await.unwrap().unwrap();
        let meta = row.metadata.unwrap();
        assert_eq!(meta["stage_context"]["stage"], "two", "{meta}");
        assert_eq!(meta["stage_context"]["task"], "vendor-task-2", "{meta}");

        // …and a poller rebuilding the handle from the row (a restart) polls
        // stage two, never stage one again.
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;
        let row = db.get_generation(&id).await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed, "{:?}", row.error_message);
        assert_eq!(provider.starts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_interrupted_stage_start_fails_the_generation_instead_of_resending() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let provider = Arc::new(staged_provider());
        let (registry, _router, id) = staged_job(&db, provider.clone()).await;
        // A claim from a process that died before it could record stage two.
        let at = (chrono::Utc::now() - chrono::Duration::minutes(11)).to_rfc3339();
        db.merge_generation_metadata_if_active(&id, &serde_json::json!({
            "stage_claims": { "two": { "by": "a-dead-process", "at": at } },
        })).await.unwrap();

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;
        let row = db.get_generation(&id).await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Failed);
        assert!(row.error_message.as_deref().unwrap_or("").contains("interrupted"), "{:?}", row.error_message);
        assert_eq!(provider.starts.load(std::sync::atomic::Ordering::SeqCst), 0, "never re-sent");
    }

    #[tokio::test]
    async fn a_fresh_claim_by_another_observer_just_waits() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let provider = Arc::new(staged_provider());
        let (registry, _router, id) = staged_job(&db, provider.clone()).await;
        db.merge_generation_metadata_if_active(&id, &serde_json::json!({
            "stage_claims": { "two": { "by": "the-client", "at": chrono::Utc::now().to_rfc3339() } },
        })).await.unwrap();

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;
        let row = db.get_generation(&id).await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Processing);
        assert_eq!(provider.starts.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// 3D doc §3D: whichever observer sees a job finish, the caller gets the
    /// same files — the client's poll and the poller run one delivery step
    /// from one plan.
    #[tokio::test]
    async fn the_client_and_the_poller_deliver_identical_assets() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_3d_registry().await;
        let store = test_3d_store();
        let router = crate::proxy::router::ProxyRouter::new(
            registry.clone(),
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(crate::config::AppConfig::default()),
            Arc::new(crate::proxy::storage::LocalStore),
        );
        let mut schema = staged_schema();
        schema.id = "mock/turned-3d".into();
        schema.native_orientation = Some(crate::capabilities::NativeOrientation {
            up: "+y".into(), front: "+x".into(), evidence: "the mock builds it so".into(),
        });
        let base = crate::types::BaseGenerationRequest {
            prompt: "a knight".into(), model: schema.id.clone(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        };
        let extras = crate::providers::Model3dExtras {
            output_formats: vec!["glb".into(), "obj".into()],
            texture: None, pbr: None, target_polycount: None,
            symmetry: None, topology: None, rig: None, extra: None,
            source: None,
            animations: Vec::new(), fps: None,
            skeleton: None, joint_naming: None, height_m: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs: None, usdz_root: None, usd_clips: None, format_source: None,output_packages: None, ktx2_textures: None,
            texture_resolution: None, keep_uvs: None,
            ground: None, center: None,
        };
        let plan = crate::proxy::model3d_deliver::DeliveryPlan::for_request(&schema, &extras);

        let mut seen = Vec::new();
        for observer in ["client", "poller"] {
            let resp = router.generate_model3d(&schema, &base, &extras, &Default::default(), None).await.unwrap();
            let job = router.get_model3d_provider_job_id(&resp.id).await;
            db.insert_generation(&resp.id, None, &schema.id, "mock", "model3d", job.as_deref(), 0.0, None, None).await.unwrap();
            db.merge_generation_metadata_if_active(&resp.id, &serde_json::json!({
                "requested_formats": extras.output_formats, "delivery": plan,
            })).await.unwrap();
            let mut assets = Vec::new();
            if observer == "client" {
                for _ in 0..5 {
                    let r = router.get_model3d_status(&resp.id, None, None, &db).await.unwrap();
                    if r.status == crate::types::GenerationStatus::Completed {
                        assets = r.assets;
                        break;
                    }
                }
            } else {
                router.forget_model3d_job(&resp.id).await;
                for _ in 0..5 {
                    poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
                    let row = db.get_generation(&resp.id).await.unwrap().unwrap();
                    if row.status == crate::types::GenerationStatus::Completed {
                        assets = serde_json::from_value(row.metadata.unwrap()["assets"].clone()).unwrap();
                        break;
                    }
                }
            }
            assert!(!assets.is_empty(), "the {observer} saw the job finish");
            seen.push(assets);
        }
        // Byte for byte, with the same manifests: only where they are stored differs.
        let files = |a: &[crate::types::Model3dAsset]| {
            a.iter().map(|a| (a.kind, a.format.clone(), a.sha256.clone(), a.scene.clone())).collect::<Vec<_>>()
        };
        assert_eq!(files(&seen[0]), files(&seen[1]));
        assert!(seen[0].iter().any(|a| a.kind == crate::types::Model3dAssetKind::Original), "{:?}", files(&seen[0]));
        assert!(seen[0].iter().filter(|a| a.kind == crate::types::Model3dAssetKind::Mesh)
            .all(|a| a.scene.as_ref().is_some_and(|s| s.front == "+z" && s.oriented_by == "litegen")));
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
    async fn poll_once_hands_out_the_vendors_3d_files_for_a_row_that_keeps_nothing() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_3d_registry().await;
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ImageStorage> = Arc::new(LocalModel3dStorage::new(dir.path(), "https://assets.test".into()));
        let job_id = submit_mock_3d("mock/mesh-3d", "a fox").await;
        let id = "litegen-3d-keep-nothing";
        db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", Some(&job_id), 0.0, None, None).await.unwrap();
        db.merge_generation_metadata_if_active(
            id,
            &serde_json::json!({ "keep_outputs": false, "delivery": { "requested_formats": ["glb"], "keep_nothing": true } }),
        )
        .await
        .unwrap();
        for _ in 0..5 {
            poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
            if db.get_generation(id).await.unwrap().unwrap().status.is_terminal() {
                break;
            }
        }
        let row = db.get_generation(id).await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed, "{:?}", row.error_message);
        let url = row.result_url.expect("the vendor's mesh");
        assert!(url.starts_with("https://example.com/mock-3d/"), "the vendor's own link: {url}");
        let meta = row.metadata.expect("metadata");
        assert!(meta["assets"].as_array().unwrap().iter().all(|a| a["kept"] == false), "{meta}");
        let written = walk_files(dir.path());
        assert!(written.is_empty(), "nothing written: {written:?}");
    }

    fn walk_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let mut dirs = vec![dir.to_path_buf()];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                if e.path().is_dir() { dirs.push(e.path()) } else { found.push(e.path()) }
            }
        }
        found
    }

    /// L3: a finished video is kept on litegen storage in the tick that saw
    /// it finish — the vendor's URL may expire within the hour.
    #[tokio::test]
    async fn poll_once_keeps_a_finished_video_on_litegen_storage() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_registry().await; // video mock
        let store = test_3d_store();
        db.insert_generation(
            "litegen-vid-kept", None, "mock/video-gen", "mock", "video",
            Some("mock-video-job-1"), 0.0, None, None,
        ).await.unwrap();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
        let row = db.get_generation("litegen-vid-kept").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed);
        let url = row.result_url.expect("a result url");
        assert!(url.starts_with("https://assets.test/"), "litegen's copy, not the vendor's: {url}");
        let asset = &row.metadata.expect("metadata")["assets"][0];
        assert_eq!(asset["kind"], "video", "{asset}");
        let key = store.key_of(&url).expect("our own url");
        let (bytes, _) = store.get(&key).await.unwrap().expect("stored");
        assert_eq!(asset["sha256"], crate::proxy::router::sha256_hex(&bytes));
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

    // ─── A store that never accepts the assets ────────────────────────────

    /// Every upload fails, the way a BYO bucket with a revoked key does.
    struct RejectingStore;

    #[async_trait::async_trait]
    impl ImageStorage for RejectingStore {
        async fn put(&self, _k: &str, _b: &bytes::Bytes, _ct: &str) -> Result<String, crate::proxy::storage::ImageStoreError> {
            Err(crate::proxy::storage::ImageStoreError::UploadFailed("S3 returned HTTP 403".into()))
        }
        async fn delete(&self, _k: &str) -> Result<(), crate::proxy::storage::ImageStoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_3d_row_whose_assets_never_store_is_reaped_past_max_age() {
        let concrete = in_memory_db().await;
        let db: Arc<dyn DatabaseStore> = concrete.clone();
        let registry = registry_with_3d_mesh_missing().await; // reports Completed with a file
        db.insert_generation(
            "litegen-3d-no-store", None, "mock/mesh-3d", "mock", "model3d",
            Some("job-no-store"), 0.0, None, None,
        ).await.unwrap();
        sqlx::query("UPDATE generations SET created_at = datetime('now','-6 hours') WHERE id = ?")
            .bind("litegen-3d-no-store")
            .execute(concrete.pool())
            .await
            .unwrap();

        let store: Arc<dyn ImageStorage> = Arc::new(RejectingStore);
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        let row = db.get_generation("litegen-3d-no-store").await.unwrap().unwrap();
        assert_eq!(row.status, GenerationStatus::Failed, "a permanent storage failure must not keep the row active forever");
        assert!(
            row.error_message.as_deref().is_some_and(|e| e.contains("could not store the 3d assets")),
            "{:?}", row.error_message
        );
    }

    #[tokio::test]
    async fn a_fresh_3d_row_whose_assets_fail_to_store_is_retried() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = registry_with_3d_mesh_missing().await;
        db.insert_generation(
            "litegen-3d-store-retry", None, "mock/mesh-3d", "mock", "model3d",
            Some("job-store-retry"), 0.0, None, None,
        ).await.unwrap();

        let store: Arc<dyn ImageStorage> = Arc::new(RejectingStore);
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        let row = db.get_generation("litegen-3d-store-retry").await.unwrap().unwrap();
        assert!(!row.status.is_terminal(), "within max age a storage failure is retried, got {:?}", row.status);
    }

    // ─── one delivery per finished job (2026-10-12 review #30) ───────────

    /// A vendor job that is already done: every poll is `Completed` with one
    /// GLB, so whichever observer polls first may deliver it.
    struct DoneModel3dProvider;

    #[async_trait::async_trait]
    impl Model3dProvider for DoneModel3dProvider {
        fn name(&self) -> &str { "mock" }
        fn configure(&mut self, _c: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }
        async fn generate(
            &self,
            m: &crate::capabilities::ModelSchema,
            _b: &crate::types::BaseGenerationRequest,
            _e: &crate::providers::Model3dExtras,
            _mat: &crate::proxy::materializer::MaterializedRequest,
        ) -> Result<crate::providers::Model3dGenerationHandle, crate::providers::ProviderError> {
            Ok(crate::providers::Model3dGenerationHandle {
                provider_job_id: "done-job".into(),
                provider: "mock".into(),
                model: m.id.clone(),
                stage_context: None,
            })
        }
        async fn poll_status(
            &self,
            _h: &crate::providers::Model3dGenerationHandle,
        ) -> Result<crate::providers::Model3dGenerationPollResult, crate::providers::ProviderError> {
            Ok(crate::providers::Model3dGenerationPollResult {
                status: crate::types::GenerationStatus::Completed,
                progress: 100,
                files: vec![crate::providers::Model3dFile {
                    kind: crate::types::Model3dAssetKind::Mesh,
                    format: "glb".into(),
                    content_type: "model/gltf-binary".into(),
                    bytes: crate::providers::model3d::glb::generate_cube_glb("done"),
                    polycount: None,
                    width: None,
                    height: None,
                    manifest: None,
                    source_url: None,
                }],
                error: None,
                metadata: Default::default(),
                stage_context: None,
                next_stage: None,
            })
        }
        async fn estimate_cost(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _r: &crate::types::Model3dGenerationRequest,
        ) -> Result<crate::types::CostEstimate, crate::providers::ProviderError> {
            unimplemented!()
        }
        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            unimplemented!()
        }
    }

    /// A 3D store that counts what it is asked to keep. Delivering a one-mesh
    /// job stores one file, so the count is the number of deliveries.
    struct CountingStore {
        inner: Arc<dyn ImageStorage>,
        puts: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ImageStorage for CountingStore {
        async fn put(&self, key: &str, bytes: &bytes::Bytes, content_type: &str) -> Result<String, crate::proxy::storage::ImageStoreError> {
            self.puts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.put(key, bytes, content_type).await
        }
        async fn delete(&self, key: &str) -> Result<(), crate::proxy::storage::ImageStoreError> {
            self.inner.delete(key).await
        }
    }

    fn counting_store() -> (Arc<CountingStore>, Arc<dyn ImageStorage>) {
        let counting = Arc::new(CountingStore { inner: test_3d_store(), puts: Default::default() });
        let store: Arc<dyn ImageStorage> = counting.clone();
        (counting, store)
    }

    fn puts(c: &CountingStore) -> usize {
        c.puts.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test]
    async fn the_poller_stands_aside_while_another_observer_delivers() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let (registry, _router, id) = staged_job(&db, Arc::new(DoneModel3dProvider)).await;
        let (counting, store) = counting_store();
        assert!(db.lease_generation_work(&id, "delivery", "elsewhere", chrono::Duration::minutes(5)).await.unwrap());

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
        assert_eq!(puts(&counting), 0, "nothing converted or stored while the delivery is another's");
        let row = db.get_generation(&id).await.unwrap().unwrap();
        assert!(!row.status.is_terminal(), "{:?}", row.status);

        db.release_generation_work(&id, "delivery", "elsewhere").await.unwrap();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
        assert_eq!(puts(&counting), 1);
        let row = db.get_generation(&id).await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed, "{:?}", row.error_message);
    }

    #[tokio::test]
    async fn a_client_poll_stands_aside_while_another_observer_delivers() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let (_registry, router, id) = staged_job(&db, Arc::new(DoneModel3dProvider)).await;
        let (counting, store) = counting_store();
        assert!(db.lease_generation_work(&id, "delivery", "elsewhere", chrono::Duration::minutes(5)).await.unwrap());

        let resp = router.get_model3d_status(&id, Some(store.clone()), None, &db).await.unwrap();
        assert_eq!(resp.status, crate::types::GenerationStatus::Processing, "the holder's outcome is still to come");
        assert!(resp.assets.is_empty() && resp.error.is_none(), "{resp:?}");
        assert!(resp.progress < 100);
        assert_eq!(puts(&counting), 0);
    }

    /// Review 2026-11-11 P4: a finished video is kept by one observer. While
    /// another holds its delivery, the poller keeps nothing and leaves the
    /// row in flight.
    #[tokio::test]
    async fn the_poller_stands_aside_while_another_observer_keeps_the_video() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_registry().await;
        db.insert_generation("g-vid-held", None, "mock/video-gen", "mock", "video", Some("mock-video-job-1"), 0.0, None, None).await.unwrap();
        assert!(db.lease_generation_work("g-vid-held", "delivery", "elsewhere", chrono::Duration::minutes(5)).await.unwrap());
        let (counting, store) = counting_store();

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
        assert_eq!(puts(&counting), 0, "the video was kept while another observer held its delivery");
        let row = db.get_generation("g-vid-held").await.unwrap().unwrap();
        assert!(!row.status.is_terminal(), "{:?}", row.status);

        db.release_generation_work("g-vid-held", "delivery", "elsewhere").await.unwrap();
        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
        assert_eq!(puts(&counting), 1, "kept once the delivery is free");
        let row = db.get_generation("g-vid-held").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed, "{:?}", row.error_message);
    }

    /// Review 2026-11-11 P4 (Step D): Veo hands its video over as bytes it
    /// downloads in the poll; the poller downloads none while another
    /// observer keeps it.
    #[tokio::test]
    async fn the_poller_downloads_no_veo_video_while_another_observer_keeps_it() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let uri = format!("{}/v1beta/files/abc:download?alt=media", server.uri());
        Mock::given(method("GET"))
            .and(path("/v1beta/files/abc:download"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-type", "video/mp4").set_body_bytes(b"MP4BYTES".to_vec()))
            .mount(&server)
            .await;
        let op = serde_json::json!({ "name": "models/veo-3.0-generate-001/operations/op1", "done": true,
            "response": { "generateVideoResponse": { "generatedSamples": [{ "video": { "uri": uri } }] } } });
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(op)).mount(&server).await;
        let mut veo = crate::providers::video::google::GoogleVideoProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "test-key".into(), api_base: Some(server.uri()), ..Default::default() };
        cfg.credentials.api_key = Some("test-key".into());
        veo.configure(cfg);
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_video(Arc::new(veo)).await;
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let id = "g-veo-held";
        db.insert_generation(id, None, "google/veo-3.0-generate-001", "mock", "video", Some("models/veo-3.0-generate-001/operations/op1"), 0.0, None, None)
            .await
            .unwrap();
        assert!(db.lease_generation_work(id, "delivery", "elsewhere", chrono::Duration::minutes(5)).await.unwrap());

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;
        let n = server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/v1beta/files/abc:download").count();
        assert_eq!(n, 0, "the video was downloaded {n} time(s) while another observer kept it");
        assert!(!db.get_generation(id).await.unwrap().unwrap().status.is_terminal());
    }

    /// Review 2026-11-11 P5: an observer that does not hold the delivery
    /// downloads nothing. A real adapter (Tripo, on wiremock) downloads a
    /// finished job's files inside its poll, so the claim must come first.
    #[tokio::test]
    async fn the_poller_downloads_nothing_while_another_observer_delivers() {
        use crate::providers::model3d::tripo::TripoModel3dProvider;
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let (server, tripo) = TripoModel3dProvider::finished_rig_for_tests().await;
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_model3d(Arc::new(tripo)).await;
        db.insert_generation("g-tripo", None, "tripo/rig", "mock", "model3d", Some("rig-1"), 0.0, None, None).await.unwrap();
        assert!(db.lease_generation_work("g-tripo", "delivery", "elsewhere", chrono::Duration::minutes(5)).await.unwrap());

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;
        let n = TripoModel3dProvider::rig_downloads_for_tests(&server).await;
        assert_eq!(n, 0, "the mesh was downloaded {n} time(s) while another observer held the delivery");
        let row = db.get_generation("g-tripo").await.unwrap().unwrap();
        assert!(!row.status.is_terminal(), "{:?}", row.status);
    }

    /// The client's poll and the poller both see the job finish. One of them
    /// converts and stores it; the other stands aside, and the row completes.
    #[tokio::test]
    async fn two_observers_deliver_a_finished_job_exactly_once() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let (registry, router, id) = staged_job(&db, Arc::new(DoneModel3dProvider)).await;
        let (counting, store) = counting_store();

        let (client, _) = tokio::join!(
            router.get_model3d_status_claimed(&id, Some(store.clone()), None, &db),
            poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store),
        );
        let (client, claim) = client.expect("the client's poll answers");
        assert_eq!(puts(&counting), 1, "one observer delivered the job");
        if client.status == crate::types::GenerationStatus::Completed {
            // The client won: its handler persists the outcome, holding the
            // claim until it has, as here.
            assert!(claim.is_some());
            let url = client.mesh().map(|m| m.url.clone());
            db.update_generation_if_active(&id, "completed", 100, url.as_deref(), None, Some(chrono::Utc::now()), None)
                .await
                .unwrap();
            drop(claim);
        } else {
            assert_eq!(client.status, crate::types::GenerationStatus::Processing, "{client:?}");
            assert!(client.assets.is_empty());
        }
        let row = db.get_generation(&id).await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed, "{:?}", row.error_message);
    }

    // ─── rows are polled side by side (2026-10-12 review #30) ────────────

    /// One vendor job hangs until `release` is notified; any other job is
    /// already done.
    struct GatedModel3dProvider {
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl Model3dProvider for GatedModel3dProvider {
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
            unimplemented!()
        }
        async fn poll_status(
            &self,
            h: &crate::providers::Model3dGenerationHandle,
        ) -> Result<crate::providers::Model3dGenerationPollResult, crate::providers::ProviderError> {
            if h.provider_job_id == "slow-job" {
                self.release.notified().await;
            }
            DoneModel3dProvider.poll_status(h).await
        }
        async fn estimate_cost(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _r: &crate::types::Model3dGenerationRequest,
        ) -> Result<crate::types::CostEstimate, crate::providers::ProviderError> {
            unimplemented!()
        }
        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            unimplemented!()
        }
    }

    /// A row that is slow to poll (a vendor that hangs, a large delivery)
    /// must not hold every row behind it until the next tick.
    #[tokio::test]
    async fn one_slow_row_does_not_hold_up_the_rest_of_the_tick() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let release = Arc::new(tokio::sync::Notify::new());
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_model3d(Arc::new(GatedModel3dProvider { release: release.clone() })).await;
        // Oldest first: the slow row is polled first.
        db.insert_generation("g-slow", None, "mock/mesh-3d", "mock", "model3d", Some("slow-job"), 0.0, None, None).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        db.insert_generation("g-quick", None, "mock/mesh-3d", "mock", "model3d", Some("quick-job"), 0.0, None, None).await.unwrap();
        let store = test_3d_store();

        let tick = {
            let (db, registry, store) = (db.clone(), registry.clone(), store.clone());
            tokio::spawn(async move { poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await })
        };
        let quick_done = async {
            loop {
                let row = db.get_generation("g-quick").await.unwrap().unwrap();
                if row.status.is_terminal() {
                    return row.status;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        let quick = tokio::time::timeout(std::time::Duration::from_secs(10), quick_done).await;
        release.notify_one();
        tick.await.unwrap();
        assert_eq!(quick.expect("the quick row finished while the slow one was still polling"), crate::types::GenerationStatus::Completed);
        let slow = db.get_generation("g-slow").await.unwrap().unwrap();
        assert_eq!(slow.status, crate::types::GenerationStatus::Completed, "and the slow row still finishes");
    }

    // ─── replicas claim the rows they poll (horizontal scaling) ───────────

    /// One generation per entry of `statuses`, in that status, oldest first.
    async fn sqlite_with_rows(statuses: &[&str]) -> Arc<SqliteDatabase> {
        let db = in_memory_db().await;
        for (i, status) in statuses.iter().enumerate() {
            let id = format!("g-claim-{i}");
            db.insert_generation(&id, None, "mock/mesh-3d", "mock", "model3d", Some(&format!("job-{i}")), 0.0, None, None)
                .await
                .unwrap();
            if *status != "pending" {
                let done = matches!(*status, "completed" | "failed" | "cancelled");
                db.update_generation_status(&id, status, 50, None, None, done.then(chrono::Utc::now)).await.unwrap();
            }
        }
        db
    }

    /// `n` active rows, half `pending` and half `processing`.
    async fn sqlite_with_active_rows(n: usize) -> Arc<SqliteDatabase> {
        let statuses: Vec<&str> = (0..n).map(|i| if i % 2 == 0 { "pending" } else { "processing" }).collect();
        sqlite_with_rows(&statuses).await
    }

    fn ids(rows: Vec<crate::types::Generation>) -> Vec<String> {
        let mut ids: Vec<String> = rows.into_iter().map(|g| g.id).collect();
        ids.sort();
        ids
    }

    #[tokio::test]
    async fn two_claimants_take_disjoint_rows() {
        let db = sqlite_with_active_rows(6).await;
        let a = db.claim_active_generations(3, "a", POLL_CLAIM_HOLD).await.unwrap();
        let b = db.claim_active_generations(10, "b", POLL_CLAIM_HOLD).await.unwrap();
        assert_eq!((a.len(), b.len()), (3, 3));
        assert!(a.iter().all(|r| b.iter().all(|s| s.id != r.id)));
    }

    #[tokio::test]
    async fn a_claimant_keeps_its_rows_tick_after_tick() {
        let db = sqlite_with_active_rows(2).await;
        let first = ids(db.claim_active_generations(10, "a", POLL_CLAIM_HOLD).await.unwrap());
        assert_eq!(first.len(), 2);
        assert_eq!(ids(db.claim_active_generations(10, "a", POLL_CLAIM_HOLD).await.unwrap()), first);
        assert!(db.claim_active_generations(10, "b", POLL_CLAIM_HOLD).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_lapsed_claim_passes_to_another_claimant() {
        let db = sqlite_with_active_rows(1).await;
        assert_eq!(db.claim_active_generations(10, "a", chrono::Duration::seconds(1)).await.unwrap().len(), 1);
        assert!(
            db.claim_active_generations(10, "b", POLL_CLAIM_HOLD).await.unwrap().is_empty(),
            "while the hold lasts the row is a's"
        );
        // 1.2 s on a 1 s hold: a slow machine can't make b's first claim fall
        // after the hold, and the lapse itself is kept to the millisecond
        // (SQLite's CURRENT_TIMESTAMP only has 1 s resolution).
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        assert_eq!(db.claim_active_generations(10, "b", POLL_CLAIM_HOLD).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_claim_takes_the_oldest_rows_first() {
        let db = sqlite_with_active_rows(6).await;
        // Row i is the (6 - i)th oldest: the newest row was inserted first.
        for i in 0..6 {
            sqlx::query("UPDATE generations SET created_at = ? WHERE id = ?")
                .bind(format!("2020-01-01 00:00:0{}", 9 - i))
                .bind(format!("g-claim-{i}"))
                .execute(db.pool())
                .await
                .unwrap();
        }
        let oldest = db.claim_active_generations(3, "a", POLL_CLAIM_HOLD).await.unwrap();
        assert_eq!(ids(oldest), ["g-claim-3", "g-claim-4", "g-claim-5"]);
        let rest = db.claim_active_generations(10, "b", POLL_CLAIM_HOLD).await.unwrap();
        assert_eq!(ids(rest), ["g-claim-0", "g-claim-1", "g-claim-2"]);
    }

    /// A replica with more active rows than one claim takes must not re-take
    /// the same oldest ones every tick (P2): its claims go round all of them.
    #[tokio::test]
    async fn a_claimant_with_more_active_rows_than_its_limit_takes_them_in_turn() {
        let db = sqlite_with_active_rows(5).await;
        // Distinct old creation times, row 0 the oldest.
        for i in 0..5 {
            sqlx::query("UPDATE generations SET created_at = ? WHERE id = ?")
                .bind(format!("2020-01-01 00:00:0{i}"))
                .bind(format!("g-claim-{i}"))
                .execute(db.pool())
                .await
                .unwrap();
        }
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..3 {
            seen.extend(ids(db.claim_active_generations(2, "a", POLL_CLAIM_HOLD).await.unwrap()));
        }
        assert_eq!(seen.len(), 5, "three claims of two take all five active rows in turn: {seen:?}");
    }

    /// A replica that stops gracefully hands its rows back, so the others take
    /// them at their next tick and not 30 s later. What it never held, and what
    /// another replica holds, stays as it was.
    #[tokio::test]
    async fn releasing_a_claimants_claims_frees_its_rows_at_once_and_only_its_rows() {
        let db = sqlite_with_active_rows(6).await;
        let a = ids(db.claim_active_generations(2, "a", POLL_CLAIM_HOLD).await.unwrap());
        let c = ids(db.claim_active_generations(2, "c", POLL_CLAIM_HOLD).await.unwrap());
        assert_eq!((a.len(), c.len()), (2, 2));

        // A claim is never cleared when its row turns terminal. So one of a's
        // rows finishes first: the release must neither rewrite nor count it
        // (on Postgres that would also be a scan of every finished row ever
        // polled).
        let finished = a[0].clone();
        db.update_generation_status(&finished, "completed", 100, None, None, Some(chrono::Utc::now())).await.unwrap();
        let claimed_by = |id: String| {
            let db = db.clone();
            async move {
                sqlx::query_scalar::<_, Option<String>>("SELECT poll_claimed_by FROM generations WHERE id = ?")
                    .bind(id)
                    .fetch_one(db.pool())
                    .await
                    .unwrap()
            }
        };
        assert_eq!(claimed_by(finished.clone()).await.as_deref(), Some("a"), "the finished row still names a");

        assert_eq!(db.release_poll_claims("a").await.unwrap(), 1, "a held one active row (the finished one isn't counted)");
        assert_eq!(claimed_by(finished.clone()).await.as_deref(), Some("a"), "and the finished row was left as it was");
        let b = ids(db.claim_active_generations(10, "b", POLL_CLAIM_HOLD).await.unwrap());
        assert_eq!(b.len(), 3, "a's active row and the two nobody held: {b:?}");
        assert!(b.contains(&a[1]), "b took a's active row at once: a={a:?} b={b:?}");
        assert!(!b.contains(&finished), "a finished row is never claimed: {b:?}");
        assert!(c.iter().all(|id| !b.contains(id)), "and none of c's: c={c:?} b={b:?}");

        assert_eq!(db.release_poll_claims("a").await.unwrap(), 0, "a has nothing left to release");
        assert_eq!(db.release_poll_claims("nobody").await.unwrap(), 0);
        assert_eq!(ids(db.claim_active_generations(10, "c", POLL_CLAIM_HOLD).await.unwrap()), c, "c still holds its own");
    }

    #[tokio::test]
    async fn terminal_rows_are_never_claimed() {
        let db = sqlite_with_rows(&["completed", "failed", "cancelled"]).await;
        assert!(db.claim_active_generations(10, "a", POLL_CLAIM_HOLD).await.unwrap().is_empty());
    }

    /// A 3D vendor whose jobs never finish, counting every poll of each job.
    #[derive(Default)]
    struct CountingModel3dProvider {
        polls: std::sync::Mutex<std::collections::HashMap<String, usize>>,
    }

    impl CountingModel3dProvider {
        fn polls_per_job(&self) -> std::collections::HashMap<String, usize> {
            self.polls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Model3dProvider for CountingModel3dProvider {
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
            unimplemented!()
        }
        async fn poll_status(
            &self,
            h: &crate::providers::Model3dGenerationHandle,
        ) -> Result<crate::providers::Model3dGenerationPollResult, crate::providers::ProviderError> {
            *self.polls.lock().unwrap().entry(h.provider_job_id.clone()).or_default() += 1;
            // Still running, so the row stays active for the next replica's tick.
            Ok(crate::providers::Model3dGenerationPollResult {
                status: crate::types::GenerationStatus::Processing,
                progress: 50,
                files: vec![],
                error: None,
                metadata: Default::default(),
                stage_context: None,
                next_stage: None,
            })
        }
        async fn estimate_cost(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _r: &crate::types::Model3dGenerationRequest,
        ) -> Result<crate::types::CostEstimate, crate::providers::ProviderError> {
            unimplemented!()
        }
        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            unimplemented!()
        }
    }

    /// Two replicas ticking one after the other: the second finds every row
    /// the first claimed taken, so no vendor job is polled twice.
    #[tokio::test]
    async fn two_pollers_poll_each_job_once_per_tick() {
        let db: Arc<dyn DatabaseStore> = sqlite_with_active_rows(4).await;
        let provider = Arc::new(CountingModel3dProvider::default());
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_model3d(provider.clone()).await;
        let store = test_3d_store();

        poll_once_as("a", &db, &registry, None, crate::config::Mode::SingleTenant, &store, None).await;
        poll_once_as("b", &db, &registry, None, crate::config::Mode::SingleTenant, &store, None).await;

        let polls = provider.polls_per_job();
        assert_eq!(polls.len(), 4, "every job was polled: {polls:?}");
        assert!(polls.values().all(|n| *n == 1), "and none twice: {polls:?}");
    }

    /// More active rows than one tick claims (P2): the rows a tick could not
    /// take are polled in the next one, not starved behind the oldest 100.
    #[tokio::test]
    async fn every_active_job_is_polled_when_one_tick_cannot_claim_them_all() {
        let sqlite = sqlite_with_active_rows(105).await;
        // Row i made 30 min ago plus i seconds: well inside the maximum age,
        // so no row is reaped, and distinct times keep the order fixed.
        for i in 0..105 {
            sqlx::query("UPDATE generations SET created_at = datetime('now', '-30 minutes', '+' || ? || ' seconds') WHERE id = ?")
                .bind(i as i64)
                .bind(format!("g-claim-{i}"))
                .execute(sqlite.pool())
                .await
                .unwrap();
        }
        let db: Arc<dyn DatabaseStore> = sqlite;
        let provider = Arc::new(CountingModel3dProvider::default());
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_model3d(provider.clone()).await;
        let store = test_3d_store();

        for _ in 0..2 {
            poll_once_as("a", &db, &registry, None, crate::config::Mode::SingleTenant, &store, None).await;
        }

        // Guard: no row was reaped, so what follows comes from the claims.
        assert_eq!(db.list_active_generations(1000).await.unwrap().len(), 105, "every row is still active");
        let polls = provider.polls_per_job();
        let never: Vec<String> = (0..105).map(|i| format!("job-{i}")).filter(|j| !polls.contains_key(j)).collect();
        assert!(never.is_empty(), "{} of 105 jobs were never polled in two ticks: {never:?}", never.len());
    }

    // ─── a long tick keeps the rows it is polling (R7) ────────────────────

    /// Renewing extends only the claimant's own active rows: past its first
    /// hold the renewed row is still its own, and nothing is free for another.
    #[tokio::test]
    async fn renewing_poll_claims_extends_only_the_claimants_active_rows() {
        let db = sqlite_with_active_rows(3).await;
        let a = ids(db.claim_active_generations(2, "a", chrono::Duration::seconds(1)).await.unwrap());
        let b = ids(db.claim_active_generations(10, "b", POLL_CLAIM_HOLD).await.unwrap());
        assert_eq!((a.len(), b.len()), (2, 1));
        db.update_generation_status(&a[0], "completed", 100, None, None, Some(chrono::Utc::now())).await.unwrap();
        let all: Vec<String> = a.iter().chain(&b).cloned().collect();
        assert_eq!(
            db.renew_poll_claims("a", &all, POLL_CLAIM_HOLD).await.unwrap(),
            1,
            "only a's active row is a's to renew (not its finished one, not b's)"
        );
        // 1.2 s on a 1 s hold, as in `a_lapsed_claim_passes_to_another_claimant`.
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        let c = ids(db.claim_active_generations(10, "c", POLL_CLAIM_HOLD).await.unwrap());
        assert!(c.is_empty(), "a's renewed row is still a's and the third is b's: {c:?}");
    }

    /// A 3D vendor counting every poll of each job. A `slow-*` job's poll
    /// waits at the gate until [`Self::open`]; then every job answers as
    /// [`DoneModel3dProvider`] does.
    struct GatedCountingModel3dProvider {
        polls: std::sync::Mutex<std::collections::HashMap<String, usize>>,
        gate: tokio::sync::Semaphore,
    }

    impl GatedCountingModel3dProvider {
        fn new() -> Self {
            Self { polls: Default::default(), gate: tokio::sync::Semaphore::new(0) }
        }
        fn polls_per_job(&self) -> std::collections::HashMap<String, usize> {
            self.polls.lock().unwrap().clone()
        }
        fn total(&self) -> usize {
            self.polls.lock().unwrap().values().sum()
        }
        /// One permit, which each waiting poll takes and hands back in turn.
        fn open(&self) {
            self.gate.add_permits(1);
        }
    }

    #[async_trait::async_trait]
    impl Model3dProvider for GatedCountingModel3dProvider {
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
            unimplemented!()
        }
        async fn poll_status(
            &self,
            h: &crate::providers::Model3dGenerationHandle,
        ) -> Result<crate::providers::Model3dGenerationPollResult, crate::providers::ProviderError> {
            *self.polls.lock().unwrap().entry(h.provider_job_id.clone()).or_default() += 1;
            if h.provider_job_id.starts_with("slow-") {
                drop(self.gate.acquire().await.unwrap());
            }
            DoneModel3dProvider.poll_status(h).await
        }
        async fn estimate_cost(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _r: &crate::types::Model3dGenerationRequest,
        ) -> Result<crate::types::CostEstimate, crate::providers::ProviderError> {
            unimplemented!()
        }
        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            unimplemented!()
        }
    }

    /// Waits (10 s at most) until `done` holds.
    async fn until(what: &str, done: impl Fn() -> bool) {
        let wait = async {
            while !done() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), wait).await.unwrap_or_else(|_| panic!("timed out waiting until {what}"));
    }

    /// `claimant`'s tick with a `hold` claim, run on its own task.
    fn spawn_tick(
        claimant: &'static str,
        hold: chrono::Duration,
        db: &Arc<dyn DatabaseStore>,
        registry: &Arc<ProviderRegistry>,
        store: &Arc<dyn ImageStorage>,
    ) -> tokio::task::JoinHandle<()> {
        let (db, registry, store) = (db.clone(), registry.clone(), store.clone());
        tokio::spawn(async move {
            poll_once_with_hold(claimant, hold, &db, &registry, None, crate::config::Mode::SingleTenant, &store, None).await
        })
    }

    /// A tick still polling a row past the claim's hold keeps it (R7): another
    /// replica's tick must not poll the same vendor job meanwhile. Wall-clock
    /// times: the claim's clock is the database's.
    #[tokio::test]
    async fn a_row_a_replica_is_still_polling_is_not_polled_by_another() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        db.insert_generation("g-slow", None, "mock/mesh-3d", "mock", "model3d", Some("slow-job"), 0.0, None, None).await.unwrap();
        let provider = Arc::new(GatedCountingModel3dProvider::new());
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_model3d(provider.clone()).await;
        let store = test_3d_store();
        let hold = chrono::Duration::seconds(3);

        let a = spawn_tick("a", hold, &db, &registry, &store);
        until("a polls the slow job", || provider.total() == 1).await;
        // Past a's 3 s hold, with a still polling the job.
        tokio::time::sleep(std::time::Duration::from_millis(4500)).await;
        let b = spawn_tick("b", hold, &db, &registry, &store);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let polls = provider.polls_per_job().get("slow-job").copied().unwrap_or(0);
        provider.open();
        a.await.unwrap();
        b.await.unwrap();
        assert_eq!(polls, 1, "b polled the job a was still polling");
        let row = db.get_generation("g-slow").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed, "and a finished it");
    }

    /// A row that waited for a slot past the hold, and that another replica
    /// took meanwhile, is skipped when the slot frees (R7): each vendor job is
    /// polled once. The rows already being polled stay this replica's.
    #[tokio::test]
    async fn a_row_another_replica_took_while_it_waited_is_skipped() {
        let sqlite = in_memory_db().await;
        for i in 0..=POLL_CONCURRENCY {
            sqlite
                .insert_generation(&format!("g-slow-{i}"), None, "mock/mesh-3d", "mock", "model3d", Some(&format!("slow-{i}")), 0.0, None, None)
                .await
                .unwrap();
        }
        let db: Arc<dyn DatabaseStore> = sqlite.clone();
        let provider = Arc::new(GatedCountingModel3dProvider::new());
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_model3d(provider.clone()).await;
        let store = test_3d_store();
        let hold = chrono::Duration::seconds(3);

        // a claims every row; all its slots fill and one row waits.
        let a = spawn_tick("a", hold, &db, &registry, &store);
        until("a fills its slots", || provider.total() == POLL_CONCURRENCY).await;
        tokio::time::sleep(std::time::Duration::from_millis(4500)).await;
        let b = spawn_tick("b", hold, &db, &registry, &store);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let held_by_b: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM generations WHERE poll_claimed_by = 'b'")
            .fetch_one(sqlite.pool())
            .await
            .unwrap();
        provider.open();
        a.await.unwrap();
        b.await.unwrap();
        let polls = provider.polls_per_job();
        assert_eq!(held_by_b, 1, "b took only the row a had not started: {polls:?}");
        assert_eq!(polls.len(), POLL_CONCURRENCY + 1, "every job was polled: {polls:?}");
        assert!(polls.values().all(|n| *n == 1), "and none twice: {polls:?}");
    }

    // ─── the router lets go of the jobs the poller ends (P3) ──────────────

    fn a_router(registry: &Arc<ProviderRegistry>) -> Arc<crate::proxy::router::ProxyRouter> {
        Arc::new(crate::proxy::router::ProxyRouter::new(
            registry.clone(),
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(crate::config::AppConfig::default()),
            Arc::new(crate::proxy::storage::LocalStore),
        ))
    }

    fn presenting_of(router: &Arc<crate::proxy::router::ProxyRouter>, db: &Arc<dyn DatabaseStore>) -> crate::access::present::Presenting {
        crate::access::present::Presenting { router: router.clone(), db: db.clone(), secrets_key: None, mode: crate::config::Mode::SingleTenant }
    }

    /// `mock/video-gen` submitted through `router`, with its row written as
    /// the submit handler writes it. Returns the generation id.
    async fn submit_video_through(router: &crate::proxy::router::ProxyRouter, db: &Arc<dyn DatabaseStore>) -> String {
        let schema = {
            let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            p.pop();
            p.push("models");
            crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load shipped models").get("mock/video-gen").expect("mock/video-gen ships").clone()
        };
        let base = BaseGenerationRequest {
            prompt: "a video".into(), model: "mock/video-gen".into(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        };
        let extras = VideoExtras {
            duration_seconds: 5.0,
            aspect_ratio: None,
            resolution: None,
            fps: None,
            extra: None,
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
        };
        let submitted = router.generate_video(&schema, &base, &extras, &Default::default(), Default::default()).await.expect("submitted");
        let job = router.get_provider_job_id(&submitted.id).await.expect("the router tracks the job");
        db.insert_generation(&submitted.id, None, "mock/video-gen", "mock", "video", Some(&job), 0.0, None, None).await.unwrap();
        submitted.id
    }

    /// The replica whose router took a video lets it go when its poller
    /// finishes it, instead of keeping it, provider instance and all, until
    /// the job's age passes the reap age or the process restarts.
    #[tokio::test]
    async fn the_poller_lets_the_router_go_of_a_video_it_finished() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_registry().await;
        let router = a_router(&registry);
        let id = submit_video_through(&router, &db).await;

        poll_once_presenting(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store(), Some(&presenting_of(&router, &db))).await;

        let row = db.get_generation(&id).await.unwrap().unwrap();
        assert_eq!(row.status, GenerationStatus::Completed, "{:?}", row.error_message);
        assert_eq!(router.get_provider_job_id(&id).await, None, "the router let go of the video the poller finished");
    }

    /// The same for a video the poller fails by a reap (a vendor that no
    /// longer knows the job).
    #[tokio::test]
    async fn the_poller_lets_the_router_go_of_a_video_it_reaped() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let router = a_router(&make_registry().await);
        let id = submit_video_through(&router, &db).await;

        let failing = registry_with(PollBehavior::TerminalError).await;
        poll_once_presenting(&db, &failing, None, crate::config::Mode::SingleTenant, &test_3d_store(), Some(&presenting_of(&router, &db))).await;

        let row = db.get_generation(&id).await.unwrap().unwrap();
        assert_eq!(row.status, GenerationStatus::Failed, "the poller reaped it");
        assert_eq!(router.get_provider_job_id(&id).await, None, "the router let go of the video the poller reaped");
    }

    /// And for a 3D job: its map was bounded by age only.
    #[tokio::test]
    async fn the_poller_lets_the_router_go_of_a_3d_job_it_finished() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = Arc::new(ProviderRegistry::new());
        registry.register_mock_model3d(Arc::new(DoneModel3dProvider)).await;
        let router = a_router(&registry);
        let model = "mock/mesh-3d";
        let submitted = router
            .generate_model3d(&mock_3d_schema(model), &mock_3d_base(model, "a mesh"), &mock_3d_extras(), &Default::default(), None)
            .await
            .expect("submitted");
        let job = router.get_model3d_provider_job_id(&submitted.id).await.expect("the router tracks the job");
        db.insert_generation(&submitted.id, None, model, "mock", "model3d", Some(&job), 0.0, None, None).await.unwrap();

        poll_once_presenting(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store(), Some(&presenting_of(&router, &db))).await;

        let row = db.get_generation(&submitted.id).await.unwrap().unwrap();
        assert_eq!(row.status, GenerationStatus::Completed, "{:?}", row.error_message);
        assert_eq!(router.get_model3d_provider_job_id(&submitted.id).await, None, "the router let go of the 3d job the poller finished");
    }

    // ─── terminal webhooks go through the process's dispatcher (P13) ──────

    /// A terminal webhook is a delivery a stop waits for: draining the
    /// process's deliveries returns once it has been answered and recorded.
    /// Timing-sensitive: the drain also waits for every other test's
    /// deliveries in this process (it is on the load list; run it alone).
    #[tokio::test]
    async fn a_terminal_webhook_goes_through_the_process_dispatcher() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_millis(500)))
            .mount(&server)
            .await;
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let hash = format!("drain-hash-{}", uuid::Uuid::new_v4().simple());
        let key = db.create_api_key("drain-key", &hash, "lg-drain", None, None, "generate,read", Some(&format!("{}/wh", server.uri()))).await.unwrap();
        let id = "litegen-vid-wh-drain";
        db.insert_generation(id, Some(&key.id), "mock/video-gen", "mock", "video", Some("job"), 0.0, None, None).await.unwrap();
        db.update_generation_status(id, "completed", 100, None, None, Some(chrono::Utc::now())).await.unwrap();
        let row = db.get_generation(id).await.unwrap().unwrap();

        dispatch_terminal_webhook(&db, &crate::util::ssrf::no_redirect_client(), crate::config::Mode::SingleTenant, row, None).await;
        let left = crate::proxy::webhook::deliveries()
            .drain(tokio::time::Instant::now() + std::time::Duration::from_secs(10))
            .await;

        let (sent, total) = db.list_webhook_deliveries(&key.id.to_string(), 1, 50).await.unwrap();
        assert_eq!(total, 1, "the drain returned before the webhook was answered (still running: {left:?})");
        assert!(sent[0].success, "{:?}", sent[0]);
    }

    // ─── ages and stage claims on the database's clock (R8) ───────────────
    // `with_clock_offset` sets "the database's clock" apart from this
    // process's, which is how a replica whose own clock is off looks.

    /// Another observer's stage claim, made just now on the database's clock,
    /// is waited for by a replica whose clock runs 11 min ahead, not failed
    /// as interrupted (10 min) while the vendor bills the stage.
    #[tokio::test]
    async fn a_stage_claim_is_judged_on_the_databases_clock() {
        let skewed = SqliteDatabase::connect("sqlite::memory:").await.unwrap().with_clock_offset(-chrono::Duration::minutes(11));
        let db: Arc<dyn DatabaseStore> = Arc::new(skewed);
        let provider = Arc::new(staged_provider());
        let (registry, _router, id) = staged_job(&db, provider.clone()).await;
        let just_now_on_the_database = (chrono::Utc::now() - chrono::Duration::minutes(11)).to_rfc3339();
        db.merge_generation_metadata_if_active(&id, &serde_json::json!({
            "stage_claims": { "two": { "by": "elsewhere", "at": just_now_on_the_database } },
        })).await.unwrap();

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;
        let row = db.get_generation(&id).await.unwrap().unwrap();
        assert_eq!(row.status, GenerationStatus::Processing, "a fresh claim is waited for: {:?}", row.error_message);
        assert_eq!(provider.starts.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// The reaper ages a row on the database's clock: a replica whose clock
    /// runs 3 h ahead must not reap (fail and refund) a row the database
    /// made just now.
    #[tokio::test]
    async fn a_row_is_aged_on_the_databases_clock() {
        let skewed = SqliteDatabase::connect("sqlite::memory:").await.unwrap().with_clock_offset(-chrono::Duration::hours(3));
        skewed.insert_generation("g-fresh", None, "mock/video-gen", "mock", "video", Some("job"), 0.0, None, None).await.unwrap();
        // Made just now on the database's clock.
        sqlx::query("UPDATE generations SET created_at = datetime('now', '-3 hours') WHERE id = 'g-fresh'")
            .execute(skewed.pool())
            .await
            .unwrap();
        let db: Arc<dyn DatabaseStore> = Arc::new(skewed);
        let registry = registry_with(PollBehavior::StuckProcessing).await;

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &test_3d_store()).await;
        let row = db.get_generation("g-fresh").await.unwrap().unwrap();
        assert_eq!(row.status, GenerationStatus::Processing, "a row made just now is not past the maximum age: {:?}", row.error_message);
    }
}
