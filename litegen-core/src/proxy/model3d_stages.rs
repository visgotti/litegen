//! Stages after submit: the bookkeeping both observers of a 3D job apply to a
//! poll result before reporting it.
//!
//! Some vendor flows bill more than one call: Tripo's free `rig-check` before
//! its billed `rig`, an upload before its task, Meshy's preview before refine.
//! The adapter's `poll_status` reports such a boundary as `next_stage`; the
//! observer that wins the stage's claim on the row (`claim_generation_stage`)
//! starts it, and persists the stage context the vendor hands back. The
//! client's `GET /v1/models3d/{id}` and the background poller both run this,
//! so whichever sees the boundary first starts the stage — exactly once.
//!
//! A claim whose start was never recorded — the claimant died between
//! claiming and persisting — fails the generation once it is older than
//! [`STAGE_START_GRACE`]. litegen never re-sends a billed call it cannot prove
//! did not land; a failed generation is refunded, a second job is billed.

use std::sync::Arc;

use crate::db::DatabaseStore;
use crate::providers::{Model3dGenerationHandle, Model3dGenerationPollResult, Model3dProvider};
use crate::types::GenerationStatus;

/// How long a stage may stay claimed without its start being recorded before
/// the generation is failed as interrupted. Far longer than any vendor's
/// submit call, which is all that runs between the claim and the record.
pub const STAGE_START_GRACE: chrono::Duration = chrono::Duration::minutes(10);

/// Apply `poll`'s stage bookkeeping for generation `gen_id`, as `claimant`.
///
/// Persists a changed stage context, and starts a requested stage when this
/// observer wins its claim. On return `poll` is what to report: `processing`
/// while a stage is being started or waited on, `failed` when starting it
/// failed or a claim was interrupted. `handle` carries any new stage context.
pub async fn advance(
    db: &Arc<dyn DatabaseStore>,
    provider: &Arc<dyn Model3dProvider>,
    gen_id: &str,
    handle: &mut Model3dGenerationHandle,
    poll: &mut Model3dGenerationPollResult,
    claimant: &str,
) {
    if let Some(ctx) = poll.stage_context.take() {
        persist_stage_context(db, gen_id, &ctx).await;
        handle.stage_context = Some(ctx);
    }
    let Some(stage) = poll.next_stage.take() else { return };
    if poll.status.is_terminal() {
        tracing::warn!(generation_id = %gen_id, stage = %stage, "a terminal poll asked for a next stage; ignored");
        return;
    }
    // Whatever happens below, the job is not done while a stage is pending.
    poll.status = GenerationStatus::Processing;
    poll.files.clear();

    // The claim, the billed start and its record run as one task of their
    // own: an observer dropped mid-way — a client's GET that hung up — must
    // not leave the stage claimed and started at the vendor but never
    // recorded, which fails the generation as interrupted after the vendor
    // billed the stage. Awaited, so this observer still reports it.
    let claimed = {
        let (db, provider, gen_id, stage, claimant, handle) =
            (db.clone(), provider.clone(), gen_id.to_string(), stage.clone(), claimant.to_string(), handle.clone());
        tokio::spawn(async move {
            match db.claim_generation_stage(&gen_id, &stage, &claimant).await {
                Ok(true) => Ok(Some(match provider.start_stage(&handle, &stage).await {
                    Ok(mut ctx) => {
                        // The convention every observer reads back: the
                        // context names the stage it belongs to.
                        if ctx.is_object() {
                            ctx["stage"] = serde_json::Value::String(stage.clone());
                        }
                        persist_stage_context(&db, &gen_id, &ctx).await;
                        Ok(ctx)
                    }
                    Err(e) => Err(e.to_string()),
                })),
                Ok(false) => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        })
        .await
        .unwrap_or_else(|e| Err(format!("the stage task failed: {e}")))
    };
    match claimed {
        Ok(Some(Ok(ctx))) => handle.stage_context = Some(ctx),
        Ok(Some(Err(e))) => {
            poll.status = GenerationStatus::Failed;
            poll.error = Some(format!("could not start the {stage} stage: {e}"));
        }
        Ok(None) => match held_claim(db, gen_id, &stage).await {
            // The other observer started it: poll the stage it recorded.
            HeldClaim::Started(ctx) => handle.stage_context = Some(ctx),
            HeldClaim::Interrupted(at) => {
                poll.status = GenerationStatus::Failed;
                poll.error = Some(format!(
                    "the {stage} stage was claimed at {at} but its start was never recorded (interrupted); \
                     litegen does not re-send a billed call it cannot prove did not land"
                ));
            }
            HeldClaim::Pending => {}
        },
        Err(e) => {
            // Nothing was claimed; the next poll tries again.
            tracing::warn!(generation_id = %gen_id, stage = %stage, error = %e, "could not claim a 3d stage");
        }
    }
}

async fn persist_stage_context(db: &Arc<dyn DatabaseStore>, gen_id: &str, ctx: &serde_json::Value) {
    if let Err(e) = db
        .merge_generation_metadata_if_active(gen_id, &serde_json::json!({ "stage_context": ctx }))
        .await
    {
        tracing::warn!(generation_id = %gen_id, error = %e, "failed to persist a 3d stage context");
    }
}

/// What another observer's claim on a stage amounts to.
enum HeldClaim {
    /// The stage was started; this is the context it recorded.
    Started(serde_json::Value),
    /// Claimed longer than [`STAGE_START_GRACE`] ago and never recorded as
    /// started: the claimant died in between. Carries the claim time.
    Interrupted(String),
    /// Claimed recently, or not readable now: wait for the next poll.
    Pending,
}

async fn held_claim(db: &Arc<dyn DatabaseStore>, gen_id: &str, stage: &str) -> HeldClaim {
    let Ok(Some(row)) = db.get_generation(gen_id).await else { return HeldClaim::Pending };
    let Some(meta) = row.metadata else { return HeldClaim::Pending };
    if let Some(ctx) = meta.get("stage_context") {
        if ctx.get("stage").and_then(|s| s.as_str()) == Some(stage) {
            return HeldClaim::Started(ctx.clone());
        }
    }
    let at = meta
        .get("stage_claims")
        .and_then(|c| c.get(stage))
        .and_then(|c| c.get("at"))
        .and_then(|a| a.as_str());
    match at.and_then(|a| chrono::DateTime::parse_from_rfc3339(a).ok().map(|t| (a, t))) {
        Some((a, t)) if chrono::Utc::now() - t.with_timezone(&chrono::Utc) > STAGE_START_GRACE => {
            HeldClaim::Interrupted(a.to_string())
        }
        _ => HeldClaim::Pending,
    }
}
