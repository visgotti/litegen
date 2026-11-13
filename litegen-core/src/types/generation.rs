//! What a stored generation row answers: its 3D response (with the mesh and
//! requested-formats guards every observer of completion applies), its cost
//! as usage, and its stored files. Pure reads of a `Generation`; the
//! handlers, the router, the poller and 3D delivery all use them (moved from
//! api/handlers and proxy/router.rs, docs/superpowers/specs/
//! 2026-11-07-engine-boundary-design.md D4).

use super::*;

/// The containers a persisted generation promised its caller.
///
/// Falls back to GLB when the row predates the field or its metadata write was
/// lost. That floor is deliberate: GLB is required of every 3D model, so the
/// guard still means something on an old row instead of silently becoming a
/// no-op — it just cannot know about any additional container that was asked
/// for.
pub(crate) fn row_requested_formats(gen: &crate::types::Generation) -> Vec<String> {
    gen.metadata.as_ref()
        .and_then(|m| m.get("requested_formats"))
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![crate::types::DEFAULT_MODEL3D_FORMAT.to_string()])
}

/// USD per internal token: the one rate. The router's usage and quotes,
/// `providers::build_cost_estimate` and a generation answered from its row all
/// convert at it, so the same generation always quotes one token count.
pub(crate) const USD_PER_TOKEN: f64 = 0.001;

/// The `usage` triple for a generation answered out of the database.
///
/// `generations.cost_usd` is written at submit time from the very `UsageInfo`
/// the submit response carried, so re-deriving the triple from it reproduces
/// that response exactly. `Estimated` because that is what the router quoted:
/// nothing in the async path ever replaces it with a provider-reported cost.
pub(crate) fn usage_from_row_cost(cost_usd: f64) -> UsageInfo {
    UsageInfo {
        cost_usd,
        tokens: crate::providers::usd_to_tokens(cost_usd, USD_PER_TOKEN),
        cost_source: CostSource::Estimated,
    }
}

/// Rebuild a `Model3dGenerationResponse` from a persisted row — the answer once
/// the poller has terminalised a job and the router no longer tracks it, and
/// the third observer of completion, so it applies the same mesh and
/// requested-formats guards the other two do.
pub(crate) fn model3d_response_from_row(gen: &crate::types::Generation) -> Model3dGenerationResponse {
    let assets = gen.metadata.as_ref()
        .and_then(|m| m.get("assets"))
        .and_then(|a| serde_json::from_value::<Vec<Model3dAsset>>(a.clone()).ok())
        .unwrap_or_default();
    // A `completed` row with no mesh is not a usable generation — it is the
    // failure `get_3d_status` and the poller both refuse to report as success,
    // and a row can only reach this state if a terminal write was lost. Report
    // it the same way here, or the one path that answers a generation AFTER it
    // has left the poller's window would be the one path that reports a paid
    // generation the caller cannot use as `completed` (Ruling R21).
    let has_mesh = assets.iter().any(|a| a.kind == Model3dAssetKind::Mesh);
    let (status, error, assets) = if gen.status == GenerationStatus::Completed && !has_mesh {
        (
            GenerationStatus::Failed,
            Some(gen.error_message.clone().unwrap_or_else(
                || "provider reported success without a mesh asset".to_string(),
            )),
            Vec::new(),
        )
    } else {
        (gen.status, gen.error_message.clone(), assets)
    };
    let mut resp = Model3dGenerationResponse {
        id: gen.id.clone(),
        status,
        model: gen.model.clone(),
        provider: gen.provider.clone(),
        assets,
        progress: gen.progress.clamp(0, 100) as u8,
        error,
        // The consumer contract reads cost off this response, so a row-answered
        // poll carries the same triple a router-answered one does.
        usage: Some(usage_from_row_cost(gen.cost_usd)),
        created: gen.created_at.timestamp(),
        metadata: gen.request_metadata(),
    };
    // Third and last observer of completion. The mesh guard above answers "is
    // there a mesh at all"; this answers "is every mesh the caller was promised
    // there" — the row-answered path must reach the same verdict the poller and
    // `get_3d_status` did, or which observer won the race would decide whether
    // the generation succeeded.
    enforce_requested_formats(&mut resp, &row_requested_formats(gen));
    resp
}

/// Every stored file of a generation, in order. A 3D row goes through the
/// same guards its `GET /v1/models3d/{id}` answer does, so a generation that
/// failed (or never delivered a promised container) lists nothing; any other
/// row lists what it recorded once `completed`.
pub(crate) fn generation_assets(gen: &crate::types::Generation) -> Vec<crate::types::GenerationAsset> {
    use crate::types::GenerationAsset;
    if gen.media_type == "model3d" {
        return model3d_response_from_row(gen)
            .assets
            .into_iter()
            .enumerate()
            .map(|(i, a)| GenerationAsset {
                expires_at: None,
                index: i as u32,
                kind: serde_json::to_value(a.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
                format: a.format,
                url: a.url,
                size_bytes: a.size_bytes,
                sha256: a.sha256,
                width: a.width,
                height: a.height,
                duration_s: None,
                fps: None,
                has_audio: None,
                kept: true,
            })
            .collect();
    }
    if gen.status != GenerationStatus::Completed {
        return Vec::new();
    }
    let recorded: Vec<GenerationAsset> = gen.metadata.as_ref()
        .and_then(|m| m.get("assets"))
        .and_then(|a| serde_json::from_value(a.clone()).ok())
        .unwrap_or_default();
    if recorded.is_empty() {
        // A row written before litegen kept results: its one result URL.
        return gen.result_url.iter().map(|url| GenerationAsset {
            expires_at: None,
            index: 0,
            kind: gen.media_type.clone(),
            format: url.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default(),
            url: url.clone(),
            size_bytes: None,
            sha256: None,
            width: None,
            height: None,
            duration_s: None,
            fps: None,
            has_audio: None,
            kept: true,
        }).collect();
    }
    recorded.into_iter().enumerate().map(|(i, a)| GenerationAsset { index: i as u32, ..a }).collect()
}

/// Demote a `completed` 3D response that is missing a container the caller was
/// promised.
///
/// A partial delivery is a failure, not a degraded success: the caller asked for
/// `["glb", "fbx"]` because something downstream needs the fbx, and reporting
/// `completed` would bill them for a result they cannot use. Assets are cleared
/// for the same reason the mesh guard clears them — a failed generation must not
/// look half-usable.
///
/// This runs at EVERY observer of completion. There are three
/// (the router's `get_model3d_status`, the poller, and `model3d_response_from_row`), and
/// a check in fewer than all of them means the observer that wins the race
/// decides whether the generation succeeded.
pub fn enforce_requested_formats(resp: &mut Model3dGenerationResponse, requested: &[String]) {
    if resp.status != GenerationStatus::Completed {
        return;
    }
    let delivered = resp.delivered_formats();
    // No mesh at all is the mesh guard's diagnosis, not this one. Both are true
    // of an empty result, but "reported success without a mesh asset" says what
    // actually happened; "did not return glb" reads as a format quibble about a
    // generation that produced nothing. Leaving it to the caller-side guard also
    // keeps that error string — which consumers match on — stable.
    if delivered.is_empty() {
        return;
    }
    let missing = crate::types::missing_model3d_formats(requested, &delivered);
    if missing.is_empty() {
        return;
    }
    resp.status = GenerationStatus::Failed;
    resp.error = Some(format!(
        "provider did not return the requested format(s): {}",
        missing.join(", ")
    ));
    resp.assets.clear();
}
