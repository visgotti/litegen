pub mod auth_password;
pub mod oauth;
pub mod users;
pub mod account;
pub mod orgs;

pub use auth_password::{
    auth_config, csrf_token, login, logout, me, password_reset_confirm, password_reset_request,
    signup,
};
pub use oauth::{github_callback, github_start, google_callback, google_start, oauth_redirect};
pub use users::{
    accept_invitation, delete_user, get_invitation, invite_user, list_users, patch_user,
    transfer_owner,
};
pub use account::{
    get_account, list_sessions, patch_account, revoke_session,
};

use axum::{
    extract::{Extension, Json, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::error;
use uuid::Uuid;

// CSV helpers
fn logs_to_csv(logs: &[RequestLog]) -> String {
    let mut wtr = csv::Writer::from_writer(vec![]);
    let _ = wtr.write_record(["id","model","provider","status","media_type","cost_usd","latency_ms","created_at","error"]);
    for l in logs {
        let _ = wtr.write_record([
            &l.id,
            &l.model,
            &l.provider,
            &format!("{}", l.status),
            &format!("{:?}", l.media_type).to_lowercase(),
            &l.cost_usd.to_string(),
            &l.latency_ms.to_string(),
            &l.created_at.to_rfc3339(),
            l.error.as_deref().unwrap_or(""),
        ]);
    }
    String::from_utf8(wtr.into_inner().unwrap_or_default()).unwrap_or_default()
}

fn audit_to_csv(entries: &[AuditLogEntry]) -> String {
    let mut wtr = csv::Writer::from_writer(vec![]);
    let _ = wtr.write_record(["id","actor_key_id","actor_label","action","target_type","target_id","before_json","after_json","created_at"]);
    for e in entries {
        let _ = wtr.write_record([
            &e.id,
            e.actor_key_id.as_deref().unwrap_or(""),
            &e.actor_label,
            &e.action,
            &e.target_type,
            &e.target_id,
            e.before_json.as_deref().unwrap_or(""),
            e.after_json.as_deref().unwrap_or(""),
            &e.created_at.to_rfc3339(),
        ]);
    }
    String::from_utf8(wtr.into_inner().unwrap_or_default()).unwrap_or_default()
}

fn csv_response(body: String, filename: &str) -> axum::response::Response {
    use axum::http::header;
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", filename)),
        ],
        body,
    ).into_response()
}

use crate::types::UpdateApiKeyRequest;

use crate::providers::{ImageExtras, Model3dExtras, VideoExtras};
use crate::types::*;

use super::middleware::{AppState, KeyContext};
use super::middleware::validator::{ValidatedImage, ValidatedModel3d, ValidatedVideo, dropped_header};

// ─── Key context extractor ───────────────────────────────────────────────────

/// Extracts KeyContext from request extensions. None if auth is not configured.
pub struct OptionalKeyContext(pub Option<KeyContext>);

impl<S> axum::extract::FromRequestParts<S> for OptionalKeyContext
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let ctx = parts.extensions.get::<KeyContext>().cloned();
        async move { Ok(OptionalKeyContext(ctx)) }
    }
}

// ─── Image Generation ───────────────────────────────────────────────────────

/// POST /v1/images/generations — Generate images (OpenAI-compatible).
#[utoipa::path(
    post,
    path = "/v1/images/generations",
    request_body = ImageGenerationRequest,
    responses(
        (status = 200, description = "Image generated successfully", body = ImageGenerationResponse),
        (status = 400, description = "Bad request", body = ErrorResponse),
        (status = 502, description = "Provider error", body = ErrorResponse),
    ),
    tag = "Images"
)]
pub async fn generate_image(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    validated: ValidatedImage,
) -> impl IntoResponse {
    // The in-flight slot was acquired in the Validated* extractor (before the
    // body was buffered) and is held for this request via `validated._permit`.

    let start = std::time::Instant::now();

    let materialized = match state.materializer.materialize(
        &validated.schema,
        validated.request.base.reference_images.clone(),
        &validated.ctx,
    ).await {
        Ok(m) => m,
        Err(e) => return validation_rejection_response(&e.to_string(), 400, &validated.schema.id),
    };

    let extras = ImageExtras {
        size: validated.request.size.clone(),
        aspect_ratio: validated.request.aspect_ratio.clone(),
        quality: validated.request.quality.clone(),
        style: validated.request.style.clone(),
        steps: validated.request.steps,
        guidance_scale: validated.request.guidance_scale,
        strength: validated.request.strength,
        response_format: validated.request.response_format.clone(),
        extra: validated.request.base.extra.clone(),
    };

    // Resolve the calling app's BYO credential for this model's provider. A
    // server-side failure (bad secrets key / corrupt cred) early-returns a 500.
    let app_creds = match resolve_org_provider_credential(&state, &key_ctx, &validated.schema.provider).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    // No per-app credential AND no registered platform default → the provider is
    // unusable for this app. (When `app_creds` is Some, BYO works even with no
    // global instance, so we skip the check.)
    if app_creds.is_none() && !state.router.has_image_provider(&validated.schema.provider).await {
        return provider_not_configured_response(&validated.schema.provider);
    }

    // Resolve the calling app's BYO image store (None → global store fallback).
    let app_store = resolve_app_image_store(&state, &key_ctx).await;

    // Tenant scope for the (process-global) cache. Composed so distinct
    // principals can NEVER share a bucket: org+app keeps an app-scoped request
    // separate from a sibling app and from the org scope, and when no tenant is
    // resolved we fall back to the calling principal (key/user/master) instead
    // of a shared global bucket. See `cache_scope`.
    let cache_scope = key_ctx.as_ref().map(cache_scope);

    // Reserve the estimated cost against the key's quota BEFORE dispatch (the
    // real spend cap; see reserve_quota). Settled to the actual cost on success
    // and released on failure so a failed request is never billed.
    let charge_key = key_ctx.as_ref().and_then(|c| c.key_id);
    let reserved = match reserve_quota(&state, charge_key, validated.schema.pricing.base_cost_usd).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    match state.router.generate_image(&validated.schema, &validated.request.base, &extras, &materialized, app_creds, app_store, cache_scope.as_deref()).await {
        Ok(response) => {
            let latency = start.elapsed().as_millis() as i64;
            let cost = response.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);

            // Settle the reservation to the actual cost.
            settle_quota(&state, charge_key, reserved, cost).await;

            // Build artifact for drill-down storage
            let artifact = {
                let req_id = response.id.clone();
                let prompt = validated.request.base.prompt.clone();
                let neg = validated.request.base.negative_prompt.clone();
                let params = serde_json::to_value(&extras).ok();
                let refs_meta = build_refs_meta(&validated.request.base.reference_images);
                let first = response.data.first();
                let (output_kind, output_value, output_mime, output_truncated) = match first {
                    Some(img) => {
                        if let Some(ref b64) = img.b64_json {
                            const CAP: usize = 2 * 1024 * 1024;
                            let truncated = b64.len() > CAP;
                            let value = if truncated { b64[..CAP].to_string() } else { b64.clone() };
                            ("b64".to_string(), Some(value), Some(img.content_type.clone()), truncated)
                        } else if let Some(ref url) = img.url {
                            ("url".to_string(), Some(url.clone()), Some(img.content_type.clone()), false)
                        } else {
                            ("error".to_string(), None, None, false)
                        }
                    }
                    None => ("error".to_string(), None, None, false),
                };
                RequestArtifact {
                    request_id: req_id,
                    media_type: "image".to_string(),
                    prompt: Some(prompt),
                    negative_prompt: neg,
                    params_json: params,
                    refs_meta_json: refs_meta,
                    output_kind,
                    output_value,
                    output_mime,
                    output_truncated,
                    error_message: None,
                    created_at: chrono::Utc::now(),
                    org_id: key_ctx.as_ref().and_then(|c| c.org_id.clone()),
                    app_id: key_ctx.as_ref().and_then(|c| c.app_id.clone()),
                }
            };

            // Log request + artifact async
            let db = state.db.clone();
            let id = response.id.clone();
            let model = response.model.clone();
            let provider = response.provider.clone();
            let org_id = key_ctx.as_ref().and_then(|c| c.org_id.clone());
            let app_id = key_ctx.as_ref().and_then(|c| c.app_id.clone());
            tokio::spawn(async move {
                let _ = db
                    .log_request(&id, &model, &provider, "completed", "image", cost, latency, None, None, org_id.as_deref(), app_id.as_deref())
                    .await;
                if let Err(e) = db.insert_request_artifact(&artifact).await {
                    tracing::warn!(error = %e, request_id = %artifact.request_id, "Failed to store request artifact");
                }
            });
            let mut resp = (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
            if let Some((k, v)) = dropped_header(&validated.dropped) {
                resp.headers_mut().insert(k, v);
            }
            resp
        }
        Err(e) => {
            // Nothing was delivered — release the reservation so a failed
            // request is never billed.
            settle_quota(&state, charge_key, reserved, 0.0).await;
            let latency = start.elapsed().as_millis() as i64;
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            error!(error = %e, "Image generation failed");
            // Log failure + error artifact
            let db = state.db.clone();
            let model = validated.schema.id.clone();
            let err_msg = e.to_string();
            let prompt = validated.request.base.prompt.clone();
            let org_id = key_ctx.as_ref().and_then(|c| c.org_id.clone());
            let app_id = key_ctx.as_ref().and_then(|c| c.app_id.clone());
            tokio::spawn(async move {
                let id = format!("litegen-img-{}", Uuid::new_v4());
                let _ = db
                    .log_request(&id, &model, "unknown", "failed", "image", 0.0, latency, Some(&err_msg), None, org_id.as_deref(), app_id.as_deref())
                    .await;
                let artifact = RequestArtifact {
                    request_id: id,
                    media_type: "image".to_string(),
                    prompt: Some(prompt),
                    negative_prompt: None,
                    params_json: None,
                    refs_meta_json: None,
                    output_kind: "error".to_string(),
                    output_value: None,
                    output_mime: None,
                    output_truncated: false,
                    error_message: Some(err_msg),
                    created_at: chrono::Utc::now(),
                    org_id: org_id.clone(),
                    app_id: app_id.clone(),
                };
                if let Err(e) = db.insert_request_artifact(&artifact).await {
                    tracing::warn!(error = %e, "Failed to store error artifact");
                }
            });
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

/// POST /v1/images/cost — Estimate cost for image generation.
#[utoipa::path(
    post,
    path = "/v1/images/cost",
    request_body = ImageGenerationRequest,
    responses(
        (status = 200, description = "Cost estimate", body = CostEstimate),
        (status = 400, description = "Bad request (validation failed)", body = ErrorResponse),
    ),
    tag = "Images"
)]
pub async fn estimate_image_cost(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    validated: ValidatedImage,
) -> impl IntoResponse {
    let app_creds = match resolve_org_provider_credential(&state, &key_ctx, &validated.schema.provider).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if app_creds.is_none() && !state.router.has_image_provider(&validated.schema.provider).await {
        return provider_not_configured_response(&validated.schema.provider);
    }
    match state.router.estimate_image_cost(&validated.schema, &validated.request, app_creds).await {
        Ok(estimate) => (StatusCode::OK, Json(serde_json::to_value(estimate).unwrap())).into_response(),
        Err(e) => {
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

// ─── Video Generation ───────────────────────────────────────────────────────

/// POST /v1/videos/generations — Start video generation.
#[utoipa::path(
    post,
    path = "/v1/videos/generations",
    request_body = VideoGenerationRequest,
    responses(
        (status = 200, description = "Video generation started", body = VideoGenerationResponse),
        (status = 400, description = "Bad request", body = ErrorResponse),
    ),
    tag = "Videos"
)]
pub async fn generate_video(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    validated: ValidatedVideo,
) -> impl IntoResponse {
    // The in-flight slot was acquired in the Validated* extractor (before the
    // body was buffered) and is held for this request via `validated._permit`.

    let start = std::time::Instant::now();

    let materialized = match state.materializer.materialize(
        &validated.schema,
        validated.request.base.reference_images.clone(),
        &validated.ctx,
    ).await {
        Ok(m) => m,
        Err(e) => return validation_rejection_response(&e.to_string(), 400, &validated.schema.id),
    };

    let extras = VideoExtras {
        duration_seconds: validated.request.duration_seconds,
        aspect_ratio: validated.request.aspect_ratio.clone(),
        resolution: validated.request.resolution.clone(),
        fps: validated.request.fps,
        extra: validated.request.base.extra.clone(),
    };

    // Resolve the calling app's BYO credential for this model's provider (see
    // `generate_image`). Server-side failures early-return a 500.
    let app_creds = match resolve_org_provider_credential(&state, &key_ctx, &validated.schema.provider).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if app_creds.is_none() && !state.router.has_video_provider(&validated.schema.provider).await {
        return provider_not_configured_response(&validated.schema.provider);
    }

    // Reserve the estimated cost against the key's quota BEFORE dispatch, then
    // settle to the actual cost on success / release on failure.
    let charge_key = key_ctx.as_ref().and_then(|c| c.key_id);
    let reserved = match reserve_quota(&state, charge_key, validated.schema.pricing.base_cost_usd).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    match state.router.generate_video(&validated.schema, &validated.request.base, &extras, &materialized, app_creds).await {
        Ok(response) => {
            let latency = start.elapsed().as_millis() as i64;
            let cost = response.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);

            // Settle the reservation to the actual cost.
            settle_quota(&state, charge_key, reserved, cost).await;

            // Build video artifact
            let video_artifact = {
                let req_id = response.id.clone();
                let prompt = validated.request.base.prompt.clone();
                let neg = validated.request.base.negative_prompt.clone();
                let params = serde_json::to_value(&extras).ok();
                let refs_meta = build_refs_meta(&validated.request.base.reference_images);
                let (output_kind, output_value) = if let Some(ref url) = response.video_url {
                    ("url".to_string(), Some(url.clone()))
                } else {
                    ("url".to_string(), None) // async — URL not yet known
                };
                RequestArtifact {
                    request_id: req_id,
                    media_type: "video".to_string(),
                    prompt: Some(prompt),
                    negative_prompt: neg,
                    params_json: params,
                    refs_meta_json: refs_meta,
                    output_kind,
                    output_value,
                    output_mime: None,
                    output_truncated: false,
                    error_message: None,
                    created_at: chrono::Utc::now(),
                    org_id: key_ctx.as_ref().and_then(|c| c.org_id.clone()),
                    app_id: key_ctx.as_ref().and_then(|c| c.app_id.clone()),
                }
            };

            let db = state.db.clone();
            let id = response.id.clone();
            let model = response.model.clone();
            let provider = response.provider.clone();
            // Extract provider_job_id from the router's in-flight jobs map via response id.
            let provider_job_id_for_insert = state.router.get_provider_job_id(&id).await;
            let key_id_for_insert = key_ctx.as_ref().and_then(|c| c.key_id);
            let org_id = key_ctx.as_ref().and_then(|c| c.org_id.clone());
            let app_id = key_ctx.as_ref().and_then(|c| c.app_id.clone());
            tokio::spawn(async move {
                let _ = db
                    .log_request(&id, &model, &provider, "pending", "video", cost, latency, None, None, org_id.as_deref(), app_id.as_deref())
                    .await;
                let _ = db.insert_generation(
                    &id,
                    key_id_for_insert.as_ref(),
                    &model,
                    &provider,
                    "video",
                    provider_job_id_for_insert.as_deref(),
                    cost,
                    org_id.as_deref(),
                    app_id.as_deref(),
                ).await;
                if let Err(e) = db.insert_request_artifact(&video_artifact).await {
                    tracing::warn!(error = %e, request_id = %video_artifact.request_id, "Failed to store video artifact");
                }
            });
            let mut resp = (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
            if let Some((k, v)) = dropped_header(&validated.dropped) {
                resp.headers_mut().insert(k, v);
            }
            resp
        }
        Err(e) => {
            // Nothing was delivered — release the reservation.
            settle_quota(&state, charge_key, reserved, 0.0).await;
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            error!(error = %e, "Video generation failed");
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

/// GET /v1/videos/{id} — Poll the status of an in-flight video generation.
#[utoipa::path(
    get,
    path = "/v1/videos/{id}",
    params(("id" = String, Path, description = "Video generation ID")),
    responses(
        (status = 200, description = "Current status", body = VideoGenerationResponse),
        (status = 403, description = "Forbidden (no active org, or read:own boundary)", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Videos"
)]
pub async fn get_video_status(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Tenant scope: require an active org. If a persisted generation row exists
    // for this id, it must belong to the caller's org (→ 404 otherwise). Rows
    // that aren't yet persisted (router-tracked in-flight jobs) pass through.
    let ctx_org = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    if let Ok(Some(gen)) = state.db.get_generation(&id).await {
        if gen.org_id.as_deref() != Some(ctx_org) {
            return (StatusCode::NOT_FOUND, Json(error_response("Not found", 404))).into_response();
        }
        // Read:own boundary (same as get_generation) when the row is persisted.
        if let Err(resp) = authorize_generation_for_session(
            &state, key_ctx.as_ref(), &gen,
            crate::auth::permissions::Permission::GenerationReadAny,
            crate::auth::permissions::Permission::GenerationReadOwn,
            "generation:read:own",
        ).await {
            return resp;
        }
    }
    match state.router.get_video_status(&id).await {
        Ok(resp) => (StatusCode::OK, Json(serde_json::to_value(resp).unwrap())).into_response(),
        Err(e) => {
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

/// GET /v1/generations/{id} — Poll DB-backed generation status.
/// Live: `curl https://app.litegen.ai/api/v1/generations/litegen-vid-... -H "Authorization: Bearer sk_live_..."`
#[utoipa::path(
    get,
    path = "/v1/generations/{id}",
    params(("id" = String, Path, description = "Generation ID (litegen-vid-...)")),
    responses(
        (status = 200, description = "Generation detail", body = crate::types::Generation),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Generations"
)]
pub async fn get_generation(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Tenant scope: require an active org; never reveal another org's row (→ 404).
    let ctx_org = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    match state.db.get_generation(&id).await {
        Ok(Some(gen)) if gen.org_id.as_deref() == Some(ctx_org) => {
            // Enforce the same read:own boundary as list_generations so a member
            // can't fetch another member's generation (result_url/metadata) by id.
            if let Err(resp) = authorize_generation_for_session(
                &state, key_ctx.as_ref(), &gen,
                crate::auth::permissions::Permission::GenerationReadAny,
                crate::auth::permissions::Permission::GenerationReadOwn,
                "generation:read:own",
            ).await {
                return resp;
            }
            (StatusCode::OK, Json(serde_json::to_value(&gen).unwrap())).into_response()
        }
        Ok(_) => (StatusCode::NOT_FOUND, Json(error_response("Generation not found", 404))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to get generation");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response()
        }
    }
}

/// POST /v1/videos/cost — Estimate cost for video generation.
#[utoipa::path(
    post,
    path = "/v1/videos/cost",
    request_body = VideoGenerationRequest,
    responses(
        (status = 200, description = "Cost estimate", body = CostEstimate),
        (status = 400, description = "Bad request (validation failed)", body = ErrorResponse),
    ),
    tag = "Videos"
)]
pub async fn estimate_video_cost(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    validated: ValidatedVideo,
) -> impl IntoResponse {
    let app_creds = match resolve_org_provider_credential(&state, &key_ctx, &validated.schema.provider).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if app_creds.is_none() && !state.router.has_video_provider(&validated.schema.provider).await {
        return provider_not_configured_response(&validated.schema.provider);
    }
    match state.router.estimate_video_cost(&validated.schema, &validated.request, app_creds).await {
        Ok(estimate) => (StatusCode::OK, Json(serde_json::to_value(estimate).unwrap())).into_response(),
        Err(e) => {
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

// ─── 3D Model Generation ────────────────────────────────────────────────────

/// POST /v1/models3d/generations — Start a 3D (mesh) generation.
#[utoipa::path(
    post,
    path = "/v1/models3d/generations",
    request_body = Model3dGenerationRequest,
    responses(
        (status = 200, description = "3D generation started", body = Model3dGenerationResponse),
        (status = 400, description = "Bad request", body = ErrorResponse),
    ),
    tag = "Models3D"
)]
pub async fn generate_3d(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    validated: ValidatedModel3d,
) -> impl IntoResponse {
    let start = std::time::Instant::now();

    let materialized = match state.materializer.materialize(
        &validated.schema,
        validated.request.base.reference_images.clone(),
        &validated.ctx,
    ).await {
        Ok(m) => m,
        Err(e) => return validation_rejection_response(&e.to_string(), 400, &validated.schema.id),
    };

    let extras = Model3dExtras {
        output_format: validated.request.output_format.clone(),
        texture: validated.request.texture,
        pbr: validated.request.pbr,
        target_polycount: validated.request.target_polycount,
        symmetry: validated.request.symmetry.clone(),
        topology: validated.request.topology.clone(),
        rig: validated.request.rig,
        extra: validated.request.base.extra.clone(),
    };

    let app_creds = match resolve_org_provider_credential(&state, &key_ctx, &validated.schema.provider).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if app_creds.is_none() && !state.router.has_model3d_provider(&validated.schema.provider).await {
        return provider_not_configured_response(&validated.schema.provider);
    }

    let charge_key = key_ctx.as_ref().and_then(|c| c.key_id);
    let reserved = match reserve_quota(&state, charge_key, validated.schema.pricing.base_cost_usd).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    match state.router.generate_model3d(
        &validated.schema, &validated.request.base, &extras, &materialized, app_creds,
    ).await {
        Ok(response) => {
            let latency = start.elapsed().as_millis() as i64;
            let cost = response.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);
            settle_quota(&state, charge_key, reserved, cost).await;

            let artifact = RequestArtifact {
                request_id: response.id.clone(),
                media_type: "model3d".to_string(),
                prompt: Some(validated.request.base.prompt.clone()),
                negative_prompt: validated.request.base.negative_prompt.clone(),
                params_json: serde_json::to_value(&extras).ok(),
                refs_meta_json: build_refs_meta(&validated.request.base.reference_images),
                output_kind: "url".to_string(),
                output_value: None, // async — the mesh URL is not known yet
                output_mime: None,
                output_truncated: false,
                error_message: None,
                created_at: chrono::Utc::now(),
                org_id: key_ctx.as_ref().and_then(|c| c.org_id.clone()),
                app_id: key_ctx.as_ref().and_then(|c| c.app_id.clone()),
            };

            let db = state.db.clone();
            let id = response.id.clone();
            let model = response.model.clone();
            let provider = response.provider.clone();
            let provider_job_id = state.router.get_model3d_provider_job_id(&id).await;
            let key_id = key_ctx.as_ref().and_then(|c| c.key_id);
            let org_id = key_ctx.as_ref().and_then(|c| c.org_id.clone());
            let app_id = key_ctx.as_ref().and_then(|c| c.app_id.clone());
            tokio::spawn(async move {
                let _ = db.log_request(&id, &model, &provider, "pending", "model3d", cost, latency, None, None, org_id.as_deref(), app_id.as_deref()).await;
                let _ = db.insert_generation(
                    &id, key_id.as_ref(), &model, &provider, "model3d",
                    provider_job_id.as_deref(), cost, org_id.as_deref(), app_id.as_deref(),
                ).await;
                if let Err(e) = db.insert_request_artifact(&artifact).await {
                    tracing::warn!(error = %e, request_id = %artifact.request_id, "Failed to store 3d artifact");
                }
            });

            let mut resp = (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
            if let Some((k, v)) = dropped_header(&validated.dropped) {
                resp.headers_mut().insert(k, v);
            }
            resp
        }
        Err(e) => {
            settle_quota(&state, charge_key, reserved, 0.0).await;
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            error!(error = %e, "3D generation failed");
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

/// GET /v1/models3d/{id} — Poll the status of an in-flight 3D generation.
#[utoipa::path(
    get,
    path = "/v1/models3d/{id}",
    params(("id" = String, Path, description = "3D generation ID")),
    responses(
        (status = 200, description = "Current status", body = Model3dGenerationResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Models3D"
)]
pub async fn get_3d_status(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Same tenant scoping as get_video_status: a persisted row must belong to
    // the caller's org; router-tracked in-flight jobs pass through.
    let ctx_org = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    if let Ok(Some(gen)) = state.db.get_generation(&id).await {
        if gen.org_id.as_deref() != Some(ctx_org) {
            return (StatusCode::NOT_FOUND, Json(error_response("Not found", 404))).into_response();
        }
        if let Err(resp) = authorize_generation_for_session(
            &state, key_ctx.as_ref(), &gen,
            crate::auth::permissions::Permission::GenerationReadAny,
            crate::auth::permissions::Permission::GenerationReadOwn,
            "generation:read:own",
        ).await {
            return resp;
        }
    }

    match state.router.get_model3d_status(&id).await {
        Ok(mut resp) => {
            // Persist the terminal result so `GET /v1/generations/{id}`, the
            // gallery, and any webhook see the same assets the poller would have
            // written — whichever path observed completion first.
            if resp.status == GenerationStatus::Completed {
                if resp.mesh().is_some() {
                    persist_model3d_result(&state, &resp).await;
                } else {
                    // Contract: a completed 3D generation ALWAYS carries exactly
                    // one mesh asset. A provider that reports success without one
                    // has produced a generation the caller paid for and cannot
                    // use, so this is a failure, not a degraded success — the
                    // caller refunds on `failed`, and would not on `completed`.
                    // The poller (Task 11) applies the identical rule when it is
                    // the path that observes completion first.
                    let err = "provider reported success without a mesh asset";
                    tracing::warn!(generation_id = %id, "3d completed without a mesh asset, failing");
                    let _ = state.db.update_generation_status(
                        &id, "failed", resp.progress as i32, None,
                        Some(err), Some(chrono::Utc::now()),
                    ).await;
                    resp.status = GenerationStatus::Failed;
                    resp.error = Some(err.to_string());
                    resp.assets.clear();
                }
            } else if resp.status == GenerationStatus::Failed {
                let _ = state.db.update_generation_status(
                    &id, "failed", resp.progress as i32, None,
                    resp.error.as_deref(), Some(chrono::Utc::now()),
                ).await;
            }
            (StatusCode::OK, Json(serde_json::to_value(resp).unwrap())).into_response()
        }
        Err(e) => {
            // Fall through to the DB row: once the poller has terminalised a
            // job the router no longer tracks it.
            if let Ok(Some(gen)) = state.db.get_generation(&id).await {
                return (StatusCode::OK, Json(serde_json::to_value(model3d_response_from_row(&gen)).unwrap())).into_response();
            }
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

/// POST /v1/models3d/cost — Estimate cost for a 3D generation.
#[utoipa::path(
    post,
    path = "/v1/models3d/cost",
    request_body = Model3dGenerationRequest,
    responses(
        (status = 200, description = "Cost estimate", body = CostEstimate),
        (status = 400, description = "Bad request", body = ErrorResponse),
    ),
    tag = "Models3D"
)]
pub async fn estimate_3d_cost(
    State(state): State<Arc<AppState>>,
    validated: ValidatedModel3d,
) -> impl IntoResponse {
    match state.router.estimate_model3d_cost(&validated.schema, &validated.request).await {
        Ok(est) => (StatusCode::OK, Json(serde_json::to_value(est).unwrap())).into_response(),
        Err(e) => {
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

/// Write a completed 3D result to its generation row: `result_url` is the
/// primary mesh (so every existing cross-modal consumer keeps working), and the
/// full asset list rides `metadata.assets`.
pub(crate) async fn persist_model3d_result(state: &AppState, resp: &Model3dGenerationResponse) {
    let mesh_url = resp.mesh().map(|m| m.url.clone());
    if let Err(e) = state.db.update_generation_status(
        &resp.id, "completed", resp.progress as i32,
        mesh_url.as_deref(), None, Some(chrono::Utc::now()),
    ).await {
        tracing::warn!(generation_id = %resp.id, error = %e, "failed to persist 3d status");
        return;
    }
    let meta = serde_json::json!({ "assets": resp.assets });
    if let Err(e) = state.db.update_generation_metadata(&resp.id, &meta).await {
        tracing::warn!(generation_id = %resp.id, error = %e, "failed to persist 3d assets");
    }
}

/// Rebuild a `Model3dGenerationResponse` from a persisted row (used once the
/// poller has terminalised a job and the router no longer tracks it).
pub(crate) fn model3d_response_from_row(gen: &crate::types::Generation) -> Model3dGenerationResponse {
    let assets = gen.metadata.as_ref()
        .and_then(|m| m.get("assets"))
        .and_then(|a| serde_json::from_value::<Vec<Model3dAsset>>(a.clone()).ok())
        .unwrap_or_default();
    Model3dGenerationResponse {
        id: gen.id.clone(),
        status: gen.status,
        model: gen.model.clone(),
        provider: gen.provider.clone(),
        assets,
        progress: gen.progress.clamp(0, 100) as u8,
        error: gen.error_message.clone(),
        usage: None,
        created: gen.created_at.timestamp(),
    }
}

// ─── Models ─────────────────────────────────────────────────────────────────

/// Optional filter for `GET /v1/models`.
#[derive(serde::Deserialize)]
pub struct ListModelsQuery {
    /// Optional `image` | `video` | `model3d` filter.
    #[serde(default)]
    pub media_type: Option<String>,
}

/// GET /v1/models — List all available models.
#[utoipa::path(
    get,
    path = "/v1/models",
    params(("media_type" = Option<String>, Query, description = "Filter by media type")),
    responses(
        (status = 200, description = "List of available models", body = ModelListResponse),
    ),
    tag = "Models"
)]
pub async fn list_models(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListModelsQuery>,
) -> impl IntoResponse {
    let models: Vec<ModelInfo> = state.registry.all()
        .map(project_model_info)
        .filter(|m| match q.media_type.as_deref() {
            None | Some("") => true,
            Some(want) => serde_json::to_value(m.media_type)
                .ok()
                .and_then(|v| v.as_str().map(|s| s == want))
                .unwrap_or(false),
        })
        .collect();
    Json(ModelListResponse { object: "list".to_string(), data: models })
}

/// GET /v1/models/{id} — Full schema for one model.
#[utoipa::path(
    get,
    path = "/v1/models/{id}",
    params(("id" = String, Path, description = "Model ID (may contain slashes, e.g. 'openai/dall-e-3')")),
    responses(
        (status = 200, description = "Model schema", body = crate::capabilities::ModelSchema),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Models"
)]
pub async fn get_model_schema(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.registry.get(&id) {
        Some(schema) => (StatusCode::OK, Json(serde_json::to_value(schema).unwrap())).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(error_response(&format!("model '{}' not found", id), 404)),
        ).into_response(),
    }
}

fn project_model_info(s: &crate::capabilities::ModelSchema) -> ModelInfo {
    ModelInfo {
        id: s.id.clone(),
        name: s.display_name.clone(),
        description: s.description.clone(),
        provider: s.provider.clone(),
        media_type: match s.media_type {
            crate::capabilities::MediaType::Image => MediaType::Image,
            crate::capabilities::MediaType::Video => MediaType::Video,
            crate::capabilities::MediaType::Model3d => MediaType::Model3d,
        },
        is_available: true,
        capabilities: ModelCapabilities {
            supports_text_to_image: s.capabilities.text_to_image,
            supports_image_to_image: s.capabilities.image_to_image,
            supports_inpainting: s.capabilities.inpainting,
            supports_text_to_video: s.capabilities.text_to_video,
            supports_image_to_video: s.capabilities.image_to_video,
            supports_first_frame: s.ref_inputs.as_ref().is_some_and(|ri| ri.roles.contains_key("first_frame")),
            supports_last_frame: s.ref_inputs.as_ref().is_some_and(|ri| ri.roles.contains_key("last_frame")),
            supported_sizes: extract_sizes(s),
            max_images: s.ref_inputs.as_ref().map(|ri| ri.max_total).unwrap_or(1),
            max_duration_seconds: None,
            supports_text_to_3d: s.capabilities.text_to_3d,
            supports_image_to_3d: s.capabilities.image_to_3d,
            supports_multiview_to_3d: s.capabilities.multiview_to_3d,
            supports_pbr: s.params.contains_key("pbr"),
            supports_rig: s.params.contains_key("rig"),
            supports_texture: s.params.contains_key("texture"),
            output_formats: extract_output_formats(s),
            max_polycount: extract_max_polycount(s),
        },
        pricing: Some(ModelPricing {
            base_cost_usd: s.pricing.base_cost_usd,
            variable_pricing: s.pricing.variable_pricing.clone(),
        }),
        tags: s.tags.clone(),
    }
}

fn extract_sizes(s: &crate::capabilities::ModelSchema) -> Vec<String> {
    match s.params.get("size") {
        Some(crate::capabilities::ParamSpec::Size(crate::capabilities::SizeSpec::Enum(e))) => {
            e.values.iter().map(|(w, h)| format!("{}x{}", w, h)).collect()
        }
        _ => Vec::new(),
    }
}

/// `output_formats` is derived from the model's declared `output_format`
/// enum_values, so a model advertises exactly the formats it can emit.
fn extract_output_formats(s: &crate::capabilities::ModelSchema) -> Vec<String> {
    match s.params.get("output_format") {
        Some(crate::capabilities::ParamSpec::String(sp)) => sp.enum_values.clone(),
        _ => Vec::new(),
    }
}

/// `max_polycount` mirrors the upper bound of the `target_polycount` param.
fn extract_max_polycount(s: &crate::capabilities::ModelSchema) -> Option<u32> {
    match s.params.get("target_polycount") {
        Some(crate::capabilities::ParamSpec::Int(i)) => i.max.map(|m| m.max(0) as u32),
        _ => None,
    }
}

// ─── Providers ────────────────────────────────────────────────────────────────

/// GET /v1/providers — Catalog of supported providers and the credential fields
/// each one needs. Drives the dashboard's dynamic credential form.
/// Live: `curl https://app.litegen.ai/api/v1/providers -H "Authorization: Bearer sk_live_..."`
#[utoipa::path(
    get,
    path = "/v1/providers",
    responses(
        (status = 200, description = "Provider catalog", body = [crate::types::ProviderCatalogEntry]),
    ),
    tag = "Providers"
)]
pub async fn list_providers() -> impl IntoResponse {
    Json(crate::proxy::registry::provider_catalog())
}

// ─── Health ─────────────────────────────────────────────────────────────────

/// GET /health — Health check for all providers.
#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Health check results", body = HealthResponse),
    ),
    tag = "System"
)]
pub async fn health_check(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let health = state.router.registry.health_check_all().await;
    let all_healthy = health.iter().all(|h| h.healthy);
    let status_code = if all_healthy || health.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = HealthResponse {
        status: if all_healthy { "healthy".to_string() } else { "degraded".to_string() },
        providers: health,
        cache: CacheStatus {
            enabled: state.router.cache.is_enabled(),
            entries: state.router.cache.entry_count(),
        },
    };
    (status_code, Json(body))
}

/// GET /health/live — Simple liveness probe.
#[utoipa::path(
    get,
    path = "/health/live",
    responses(
        (status = 200, description = "Liveness probe", body = LivenessResponse),
    ),
    tag = "System"
)]
pub async fn liveness() -> impl IntoResponse {
    Json(LivenessResponse { status: "ok".to_string() })
}

/// GET /health/ready — Readiness probe.
/// Returns 200 when the DB is reachable (the instance can accept requests).
/// Provider availability is resolved per-request — globally configured creds OR
/// org-scoped BYO credentials — so it does NOT gate readiness; the healthy
/// provider list is reported in the body purely as an informational signal.
/// Returns 503 only when the DB is unreachable. No auth required.
/// Live: `curl https://app.litegen.ai/api/health/ready`
#[utoipa::path(
    get,
    path = "/health/ready",
    responses(
        (status = 200, description = "Service is ready", body = ReadinessResponse),
        (status = 503, description = "Service is not ready", body = ReadinessResponse),
    ),
    tag = "System"
)]
pub async fn readiness(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let db_ok = state.db.ping().await.is_ok();

    let health_results = state.router.registry.health_check_all().await;
    let healthy_providers: Vec<String> = health_results
        .into_iter()
        .filter(|h| h.healthy)
        .map(|h| h.provider.clone())
        .collect();

    // Readiness gates on the DB only: provider availability is resolved
    // per-request (global config OR org-scoped BYO credentials), so an empty
    // global-provider list must not mark a BYO deployment "not_ready" — it serves
    // fine. `healthy_providers` stays in the body as an informational signal.
    let body = ReadinessResponse {
        status: if db_ok { "ready".to_string() } else { "not_ready".to_string() },
        checks: ReadinessChecks {
            db: db_ok,
            providers: healthy_providers,
        },
    };

    let status_code = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status_code, Json(serde_json::to_value(body).unwrap()))
}

// ─── Stats / Dashboard ──────────────────────────────────────────────────────

/// GET /v1/stats — Get aggregate usage statistics.
#[utoipa::path(
    get,
    path = "/v1/stats",
    responses(
        (status = 200, description = "Usage statistics", body = ProxyStats),
    ),
    tag = "Dashboard"
)]
pub async fn get_stats(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
) -> impl IntoResponse {
    // Tenant scope: require an active org; stats are computed over org (+ app when set).
    let org_id = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    let app_id = key_ctx.as_ref().and_then(|c| c.app_id.as_deref());
    match state.db.get_stats_for_tenant(org_id, app_id).await {
        Ok(stats) => (StatusCode::OK, Json(serde_json::to_value(stats).unwrap())).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to get stats");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}
fn default_page() -> u32 { 1 }
fn default_per_page() -> u32 { 50 }

// ─── API Key Management ─────────────────────────────────────────────────────

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateApiKeyRequest {
    pub name: String,
    /// USD budget cap; None = unlimited.
    #[serde(default)]
    pub token_quota: Option<f64>,
    /// Requests-per-minute cap; None = unlimited.
    #[serde(default)]
    pub rpm_limit: Option<u32>,
    /// CSV of scopes (default: "generate,read").
    #[serde(default = "default_key_scopes")]
    pub scopes: String,
    /// Webhook URL for async callbacks.
    #[serde(default)]
    pub webhook_url: Option<String>,
    /// Optional expiry; after this instant the key is rejected at auth. None =
    /// never expires. Enforced in auth_middleware and honoured on PATCH too.
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn default_key_scopes() -> String { "generate,read".to_string() }

/// SSRF guard for tenant-supplied webhook URLs. In hosted (multi-tenant) mode
/// the URL is attacker-controlled and the server POSTs to it (poller +
/// test-webhook), so it must not target a private/loopback/link-local address
/// (e.g. the cloud metadata endpoint). In single-tenant mode the operator is
/// trusted and may legitimately target internal hosts, so any URL is allowed.
/// Returns an error `Response` to send to the client when the URL is rejected.
async fn validate_tenant_webhook_url(state: &AppState, url: &str) -> Result<(), axum::response::Response> {
    if state.mode != crate::config::Mode::Hosted || url.is_empty() {
        return Ok(());
    }
    if let Err(reason) = crate::util::ssrf::validate_public_url(url).await {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "code": "invalid_webhook_url",
                    "message": format!("webhook_url is not allowed: {reason}"),
                    "type": "invalid_request_error"
                }
            })),
        )
            .into_response());
    }
    Ok(())
}

/// POST /v1/keys — Create a new API key.
#[utoipa::path(
    post,
    path = "/v1/keys",
    request_body = CreateApiKeyRequest,
    responses(
        (status = 201, description = "API key created", body = ApiKeyCreatedResponse),
    ),
    tag = "Admin"
)]
pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Json(request): Json<CreateApiKeyRequest>,
) -> impl IntoResponse {
    // Session-auth users need KeyWriteOwn or KeyWriteAny; Bearer/master key passes through
    if let Some(ref ctx) = key_ctx {
        if ctx.user.is_some() {
            use crate::auth::permissions::Permission;
            if !ctx.permissions.contains(&Permission::KeyWriteOwn)
                && !ctx.permissions.contains(&Permission::KeyWriteAny)
            {
                return (
                    StatusCode::FORBIDDEN,
                    Json(error_response("key:write:own permission required", 403)),
                ).into_response();
            }
        }
    }

    // Tenant scope: a key is always issued into a specific org + app. A hosted
    // master/platform context (org_id None) cannot mint tenant keys → 403.
    let org_id = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    let app_id = match key_ctx.as_ref().and_then(|c| c.app_id.as_deref()) {
        Some(a) => a,
        None => return forbidden_no_org(),
    };

    // SSRF guard: reject a tenant-supplied webhook URL that targets an internal
    // address (hosted mode only).
    if let Some(wh) = request.webhook_url.as_deref() {
        if let Err(resp) = validate_tenant_webhook_url(&state, wh).await {
            return resp;
        }
    }

    // Privilege check: a key must never be granted more authority than its
    // creator holds. In particular the `admin` scope bypasses permission gates
    // on the Bearer path, so a Member (or a non-admin key) must not be able to
    // mint an admin-scope key. Reject unknown scope tokens too, so a typo is a
    // 400 rather than a silently dropped (and thus missing) scope.
    if let Some(ref ctx) = key_ctx {
        let grantable = crate::api::middleware::grantable_scopes(ctx);
        for token in request.scopes.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match crate::api::middleware::Scope::parse(token) {
                Some(scope) if grantable.contains(&scope) => {}
                Some(_) => {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(error_response(
                            &format!("insufficient privilege to grant scope '{token}'"),
                            403,
                        )),
                    ).into_response();
                }
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(error_response(&format!("unknown scope '{token}'"), 400)),
                    ).into_response();
                }
            }
        }
    }

    // Mint a pk_live_/sk_live_ id+secret pair. The secret_hash is what auth looks up.
    let kp = crate::auth::secrets::generate_key_pair();

    // Determine owner: session user → set owner_user_id; master key → None
    let owner_user_id = key_ctx.as_ref()
        .and_then(|c| c.user.as_ref())
        .map(|u| u.user_id.clone());

    match state.db.create_api_key_scoped(
        org_id, app_id, &kp.public_id, &request.name,
        &kp.secret_hash, &kp.prefix,
        request.token_quota, request.rpm_limit,
        &request.scopes, request.webhook_url.as_deref(),
        request.expires_at,
    ).await {
        Ok(key) => {
            // Set owner if session-authenticated
            if let Some(ref uid) = owner_user_id {
                let _ = state.db.set_api_key_owner(&key.id, uid).await;
            }
            log_audit(
                state.db.clone(),
                key_ctx.as_ref(),
                "key.create",
                "api_key",
                &key.id.to_string(),
                None,
                serde_json::to_value(serde_json::json!({
                    "name": key.name,
                    "scopes": key.scopes,
                    "token_quota": key.token_quota,
                    "rpm_limit": key.rpm_limit,
                })).ok(),
            );
            (
                StatusCode::CREATED,
                Json(ApiKeyCreatedResponse {
                    id: key.id,
                    public_id: kp.public_id,
                    key: kp.secret,
                    prefix: kp.prefix,
                    name: key.name,
                    created_at: key.created_at,
                    token_quota: key.token_quota,
                    rpm_limit: key.rpm_limit,
                    scopes: key.scopes,
                }),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to create API key");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response()
        }
    }
}

/// GET /v1/keys — List API keys.
#[utoipa::path(
    get,
    path = "/v1/keys",
    responses(
        (status = 200, description = "API keys", body = ApiKeyListResponse),
    ),
    tag = "Admin"
)]
pub async fn list_api_keys(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
) -> impl IntoResponse {
    use crate::auth::permissions::Permission;

    // Tenant scope: keys are listed within the active application. A hosted
    // master/platform context (app_id None) has no tenant → 403.
    let app_id = match key_ctx.as_ref().and_then(|c| c.app_id.as_deref()) {
        Some(a) => a,
        None => return forbidden_no_org(),
    };

    // Determine what the caller can see, scoped to the active app.
    let keys_result = if let Some(ref ctx) = key_ctx {
        if let Some(ref user) = ctx.user {
            // Session auth: admins see the whole app; members only their own keys.
            if ctx.permissions.contains(&Permission::KeyReadAny) {
                state.db.list_api_keys_for_app(app_id).await
            } else if ctx.permissions.contains(&Permission::KeyReadOwn) {
                state.db.list_api_keys_for_app(app_id).await.map(|keys| {
                    keys.into_iter()
                        .filter(|k| k.owner_user_id.as_deref() == Some(user.user_id.as_str()))
                        .collect()
                })
            } else {
                return (StatusCode::FORBIDDEN, Json(error_response("key:read:own permission required", 403))).into_response();
            }
        } else {
            // Bearer/master key path: all keys in the active app.
            state.db.list_api_keys_for_app(app_id).await
        }
    } else {
        // No auth context (dev mode) — keys in the active app.
        state.db.list_api_keys_for_app(app_id).await
    };

    match keys_result {
        Ok(keys) => {
            let data: Vec<ApiKeyInfo> = keys
                .into_iter()
                .map(|k| ApiKeyInfo {
                    id: k.id,
                    name: k.name,
                    prefix: k.key_prefix,
                    created_at: k.created_at,
                    expires_at: k.expires_at,
                    is_active: k.is_active,
                    token_quota: k.token_quota,
                    tokens_used: k.tokens_used,
                    rpm_limit: k.rpm_limit,
                    scopes: k.scopes,
                    webhook_url: k.webhook_url,
                    public_id: k.public_id,
                    app_id: k.app_id,
                })
                .collect();
            Json(ApiKeyListResponse { data }).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response()
        }
    }
}

/// DELETE /v1/keys/:id — Revoke an API key.
#[utoipa::path(
    delete,
    path = "/v1/keys/{id}",
    params(("id" = Uuid, Path, description = "API key ID")),
    responses(
        (status = 200, description = "Key revoked", body = RevokeKeyResponse),
        (status = 404, description = "Key not found", body = ErrorResponse),
    ),
    tag = "Admin"
)]
pub async fn revoke_api_key(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    use crate::auth::permissions::Permission;
    // Tenant scope: require an org and only operate on keys in that org. A key
    // belonging to another org (or absent) is indistinguishable → 404.
    let ctx_org = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    match state.db.get_api_key(&id).await {
        Ok(Some(k)) if k.org_id.as_deref() == Some(ctx_org) => {}
        Ok(_) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response(),
    }
    // Check ownership for session-authed users
    if let Some(ref ctx) = key_ctx {
        if let Some(ref user) = ctx.user {
            if !ctx.permissions.contains(&Permission::KeyDeleteAny) {
                // Need KeyDeleteOwn + must own the key
                if ctx.permissions.contains(&Permission::KeyDeleteOwn) {
                    match state.db.get_api_key(&id).await {
                        Ok(Some(k)) if k.owner_user_id.as_deref() != Some(&user.user_id) => {
                            return (StatusCode::FORBIDDEN, Json(error_response("Forbidden", 403))).into_response();
                        }
                        Ok(None) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
                        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response(),
                        _ => {}
                    }
                } else {
                    return (StatusCode::FORBIDDEN, Json(error_response("key:delete:own permission required", 403))).into_response();
                }
            }
        }
    }
    match state.db.revoke_api_key(&id).await {
        Ok(true) => {
            log_audit(state.db.clone(), key_ctx.as_ref(), "key.revoke", "api_key", &id.to_string(), None, None);
            (StatusCode::OK, Json(RevokeKeyResponse { revoked: true })).into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response()
        }
    }
}

/// GET /v1/keys/:id — Get a single API key by ID.
/// Live: `curl https://app.litegen.ai/api/v1/keys/{id} -H "Authorization: Bearer sk_live_..."`
#[utoipa::path(
    get,
    path = "/v1/keys/{id}",
    params(("id" = Uuid, Path, description = "API key ID")),
    responses(
        (status = 200, description = "API key detail", body = ApiKeyDetail),
        (status = 404, description = "Key not found", body = ErrorResponse),
    ),
    tag = "Admin"
)]
pub async fn get_api_key_handler(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    use crate::auth::permissions::Permission;
    // Tenant scope: require an org; never reveal another org's key (→ 404).
    let ctx_org = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    match state.db.get_api_key(&id).await {
        Ok(Some(key)) if key.org_id.as_deref() == Some(ctx_org) => {
            // Ownership check for session-authed users
            if let Some(ref ctx) = key_ctx {
                if let Some(ref user) = ctx.user {
                    if !ctx.permissions.contains(&Permission::KeyReadAny) {
                        if ctx.permissions.contains(&Permission::KeyReadOwn) {
                            if key.owner_user_id.as_deref() != Some(&user.user_id) {
                                return (StatusCode::FORBIDDEN, Json(error_response("Forbidden", 403))).into_response();
                            }
                        } else {
                            return (StatusCode::FORBIDDEN, Json(error_response("key:read:own permission required", 403))).into_response();
                        }
                    }
                }
            }
            let detail = ApiKeyDetail {
                id: key.id,
                name: key.name,
                key_prefix: key.key_prefix,
                created_at: key.created_at,
                expires_at: key.expires_at,
                is_active: key.is_active,
                token_quota: key.token_quota,
                tokens_used: key.tokens_used,
                rpm_limit: key.rpm_limit,
                scopes: key.scopes,
                webhook_url: key.webhook_url,
            };
            (StatusCode::OK, Json(serde_json::to_value(detail).unwrap())).into_response()
        }
        // None, or a key belonging to another org → 404 (don't reveal other orgs).
        Ok(_) => (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response(),
    }
}

/// PATCH /v1/keys/:id — Update an API key's quota/rpm/scopes/etc.
/// Live: `curl -X PATCH https://app.litegen.ai/api/v1/keys/{id} -H "Authorization: Bearer sk_live_..." -d '{"rpm_limit":120}'`
#[utoipa::path(
    patch,
    path = "/v1/keys/{id}",
    params(("id" = Uuid, Path, description = "API key ID")),
    request_body = UpdateApiKeyRequest,
    responses(
        (status = 200, description = "Updated API key", body = ApiKeyDetail),
        (status = 404, description = "Key not found", body = ErrorResponse),
    ),
    tag = "Admin"
)]
pub async fn patch_api_key_handler(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateApiKeyRequest>,
) -> impl IntoResponse {
    use crate::auth::permissions::Permission;
    // Tenant scope: require an org; a key in another org (or absent) → 404.
    let ctx_org = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    match state.db.get_api_key(&id).await {
        Ok(Some(k)) if k.org_id.as_deref() == Some(ctx_org) => {}
        Ok(_) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response(),
    }
    // Ownership check for session-authed users
    if let Some(ref ctx) = key_ctx {
        if let Some(ref user) = ctx.user {
            if !ctx.permissions.contains(&Permission::KeyWriteAny) {
                if ctx.permissions.contains(&Permission::KeyWriteOwn) {
                    match state.db.get_api_key(&id).await {
                        Ok(Some(k)) if k.owner_user_id.as_deref() != Some(&user.user_id) => {
                            return (StatusCode::FORBIDDEN, Json(error_response("Forbidden", 403))).into_response();
                        }
                        Ok(None) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
                        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response(),
                        _ => {}
                    }
                } else {
                    return (StatusCode::FORBIDDEN, Json(error_response("key:write:own permission required", 403))).into_response();
                }
            }
        }
    }
    // SSRF guard: reject a tenant-supplied webhook URL that targets an internal
    // address (hosted mode only).
    if let Some(wh) = req.webhook_url.as_deref() {
        if let Err(resp) = validate_tenant_webhook_url(&state, wh).await {
            return resp;
        }
    }
    match state.db.update_api_key(&id, &req).await {
        Ok(Some(key)) => {
            let detail = ApiKeyDetail {
                id: key.id,
                name: key.name,
                key_prefix: key.key_prefix,
                created_at: key.created_at,
                expires_at: key.expires_at,
                is_active: key.is_active,
                token_quota: key.token_quota,
                tokens_used: key.tokens_used,
                rpm_limit: key.rpm_limit,
                scopes: key.scopes,
                webhook_url: key.webhook_url,
            };
            log_audit(
                state.db.clone(),
                key_ctx.as_ref(),
                "key.update",
                "api_key",
                &id.to_string(),
                None,
                serde_json::to_value(&detail).ok(),
            );
            (StatusCode::OK, Json(serde_json::to_value(detail).unwrap())).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response(),
    }
}

// ─── Generation List & Cancel ───────────────────────────────────────────────

/// GET /v1/generations — Paginated list of generations.
/// Live: `curl https://app.litegen.ai/api/v1/generations?page=1&per_page=50 -H "Authorization: Bearer sk_live_..."`
#[utoipa::path(
    get,
    path = "/v1/generations",
    params(
        ("page" = Option<u32>, Query, description = "Page number (default 1)"),
        ("per_page" = Option<u32>, Query, description = "Items per page (default 50)"),
    ),
    responses(
        (status = 200, description = "Paginated generations", body = PaginatedResponse<crate::types::Generation>),
        (status = 403, description = "Forbidden", body = ErrorResponse),
    ),
    tag = "Generations"
)]
pub async fn list_generations(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Query(query): Query<PaginationParams>,
) -> impl IntoResponse {
    use crate::auth::permissions::Permission;

    // Clamp caller-supplied paging so per_page can't drive an unbounded SQL LIMIT
    // / in-memory buffer and the offset can't overflow u32. See api::pagination.
    let page = crate::api::pagination::clamp_page(query.page);
    let per_page = crate::api::pagination::clamp_per_page(query.per_page);

    // Tenant scope: require an active org; results are scoped to org (+ app when set).
    let org_id = match ctx.org_id.as_deref() {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    let app_id = ctx.app_id.as_deref();

    // Session-auth members without any generation:read permission are denied.
    // Members with read:own (but not read:any) additionally see only their own keys.
    let owned_filter: Option<Vec<uuid::Uuid>> = if ctx.user.is_some() {
        if ctx.permissions.contains(&Permission::GenerationReadAny) {
            None
        } else if ctx.permissions.contains(&Permission::GenerationReadOwn) {
            let user_id = ctx.user.as_ref().map(|u| u.user_id.as_str()).unwrap_or("");
            let owned_keys = state.db.list_api_keys_for_owner(user_id).await.unwrap_or_default();
            Some(owned_keys.iter().map(|k| k.id).collect())
        } else {
            return (StatusCode::FORBIDDEN, Json(error_response("generation:read:own permission required", 403))).into_response();
        }
    } else {
        None
    };

    if let Some(owned_key_ids) = owned_filter {
        // Read-own member: filter to the caller's owned keys AND paginate/count
        // at the DB, so their complete history is visible (not just whichever
        // rows fall inside a truncated in-memory window).
        let (paged, total) = match state.db
            .list_generations_for_owner(org_id, app_id, &owned_key_ids, page, per_page)
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                error!(error = %e, "Failed to list generations");
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
            }
        };
        let total_pages = crate::api::pagination::total_pages(total, per_page);
        let response = PaginatedResponse {
            data: paged,
            total,
            page,
            per_page,
            total_pages,
        };
        return (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
    }

    let (total, gens) = match tokio::try_join!(
        state.db.count_generations_for_tenant(org_id, app_id),
        state.db.list_generations_for_tenant(org_id, app_id, page, per_page),
    ) {
        Ok(pair) => pair,
        Err(e) => {
            error!(error = %e, "Failed to list generations");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };

    let total_pages = crate::api::pagination::total_pages(total as u64, per_page);
    let response = PaginatedResponse {
        data: gens,
        total: total as u64,
        page,
        per_page,
        total_pages,
    };
    (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response()
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CancelGenerationBody {
    /// Target status; only `"cancelled"` is honored.
    pub status: String,
}

/// PATCH /v1/generations/{id} — Soft-cancel a generation.
/// Live: `curl -X PATCH https://app.litegen.ai/api/v1/generations/{id} -H "Authorization: Bearer sk_live_..." -d '{"status":"cancelled"}'`
#[utoipa::path(
    patch,
    path = "/v1/generations/{id}",
    params(("id" = String, Path, description = "Generation ID (litegen-vid-...)")),
    request_body = CancelGenerationBody,
    responses(
        (status = 200, description = "Cancelled generation", body = crate::types::Generation),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 409, description = "Not in a cancellable state", body = ErrorResponse),
    ),
    tag = "Generations"
)]
pub async fn cancel_generation(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Path(id): Path<String>,
    Json(_body): Json<CancelGenerationBody>,
) -> impl IntoResponse {
    // Tenant scope: require an active org; a row in another org (or absent) → 404.
    let ctx_org = match ctx.org_id.as_deref() {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    // First fetch the generation to verify it belongs to the caller's org.
    let gen = match state.db.get_generation(&id).await {
        Ok(Some(g)) if g.org_id.as_deref() == Some(ctx_org) => g,
        Ok(_) => return (StatusCode::NOT_FOUND, Json(error_response("Generation not found", 404))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to get generation for cancel");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };

    // Cancelling is a state change on another principal's job: gate session users
    // on generation:cancel:any, or cancel:own for a generation they own.
    if let Err(resp) = authorize_generation_for_session(
        &state, Some(&ctx), &gen,
        crate::auth::permissions::Permission::GenerationCancelAny,
        crate::auth::permissions::Permission::GenerationCancelOwn,
        "generation:cancel:own",
    ).await {
        return resp;
    }

    // Attempt the cancel — returns None if status wasn't pending/processing.
    match state.db.cancel_generation(&id).await {
        Ok(Some(updated)) => {
            log_audit(
                state.db.clone(),
                Some(&ctx),
                "generation.cancel",
                "generation",
                &id,
                None,
                serde_json::to_value(&updated).ok(),
            );
            (StatusCode::OK, Json(serde_json::to_value(&updated).unwrap())).into_response()
        }
        Ok(None) => (StatusCode::CONFLICT, Json(error_response(
            "Generation cannot be cancelled: not in pending or processing state", 409,
        ))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to cancel generation");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response()
        }
    }
}

// ─── Key Rotate ──────────────────────────────────────────────────────────────

/// POST /v1/keys/{id}/rotate — Issue a new secret for an existing key in place.
/// Keeps the same row id and all settings; the new `sk_live_` secret is returned once.
/// Live: `curl -X POST https://app.litegen.ai/api/v1/keys/{id}/rotate -H "Authorization: Bearer sk_live_..."`
#[utoipa::path(
    post,
    path = "/v1/keys/{id}/rotate",
    params(("id" = Uuid, Path, description = "API key ID")),
    responses(
        (status = 200, description = "Rotated key with the new one-time secret", body = crate::types::RotatedKeyResponse),
        (status = 404, description = "Key not found", body = ErrorResponse),
    ),
    tag = "Admin"
)]
pub async fn rotate_api_key(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    // Tenant scope: require an org; a key in another org (or absent) → 404.
    let ctx_org = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };

    // 1. Fetch the existing key and verify it belongs to the caller's org.
    let existing = match state.db.get_api_key(&id).await {
        Ok(Some(k)) if k.org_id.as_deref() == Some(ctx_org) => k,
        Ok(_) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to fetch key for rotate");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };

    // Rotation mints a new working secret — gate it like patch/revoke: session
    // users need key:write:any, or key:write:own on a key they own. (Without
    // this, any session — including a read-only Viewer — could rotate any key
    // in the org, since all sessions carry the Admin scope.)
    use crate::auth::permissions::Permission;
    if let Err(resp) = authorize_key_for_session(
        key_ctx.as_ref(), &existing,
        Permission::KeyWriteAny, Permission::KeyWriteOwn, "key:write:own",
    ) {
        return resp;
    }

    // 2. Mint a fresh id/secret pair and update the same row in place.
    let kp = crate::auth::secrets::generate_key_pair();
    let rotated = match state.db.rotate_api_key(&id, &kp.public_id, &kp.secret_hash, &kp.prefix).await {
        Ok(Some(k)) => k,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to rotate key");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };

    log_audit(
        state.db.clone(),
        key_ctx.as_ref(),
        "key.rotate",
        "api_key",
        &id.to_string(),
        None,
        serde_json::to_value(serde_json::json!({ "public_id": kp.public_id })).ok(),
    );

    // 3. Return the new secret (only time it's visible).
    let body = crate::types::RotatedKeyResponse {
        id: rotated.id,
        public_id: kp.public_id,
        key: kp.secret,
        prefix: rotated.key_prefix,
        name: rotated.name,
        scopes: rotated.scopes,
        token_quota: rotated.token_quota,
        rpm_limit: rotated.rpm_limit,
        webhook_url: rotated.webhook_url,
        expires_at: rotated.expires_at,
    };
    (StatusCode::OK, Json(serde_json::to_value(body).unwrap())).into_response()
}

// ─── Key Test-Webhook ────────────────────────────────────────────────────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WebhookTestResult {
    pub delivered: bool,
    pub status_code: Option<u16>,
    pub error: Option<String>,
}

/// POST /v1/keys/{id}/test-webhook — Fire one synthetic webhook and return the result.
/// Live: `curl -X POST https://app.litegen.ai/api/v1/keys/{id}/test-webhook -H "Authorization: Bearer sk_live_..."`
#[utoipa::path(
    post,
    path = "/v1/keys/{id}/test-webhook",
    params(("id" = Uuid, Path, description = "API key ID")),
    responses(
        (status = 200, description = "Delivery result (delivered=false if the endpoint rejected it)", body = WebhookTestResult),
        (status = 400, description = "No webhook_url configured on the key", body = ErrorResponse),
        (status = 404, description = "Key not found", body = ErrorResponse),
    ),
    tag = "Admin"
)]
pub async fn test_webhook(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    // Tenant scope: require an org; a key in another org (or absent) → 404. This
    // must run before any webhook work so org A cannot trigger org B's webhook or
    // probe key existence.
    let ctx_org = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    let key = match state.db.get_api_key(&id).await {
        Ok(Some(k)) if k.org_id.as_deref() == Some(ctx_org) => k,
        Ok(_) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to fetch key for test-webhook");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };

    // Firing a webhook is a write-ish action on the key: gate session users.
    if let Err(resp) = authorize_key_for_session(
        key_ctx.as_ref(), &key,
        crate::auth::permissions::Permission::KeyTestWebhookAny,
        crate::auth::permissions::Permission::KeyTestWebhookOwn,
        "key:test_webhook:own",
    ) {
        return resp;
    }

    let webhook_url = match key.webhook_url.as_deref().filter(|u| !u.is_empty()) {
        Some(u) => u.to_string(),
        None => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": {
                    "code": "no_webhook_configured",
                    "message": "This key has no webhook_url configured",
                    "type": "invalid_request_error"
                }
            }))).into_response();
        }
    };

    // SSRF guard: re-validate at dispatch time (hosted mode) so a key whose
    // webhook_url predates this check — or a host that now resolves to an
    // internal address — can't turn this endpoint into an SSRF oracle.
    if let Err(resp) = validate_tenant_webhook_url(&state, &webhook_url).await {
        return resp;
    }

    let now = chrono::Utc::now();
    let synthetic = crate::types::Generation {
        id: format!("webhook-test-{}", Uuid::new_v4()),
        key_id: Some(key.id),
        model: "test/webhook".into(),
        provider: "test".into(),
        media_type: "video".into(),
        status: crate::types::GenerationStatus::Completed,
        progress: 100,
        provider_job_id: None,
        result_url: Some("https://example.com/test.mp4".into()),
        error_message: None,
        cost_usd: 0.0,
        created_at: now,
        completed_at: Some(now),
        metadata: None,
        org_id: key.org_id.clone(),
        app_id: key.app_id.clone(),
    };

    // Don't follow redirects: a public webhook host must not be able to 3xx the
    // request into an internal target.
    let client = crate::util::ssrf::no_redirect_client();
    let result = crate::proxy::webhook::dispatch_webhook_once(&client, &webhook_url, None, &synthetic).await;
    log_audit(state.db.clone(), key_ctx.as_ref(), "key.test_webhook", "api_key", &id.to_string(), None, None);
    match result {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let delivered = resp.status().is_success();
            (StatusCode::OK, Json(WebhookTestResult {
                delivered,
                status_code: Some(status),
                error: None,
            })).into_response()
        }
        Err(e) => {
            (StatusCode::OK, Json(WebhookTestResult {
                delivered: false,
                status_code: None,
                error: Some(e.to_string()),
            })).into_response()
        }
    }
}

// ─── Logs with Filters ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LogsQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub status: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub format: Option<String>,
}

/// GET /v1/logs — Paginated + filtered request logs. Supports `?format=csv`.
#[utoipa::path(
    get,
    path = "/v1/logs",
    params(
        ("page" = Option<u32>, Query, description = "Page number"),
        ("per_page" = Option<u32>, Query, description = "Items per page"),
    ),
    responses(
        (status = 200, description = "Request logs", body = PaginatedResponse<RequestLog>),
    ),
    tag = "Dashboard"
)]
pub async fn get_logs_filtered(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Query(query): Query<LogsQuery>,
) -> impl IntoResponse {
    use crate::auth::permissions::Permission;
    // Session users with only KeyReadOwn (no KeyReadAny) see an empty result for now,
    // since log rows are not user-scoped. For simplicity, session-users without
    // KeyReadAny get an empty list (logs are tied to provider, not user-owned keys yet).
    if let Some(ref ctx) = key_ctx {
        if ctx.user.is_some() && !ctx.permissions.contains(&Permission::KeyReadAny) {
            if !ctx.permissions.contains(&Permission::KeyReadOwn) {
                return (StatusCode::FORBIDDEN, Json(error_response("key:read:own permission required", 403))).into_response();
            }
            // KeyReadOwn only — return empty (logs are not filtered by key owner yet)
            let response = crate::types::PaginatedResponse {
                data: Vec::<crate::types::RequestLog>::new(),
                total: 0u64,
                page: query.page.unwrap_or(1),
                per_page: query.per_page.unwrap_or(50),
                total_pages: 0u32,
            };
            return (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
        }
    }
    // Tenant scope: require an active org; logs are restricted to org (+ app when set).
    let org_id = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    let app_id = key_ctx.as_ref().and_then(|c| c.app_id.as_deref());

    let is_csv = query.format.as_deref() == Some("csv");
    let page = crate::api::pagination::clamp_page(query.page.unwrap_or(1));
    let per_page = if is_csv {
        10000
    } else {
        crate::api::pagination::clamp_per_page(query.per_page.unwrap_or(crate::api::pagination::DEFAULT_PER_PAGE))
    };

    // Push the optional model/provider/status/date filters + pagination + count
    // down to the DB so a filtered or deep-page query returns the true matching
    // set and total — not a slice of a truncated in-memory window. Unparseable
    // date bounds are ignored (as the old lexicographic compare effectively did).
    let filter = crate::db::RequestLogFilter {
        model: query.model.as_deref(),
        provider: query.provider.as_deref(),
        status: query.status.as_deref(),
        from: query.from.as_deref().and_then(parse_rfc3339_utc),
        to: query.to.as_deref().and_then(parse_rfc3339_utc),
    };
    let (rows, total) = match state.db
        .get_request_logs_for_tenant_filtered(org_id, app_id, &filter, page, per_page)
        .await
    {
        Ok(pair) => pair,
        Err(e) => {
            error!(error = %e, "Failed to get tenant logs");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };

    if is_csv {
        // CSV export takes the (DB-filtered) first `per_page` (10k) rows.
        let date = chrono::Utc::now().format("%Y%m%d").to_string();
        let filename = format!("logs-{}.csv", date);
        return csv_response(logs_to_csv(&rows), &filename);
    }

    let total_pages = crate::api::pagination::total_pages(total, per_page);
    let response = PaginatedResponse {
        data: rows,
        total,
        page,
        per_page,
        total_pages,
    };
    (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response()
}

/// Parse an RFC-3339 timestamp into UTC, returning `None` on failure so an
/// unparseable filter bound is simply ignored rather than 400ing the request.
fn parse_rfc3339_utc(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

// ─── Webhook Delivery Log ────────────────────────────────────────────────────

/// GET /v1/keys/{id}/webhook-deliveries — List webhook deliveries for a key (admin scope).
/// Live: `curl https://app.litegen.ai/api/v1/keys/{id}/webhook-deliveries -H "Authorization: Bearer sk_live_..."`
#[utoipa::path(
    get,
    path = "/v1/keys/{id}/webhook-deliveries",
    params(
        ("id" = Uuid, Path, description = "API key ID"),
        ("page" = Option<u32>, Query, description = "Page number (default 1)"),
        ("per_page" = Option<u32>, Query, description = "Items per page (default 50)"),
    ),
    responses(
        (status = 200, description = "Paginated webhook deliveries", body = PaginatedResponse<crate::types::WebhookDelivery>),
        (status = 404, description = "Key not found", body = ErrorResponse),
    ),
    tag = "Admin"
)]
pub async fn list_webhook_deliveries(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<Uuid>,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    // Tenant scope: require an org; a key in another org (or absent) → 404.
    let ctx_org = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    let key = match state.db.get_api_key(&id).await {
        Ok(Some(k)) if k.org_id.as_deref() == Some(ctx_org) => k,
        Ok(_) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response(),
    };
    // Delivery history includes payloads/responses — gate reads like get_api_key.
    if let Err(resp) = authorize_key_for_session(
        key_ctx.as_ref(), &key,
        crate::auth::permissions::Permission::KeyReadAny,
        crate::auth::permissions::Permission::KeyReadOwn,
        "key:read:own",
    ) {
        return resp;
    }
    let key_id_str = id.to_string();
    let page = crate::api::pagination::clamp_page(params.page);
    let per_page = crate::api::pagination::clamp_per_page(params.per_page);
    match state.db.list_webhook_deliveries(&key_id_str, page, per_page).await {
        Ok((deliveries, total)) => {
            let total_pages = crate::api::pagination::total_pages(total as u64, per_page);
            let response = PaginatedResponse {
                data: deliveries,
                total: total as u64,
                page,
                per_page,
                total_pages,
            };
            (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to list webhook deliveries");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response()
        }
    }
}

// ─── Audit Log ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AuditQuery {
    pub actor_key_id: Option<String>,
    pub action: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub format: Option<String>,
}

/// GET /v1/audit — List audit log entries (admin scope required). Supports `?format=csv`.
/// Live: `curl https://app.litegen.ai/api/v1/audit -H "Authorization: Bearer sk_live_..."`
#[utoipa::path(
    get,
    path = "/v1/audit",
    params(
        ("actor_key_id" = Option<String>, Query, description = "Filter by actor API key ID"),
        ("action" = Option<String>, Query, description = "Filter by action, e.g. key.create"),
        ("from" = Option<String>, Query, description = "ISO 8601 lower bound (inclusive)"),
        ("to" = Option<String>, Query, description = "ISO 8601 upper bound (inclusive)"),
        ("page" = Option<u32>, Query, description = "Page number (default 1)"),
        ("per_page" = Option<u32>, Query, description = "Items per page (default 50)"),
        ("format" = Option<String>, Query, description = "Set to `csv` for a CSV export"),
    ),
    responses(
        (status = 200, description = "Paginated audit log entries (or a CSV attachment when format=csv)", body = PaginatedResponse<crate::types::AuditLogEntry>),
        (status = 403, description = "Forbidden (audit:read required)", body = ErrorResponse),
    ),
    tag = "Admin"
)]
pub async fn list_audit(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Query(query): Query<AuditQuery>,
) -> impl IntoResponse {
    use crate::auth::permissions::Permission;
    // Session users need AuditRead; Bearer/master key passes through (existing admin scope check)
    if let Some(ref ctx) = key_ctx {
        if ctx.user.is_some() && !ctx.permissions.contains(&Permission::AuditRead) {
            return (StatusCode::FORBIDDEN, Json(error_response("audit:read permission required", 403))).into_response();
        }
    }
    // Tenant scope: require an active org; the audit log is restricted to it.
    let org_id = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    let is_csv = query.format.as_deref() == Some("csv");
    let page = crate::api::pagination::clamp_page(query.page.unwrap_or(1));
    let per_page = if is_csv {
        10000
    } else {
        crate::api::pagination::clamp_per_page(query.per_page.unwrap_or(crate::api::pagination::DEFAULT_PER_PAGE))
    };

    // Push the optional actor/action/date filters + pagination + count down to
    // the DB (true matching set + total, not a slice of a truncated window).
    let filter = crate::db::TenantAuditFilter {
        actor_key_id: query.actor_key_id.as_deref(),
        action: query.action.as_deref(),
        from: query.from.as_deref().and_then(parse_rfc3339_utc),
        to: query.to.as_deref().and_then(parse_rfc3339_utc),
    };
    let (rows, total) = match state.db
        .list_audit_log_for_tenant_filtered(org_id, &filter, page, per_page)
        .await
    {
        Ok(pair) => pair,
        Err(e) => {
            error!(error = %e, "Failed to list audit log");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };

    if is_csv {
        let date = chrono::Utc::now().format("%Y%m%d").to_string();
        let filename = format!("audit-{}.csv", date);
        return csv_response(audit_to_csv(&rows), &filename);
    }
    let total_pages = crate::api::pagination::total_pages(total, per_page);
    let response = PaginatedResponse {
        data: rows,
        total,
        page,
        per_page,
        total_pages,
    };
    (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response()
}

// ─── Cache Management ───────────────────────────────────────────────────────

/// DELETE /v1/cache — Clear the generation cache.
#[utoipa::path(
    delete,
    path = "/v1/cache",
    responses(
        (status = 200, description = "Cache cleared", body = CacheClearedResponse),
        (status = 403, description = "Forbidden (tenant principals cannot flush the global cache)", body = ErrorResponse),
    ),
    tag = "Admin"
)]
pub async fn clear_cache(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
) -> impl IntoResponse {
    // The cache is a single PROCESS-GLOBAL instance shared across all tenants, so
    // a full flush evicts every tenant's entries. In hosted mode that makes it a
    // platform-admin operation: a tenant principal (session user OR bearer key)
    // must not be able to wipe other tenants' cached generations. In single-tenant
    // mode there is only one tenant, so the per-role cache:clear gate suffices.
    // (A scoped per-tenant clear is a follow-up; keys are hashed so a prefix sweep
    // isn't available today.)
    if let Some(ctx) = key_ctx.as_ref() {
        match state.mode {
            crate::config::Mode::Hosted => {
                if ctx.user.is_some() || ctx.key_id.is_some() {
                    return forbidden_perm_resp("cache:clear");
                }
            }
            crate::config::Mode::SingleTenant => {
                if ctx.user.is_some()
                    && !ctx.permissions.contains(&crate::auth::permissions::Permission::CacheClear)
                {
                    return forbidden_perm_resp("cache:clear");
                }
            }
        }
    }
    state.router.cache.clear().await;
    Json(CacheClearedResponse { cleared: true }).into_response()
}

// ─── OpenAPI Specification ──────────────────────────────────────────────────

/// GET /openapi.json — Serve the OpenAPI specification.
pub async fn openapi_spec() -> impl IntoResponse {
    use utoipa::OpenApi;
    Json(crate::api::openapi::ApiDoc::openapi())
}

// ─── Router Setup ───────────────────────────────────────────────────────────

pub fn create_router(state: Arc<AppState>) -> axum::Router {
    use axum::routing::{delete, get, patch, post};
    use axum::middleware;
    use super::metrics::metrics_handler;
    use super::middleware::{auth_middleware, check_scope, csrf_middleware, Scope};

    let auth_state = state.clone();
    let csrf_state = state.clone();

    // Generate routes — require Scope::Generate
    let generate_routes = axum::Router::new()
        .route("/v1/images/generations", post(generate_image))
        .route("/v1/images/cost", post(estimate_image_cost))
        .route("/v1/videos/generations", post(generate_video))
        .route("/v1/videos/cost", post(estimate_video_cost))
        .route("/v1/videos/{id}", get(get_video_status))
        .route("/v1/models3d/generations", post(generate_3d))
        .route("/v1/models3d/cost", post(estimate_3d_cost))
        // Registered after the two literal-segment routes above so they win the
        // match; axum/matchit gives static segments precedence over a capture
        // regardless of declaration order, but the ordering is kept anyway for
        // readability. The only theoretical collision is a generation id
        // literally equal to "assets" — unreachable, since ids are minted
        // `litegen-3d-{uuid}`.
        .route("/v1/models3d/{id}", get(get_3d_status))
        .layer(middleware::from_fn(|req: axum::extract::Request, next: middleware::Next| async {
            check_scope(Scope::Generate, req, next).await
        }))
        // CSRF: no-ops for safe methods and Bearer/API-key auth; only enforces
        // on mutating cookie-session requests. Same layer as auth_required_routes.
        .layer(middleware::from_fn_with_state(
            csrf_state.clone(),
            csrf_middleware,
        ));

    // Read routes — require Scope::Read
    let read_routes = axum::Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/models/{*id}", get(get_model_schema))
        .route("/v1/providers", get(list_providers))
        .route("/health", get(health_check))
        .route("/v1/stats", get(get_stats))
        .route("/v1/logs", get(get_logs_filtered))
        .route("/v1/logs/{id}/artifact", get(get_log_artifact))
        .route("/v1/generations", get(list_generations))
        .route("/v1/generations/{id}", get(get_generation))
        .route("/v1/generations/{id}", patch(cancel_generation))
        .layer(middleware::from_fn(|req: axum::extract::Request, next: middleware::Next| async {
            check_scope(Scope::Read, req, next).await
        }))
        // CSRF: no-ops for safe methods and Bearer/API-key auth; only enforces
        // on mutating cookie-session requests (e.g. PATCH cancel_generation).
        .layer(middleware::from_fn_with_state(
            csrf_state.clone(),
            csrf_middleware,
        ));

    // Admin routes — require Scope::Admin
    let admin_routes = axum::Router::new()
        .route("/v1/keys", post(create_api_key))
        .route("/v1/keys", get(list_api_keys))
        .route("/v1/keys/{id}", delete(revoke_api_key))
        .route("/v1/keys/{id}", get(get_api_key_handler))
        .route("/v1/keys/{id}", patch(patch_api_key_handler))
        .route("/v1/keys/{id}/rotate", post(rotate_api_key))
        .route("/v1/keys/{id}/test-webhook", post(test_webhook))
        .route("/v1/keys/{id}/webhook-deliveries", get(list_webhook_deliveries))
        .route("/v1/cache", delete(clear_cache))
        .route("/v1/audit", get(list_audit))
        .layer(middleware::from_fn(|req: axum::extract::Request, next: middleware::Next| async {
            check_scope(Scope::Admin, req, next).await
        }))
        // CSRF: no-ops for safe methods and Bearer/API-key auth; only enforces
        // on mutating cookie-session requests (POST/DELETE/PATCH /v1/keys,
        // DELETE /v1/cache, etc.). Same layer as auth_required_routes.
        .layer(middleware::from_fn_with_state(
            csrf_state.clone(),
            csrf_middleware,
        ));

    // Auth-required routes (auth middleware inserts KeyContext)
    let auth_required_routes = axum::Router::new()
        .route("/v1/auth/logout", post(logout))
        .route("/v1/auth/me", get(me))
        .route("/v1/auth/csrf", get(csrf_token))
        // User management. The GLOBAL (non-org-scoped) endpoints — list_users,
        // patch_user, delete_user, transfer_owner — are restricted to the
        // platform admin (master key) in hosted mode (see
        // authorize_global_user_admin); tenants manage members via
        // /v1/orgs/{id}/members. invite_user is org-scoped (uses ctx.org_id).
        .route("/v1/users", get(list_users).post(invite_user))
        .route("/v1/users/transfer-owner", post(transfer_owner))
        .route("/v1/users/{id}", patch(patch_user).delete(delete_user))
        // Account self-service
        .route("/v1/account", get(get_account).patch(patch_account))
        .route("/v1/account/sessions", get(list_sessions))
        .route("/v1/account/sessions/{id}", delete(revoke_session))
        // ─── Organizations / Applications / Members / Provider credentials ──
        // Session-authed; each handler authorizes against the PATH org via the
        // caller's membership role (not the active-org header).
        .route("/v1/orgs", get(orgs::list_orgs).post(orgs::create_org))
        .route("/v1/orgs/{id}", get(orgs::get_org).patch(orgs::patch_org).delete(orgs::delete_org))
        .route("/v1/orgs/{id}/members", get(orgs::list_members).post(orgs::invite_member))
        .route("/v1/orgs/{id}/members/{user_id}", patch(orgs::patch_member).delete(orgs::remove_member))
        .route("/v1/orgs/{id}/transfer-owner", post(orgs::transfer_owner))
        .route("/v1/orgs/{id}/apps", get(orgs::list_apps).post(orgs::create_app))
        .route("/v1/apps/{app_id}", get(orgs::get_app).patch(orgs::patch_app).delete(orgs::delete_app))
        .route(
            "/v1/orgs/{org_id}/provider-credentials",
            get(orgs::list_org_provider_credentials).post(orgs::create_org_provider_credential),
        )
        .route(
            "/v1/orgs/{org_id}/provider-credentials/{provider}",
            delete(orgs::delete_org_provider_credential),
        )
        .route(
            "/v1/apps/{app_id}/storage",
            get(orgs::get_app_storage)
                .put(orgs::put_app_storage)
                .delete(orgs::delete_app_storage),
        )
        .route(
            "/v1/orgs/{id}/allowed-models",
            get(orgs::get_org_allowed_models).put(orgs::put_org_allowed_models),
        )
        .route(
            "/v1/apps/{app_id}/allowed-models",
            get(orgs::get_app_allowed_models).put(orgs::put_app_allowed_models),
        )
        .layer(middleware::from_fn_with_state(
            csrf_state.clone(),
            csrf_middleware,
        ));

    // Unauthenticated auth routes
    let unauth_routes = axum::Router::new()
        .route("/v1/auth/config", get(auth_config))
        .route("/v1/auth/signup", post(signup))
        .route("/v1/auth/login", post(login))
        .route("/v1/auth/password-reset/request", post(password_reset_request))
        .route("/v1/auth/password-reset/confirm", post(password_reset_confirm))
        // Invitation lookup + accept (no auth needed)
        .route("/v1/auth/invitations/{token}", get(get_invitation))
        .route("/v1/auth/invitations/{token}/accept", post(accept_invitation))
        // OAuth routes — no auth middleware, no CSRF
        .route("/v1/auth/oauth/github/start", get(github_start))
        .route("/v1/auth/oauth/github/callback", get(github_callback))
        .route("/v1/auth/oauth/google/start", get(google_start))
        .route("/v1/auth/oauth/google/callback", get(google_callback))
        // Unified callback for both providers (single deployed redirect URI).
        // nginx strips `/api`, so `…/api/auth/redirect` arrives here as `/auth/redirect`.
        .route("/auth/redirect", get(oauth_redirect));

    // Auth middleware only wraps routes that need authentication/authorization context.
    // Unauthenticated routes (signup, login, invitations, oauth, health probes) are kept separate.
    let auth_layer = middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
        let s = auth_state.clone();
        async move {
            let headers = req.headers().clone();
            auth_middleware(headers, s, req, next).await
        }
    });

    let authenticated_routes = axum::Router::new()
        .merge(generate_routes)
        .merge(read_routes)
        .merge(admin_routes)
        .merge(auth_required_routes)
        .layer(auth_layer);

    axum::Router::new()
        .merge(authenticated_routes)
        // Unauth routes — no auth middleware needed, no CSRF
        .merge(unauth_routes)
        // Health probes, metrics, openapi, and mock video bytes don't require auth
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness))
        .route("/metrics", get(metrics_handler))
        .route("/openapi.json", get(openapi_spec))
        .route("/mock/video/{id}", get(get_mock_video_bytes))
        // Wildcard capture: 3D asset keys contain slashes (litegen/3d/{gen}/{name}.ext).
        // `/v1/models3d/generations`, `/v1/models3d/cost`, and `/v1/models3d/{id}`
        // (see `generate_routes` above) are merged into this same route table;
        // axum/matchit gives the static "assets" segment precedence over the
        // single-segment `{id}` capture, so these don't collide (the only
        // theoretical collision — a generation id literally equal to "assets" —
        // is unreachable, since ids are minted `litegen-3d-{uuid}`).
        .route("/v1/models3d/assets/{*key}", get(get_model3d_asset_bytes))
        .with_state(state)
}

// ─── Artifact endpoint ──────────────────────────────────────────────────────

/// GET /v1/logs/{id}/artifact — Retrieve the stored artifact for a request log.
/// Live: `curl https://app.litegen.ai/api/v1/logs/{id}/artifact -H "Authorization: Bearer sk_live_..."`
#[utoipa::path(
    get,
    path = "/v1/logs/{id}/artifact",
    params(("id" = String, Path, description = "Request log / generation ID")),
    responses(
        (status = 200, description = "Stored request/response artifact", body = crate::types::RequestArtifact),
        (status = 404, description = "Artifact not found", body = ErrorResponse),
    ),
    tag = "Dashboard"
)]
pub async fn get_log_artifact(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Tenant scope: require an active org.
    let ctx_org = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    match state.db.get_request_artifact(&id).await {
        Ok(Some(artifact)) => {
            // Cross-tenant isolation, fail-closed: the artifact must explicitly
            // belong to the caller's org. A missing org_id is NOT treated as
            // "visible to everyone" (which would leak any un-scoped artifact);
            // return 404 (same as not-found — don't reveal existence).
            if artifact.org_id.as_deref() != Some(ctx_org) {
                return (StatusCode::NOT_FOUND, Json(error_response("Artifact not found", 404))).into_response();
            }
            (StatusCode::OK, Json(serde_json::to_value(artifact).unwrap())).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(error_response("Artifact not found", 404))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to get request artifact");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response()
        }
    }
}

// ─── Mock video bytes route ──────────────────────────────────────────────────

/// GET /mock/video/{id} — Serve animated GIF produced by mock/visual-video-gen.
/// No auth required (mock-only endpoint, same as /metrics).
pub async fn get_mock_video_bytes(
    Path(id): Path<String>,
) -> impl IntoResponse {
    use crate::providers::video::visual_mock::global_store;

    match global_store().get(&id).await {
        Some(bytes) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "image/gif")],
            bytes,
        ).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

// ─── 3D asset bytes route ───────────────────────────────────────────────────

/// GET /v1/models3d/assets/{key} — Serve a 3D asset held by the local
/// (no-S3) store. No auth: these are the same unguessable, UUID-keyed URLs
/// handed to clients in the response, and S3 deployments never reach here.
pub async fn get_model3d_asset_bytes(Path(key): Path<String>) -> impl IntoResponse {
    match crate::proxy::storage::local_asset_bytes(&key).await {
        Some((bytes, content_type)) => (
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, content_type),
                // Meshes are immutable once written and keyed by generation id.
                (axum::http::header::CACHE_CONTROL, "public, max-age=31536000, immutable".to_string()),
            ],
            bytes,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Build a small summary of reference images for artifact storage.
/// Intentionally omits actual bytes — just role, kind, and a size hint.
fn build_refs_meta(refs: &[crate::types::ReferenceImage]) -> Option<serde_json::Value> {
    if refs.is_empty() {
        return None;
    }
    let summaries: Vec<serde_json::Value> = refs.iter().map(|r| {
        let source_summary = match r.kind {
            crate::types::RefImageKind::Url => format!("url {}", r.value.chars().take(80).collect::<String>()),
            crate::types::RefImageKind::Base64 => format!("blob {}KB", r.value.len() / 1024),
            crate::types::RefImageKind::Blob => format!("field {}", r.value.chars().take(40).collect::<String>()),
        };
        serde_json::json!({
            "role": r.role,
            "kind": format!("{:?}", r.kind).to_lowercase(),
            "source_summary": source_summary,
        })
    }).collect();
    Some(serde_json::json!(summaries))
}

fn validation_rejection_response(message: &str, code: u16, model: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    let body = serde_json::json!({
        "error": {
            "type": "validation_error",
            "message": message,
            "code": code,
            "model": model,
        }
    });
    (StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST), Json(body)).into_response()
}

// ─── Audit log helper ────────────────────────────────────────────────────────

/// Fire-and-forget audit log insert.  The write is spawned so it doesn't
/// block the response.  Any DB error is only logged, never surfaced.
fn log_audit(
    db: Arc<dyn crate::db::DatabaseStore>,
    key_ctx: Option<&KeyContext>,
    action: &str,
    target_type: &str,
    target_id: &str,
    before_json: Option<serde_json::Value>,
    after_json: Option<serde_json::Value>,
) {
    let actor_key_id = key_ctx.and_then(|c| c.key_id).map(|id| id.to_string());
    let actor_label = key_ctx
        .and_then(|c| c.key_id)
        .map(|id| id.to_string())
        .unwrap_or_else(|| "master-key".to_string());
    let action = action.to_string();
    let target_type = target_type.to_string();
    let target_id = target_id.to_string();
    let before = before_json.map(|v| v.to_string());
    let after = after_json.map(|v| v.to_string());
    let org_id = key_ctx.and_then(|c| c.org_id.clone());

    tokio::spawn(async move {
        let entry = crate::types::AuditLogEntry {
            id: format!("audit-{}", Uuid::new_v4()),
            actor_key_id,
            actor_label,
            action,
            target_type,
            target_id,
            before_json: before,
            after_json: after,
            created_at: chrono::Utc::now(),
            org_id,
        };
        if let Err(e) = db.insert_audit_log(&entry).await {
            tracing::warn!(error = %e, "Failed to insert audit log entry");
        }
    });
}

/// 403 used by tenant-data routes when the caller has no active organization
/// (e.g. a hosted master/platform context where `ctx.org_id == None`).
fn forbidden_no_org() -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(error_response("no active organization", 403)),
    ).into_response()
}

/// 402 returned when a pre-dispatch quota reservation would exceed the key's cap.
fn quota_exceeded_response() -> axum::response::Response {
    (
        StatusCode::PAYMENT_REQUIRED,
        Json(serde_json::json!({
            "error": { "message": "quota exceeded", "type": "quota_exceeded", "code": 402 }
        })),
    ).into_response()
}

/// Atomically reserve `estimate` of quota against `key_id` BEFORE dispatching a
/// billable generation. This — not the stale pre-flight read in the auth
/// middleware — is what actually caps spend: a single oversized request or a
/// burst of concurrent requests cannot slip past because the check-and-increment
/// is one atomic UPDATE. Returns the reserved amount (0.0 when there is no DB key
/// or nothing to charge), or an `Err(response)` (402 over-quota, 503 on DB error)
/// the caller must return without dispatching.
async fn reserve_quota(
    state: &AppState,
    key_id: Option<uuid::Uuid>,
    estimate: f64,
) -> Result<f64, axum::response::Response> {
    let Some(key_id) = key_id else { return Ok(0.0) };
    if estimate <= 0.0 {
        return Ok(0.0);
    }
    match state.db.reserve_tokens(&key_id, estimate).await {
        Ok(true) => Ok(estimate),
        Ok(false) => Err(quota_exceeded_response()),
        Err(e) => {
            tracing::error!(error = %e, key_id = %key_id, "quota reservation failed");
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                [("retry-after", "1")],
                Json(error_response("temporarily unable to authorize request", 503)),
            ).into_response())
        }
    }
}

/// Settle a prior [`reserve_quota`] hold once the generation resolves: adjust the
/// held `reserved` amount to the `actual` cost. A negative delta refunds the
/// difference; `actual == 0.0` releases the whole hold (used when generation
/// fails so a failed request is never billed).
async fn settle_quota(state: &AppState, key_id: Option<uuid::Uuid>, reserved: f64, actual: f64) {
    let Some(key_id) = key_id else { return };
    let delta = actual - reserved;
    if delta.abs() > f64::EPSILON {
        if let Err(e) = state.db.atomic_charge_tokens(&key_id, delta).await {
            tracing::warn!(error = %e, key_id = %key_id, "quota settlement failed");
        }
    }
}

fn forbidden_perm_resp(perm: &str) -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(error_response(&format!("Permission '{perm}' required"), 403)),
    ).into_response()
}

/// Enforce a key-scoped permission boundary for SESSION (cookie) users on a
/// single key. Bearer/master principals (`ctx.user` is None) are not
/// permission-gated here — they are scope+org gated upstream — matching the
/// pattern in `patch_api_key_handler`/`revoke_api_key`. `any_label` is the
/// permission string used in the 403 message.
fn authorize_key_for_session(
    key_ctx: Option<&KeyContext>,
    key: &crate::types::ApiKey,
    any_perm: crate::auth::permissions::Permission,
    own_perm: crate::auth::permissions::Permission,
    any_label: &str,
) -> Result<(), axum::response::Response> {
    let Some(ctx) = key_ctx else { return Ok(()) };
    let Some(user) = ctx.user.as_ref() else { return Ok(()) };
    if ctx.permissions.contains(&any_perm) {
        return Ok(());
    }
    if ctx.permissions.contains(&own_perm)
        && key.owner_user_id.as_deref() == Some(user.user_id.as_str())
    {
        return Ok(());
    }
    Err(forbidden_perm_resp(any_label))
}

/// Enforce a generation-scoped permission boundary for SESSION users on a single
/// generation. `read:own`/`cancel:own` is satisfied only when the generation's
/// key is owned by the caller (mirrors `list_generations`). Bearer/master
/// principals are not gated here.
async fn authorize_generation_for_session(
    state: &AppState,
    key_ctx: Option<&KeyContext>,
    gen: &crate::types::Generation,
    any_perm: crate::auth::permissions::Permission,
    own_perm: crate::auth::permissions::Permission,
    any_label: &str,
) -> Result<(), axum::response::Response> {
    let Some(ctx) = key_ctx else { return Ok(()) };
    let Some(user) = ctx.user.as_ref() else { return Ok(()) };
    if ctx.permissions.contains(&any_perm) {
        return Ok(());
    }
    if ctx.permissions.contains(&own_perm) {
        // Ownership model: API keys have owners, dashboard sessions don't. A
        // generation with no owning key (`key_id == None`) was created via a
        // session and belongs to the org as a whole — the org-scope check the
        // caller already passed is its boundary, so any session member with
        // `:own` may access it. A key-created generation must be owned by one of
        // the caller's keys.
        let owns = match gen.key_id {
            None => true,
            Some(kid) => match state.db.list_api_keys_for_owner(&user.user_id).await {
                Ok(owned) => owned.iter().any(|k| k.id == kid),
                // Don't fail-open, but don't masquerade a DB outage as a
                // permission denial either — surface a 500 so it's retryable and
                // diagnosable rather than a misleading 403 on an owned resource.
                Err(e) => {
                    error!(error = %e, "ownership check failed: list_api_keys_for_owner");
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(error_response(&e.to_string(), 500)),
                    )
                        .into_response());
                }
            },
        };
        if owns {
            return Ok(());
        }
    }
    Err(forbidden_perm_resp(any_label))
}

/// Cache scope for the process-global generation cache. Distinct principals must
/// never collide: prefer org+app, fall back to org, and when no tenant context
/// is resolved (e.g. the hosted master key or a session with no active org) scope
/// to the principal itself rather than a shared global bucket — otherwise one
/// caller could receive another's cached image for an identical request.
fn cache_scope(ctx: &KeyContext) -> String {
    match (ctx.org_id.as_deref(), ctx.app_id.as_deref()) {
        (Some(org), Some(app)) => format!("o:{org}/a:{app}"),
        (Some(org), None) => format!("o:{org}"),
        (None, _) => {
            if let Some(kid) = ctx.key_id {
                format!("k:{kid}")
            } else if let Some(u) = ctx.user.as_ref() {
                format!("u:{}", u.user_id)
            } else {
                "master".to_string()
            }
        }
    }
}

#[cfg(test)]
mod cache_scope_tests {
    use super::*;
    use crate::api::middleware::UserContext;
    use crate::types::Role;

    fn ctx(key_id: Option<uuid::Uuid>, user: Option<&str>, org: Option<&str>, app: Option<&str>) -> KeyContext {
        KeyContext {
            key_id,
            scopes: vec![],
            rpm_limit: None,
            quota_remaining: None,
            webhook_url: None,
            user: user.map(|u| UserContext { user_id: u.to_string(), email: "e@example.com".into(), role: Role::Member }),
            permissions: vec![],
            session_id: None,
            org_id: org.map(String::from),
            app_id: app.map(String::from),
        }
    }

    #[test]
    fn distinct_principals_never_share_a_bucket() {
        let kid = uuid::Uuid::new_v4();
        let scopes = [
            cache_scope(&ctx(None, Some("u1"), Some("orgA"), Some("app1"))),
            cache_scope(&ctx(None, Some("u1"), Some("orgA"), None)),       // org-scoped ≠ app-scoped
            cache_scope(&ctx(None, Some("u2"), Some("orgB"), Some("app1"))), // different org
            cache_scope(&ctx(Some(kid), None, None, None)),                // bearer key, no tenant
            cache_scope(&ctx(None, Some("u3"), None, None)),               // session, no org
            cache_scope(&ctx(None, None, None, None)),                     // master key
        ];
        let unique: std::collections::HashSet<_> = scopes.iter().collect();
        assert_eq!(unique.len(), scopes.len(), "cache scopes collided: {scopes:?}");
    }

    #[test]
    fn same_tenant_is_stable_and_distinct_from_global() {
        // Two members of the same org+app share a bucket (cache hits per tenant)...
        assert_eq!(
            cache_scope(&ctx(None, Some("u1"), Some("orgA"), Some("app1"))),
            cache_scope(&ctx(None, Some("u2"), Some("orgA"), Some("app1"))),
        );
        // ...but a real tenant never collides with the master/global bucket.
        assert_ne!(
            cache_scope(&ctx(None, Some("u1"), Some("orgA"), None)),
            cache_scope(&ctx(None, None, None, None)),
        );
    }
}

fn error_response(message: &str, code: u16) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "message": message,
            "type": match code {
                400 => "invalid_request_error",
                401 => "authentication_error",
                403 => "forbidden",
                404 => "not_found_error",
                424 => "provider_not_configured",
                429 => "rate_limit_error",
                502 => "provider_error",
                503 => "service_unavailable",
                _ => "internal_error",
            },
            "code": code
        }
    })
}

/// 400 body for a model whose provider has neither a per-app BYO credential nor a
/// registered platform default. Distinct from `error_response(400, ..)` because the
/// `type` is `provider_not_configured` (not `invalid_request_error`), matching the
/// documented BYO contract the dashboard/SDK keys off of.
fn provider_not_configured_response(provider: &str) -> axum::response::Response {
    let body = serde_json::json!({
        "error": {
            "type": "provider_not_configured",
            "code": 400,
            "message": format!(
                "Provider '{provider}' is not configured for this application. \
                 Add a provider credential in the dashboard."
            ),
        }
    });
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

/// Resolve the calling org's stored credential for `provider`, decrypted.
///
/// - `Ok(Some(creds))` — the org has a stored BYO credential (use it per-request).
/// - `Ok(None)` — the org has none (the caller should fall back to the platform
///   default, or surface `provider_not_configured` if there is none).
/// - `Err(response)` — a server-side error (no/invalid secrets key, corrupt or
///   un-decryptable stored credential, DB failure). These are 500s, never 400s:
///   the request was well-formed; the server is misconfigured.
async fn resolve_org_provider_credential(
    state: &AppState,
    key_ctx: &Option<KeyContext>,
    provider: &str,
) -> Result<Option<crate::providers::ProviderCredentials>, axum::response::Response> {
    let org_id = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return Ok(None),
    };
    let secrets_key = match state.secrets_key {
        Some(k) => k,
        None => return Ok(None),
    };
    match state.db.get_org_provider_credential(org_id, provider).await {
        Ok(Some((ct, nonce))) => {
            let plaintext = crate::auth::secrets::decrypt(&secrets_key, &ct, &nonce).map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(error_response("failed to decrypt provider credential", 500)),
                )
                    .into_response()
            })?;
            let val: serde_json::Value = serde_json::from_slice(&plaintext).map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(error_response("stored provider credential is corrupt", 500)),
                )
                    .into_response()
            })?;
            Ok(Some(crate::providers::ProviderCredentials::from_json(&val)))
        }
        Ok(None) => Ok(None),
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_response("provider credential lookup failed", 500)),
        )
            .into_response()),
    }
}

/// Resolve the calling app's BYO image store, if configured & usable. Returns
/// `None` (→ caller uses the global store) on any miss or, for a configured-but-
/// broken row, after logging a warning (fail-open — never breaks a generation).
async fn resolve_app_image_store(
    state: &AppState,
    key_ctx: &Option<KeyContext>,
) -> Option<std::sync::Arc<dyn crate::proxy::storage::ImageStore>> {
    let app_id = key_ctx.as_ref().and_then(|c| c.app_id.as_deref())?;
    let secrets_key = state.secrets_key?;
    let row = match state.db.get_app_storage(app_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(app_id, error = %e, "byo storage: lookup failed, using global store");
            return None;
        }
    };

    #[derive(serde::Deserialize)]
    struct StorageSecret {
        access_key_id: String,
        secret_access_key: String,
    }

    let plaintext = match crate::auth::secrets::decrypt(&secrets_key, &row.secret_ciphertext, &row.secret_nonce) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(app_id, error = %e, "byo storage: decrypt failed, using global store");
            return None;
        }
    };
    let secret: StorageSecret = match serde_json::from_slice(&plaintext) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(app_id, error = %e, "byo storage: corrupt secret, using global store");
            return None;
        }
    };

    let cfg = crate::config::ImageStorageConfig {
        backend: row.backend.clone(),
        path_prefix: row.path_prefix.clone(),
        s3: Some(crate::config::S3StorageConfig {
            bucket_name: row.bucket_name.clone(),
            region: row.region.clone(),
            access_key_id: Some(secret.access_key_id),
            secret_access_key: Some(secret.secret_access_key),
            endpoint_url: row.endpoint_url.clone(),
            custom_public_url: row.custom_public_url.clone(),
        }),
    };
    match crate::proxy::storage::S3Store::from_config(&cfg) {
        Ok(store) => Some(std::sync::Arc::new(store) as std::sync::Arc<dyn crate::proxy::storage::ImageStore>),
        Err(e) => {
            tracing::warn!(app_id, error = %e, "byo storage: build failed, using global store");
            None
        }
    }
}

/// Resolve the calling app's BYO 3D asset store, if configured & usable.
///
/// Reads the SAME `app_storage_credentials` row as `resolve_app_image_store`,
/// but builds an `S3Storage` (key-explicit `put`) rather than an `S3Store`
/// (content-type→extension inference, which would save a `.glb` as `.png`).
/// Returns the store and the app's configured path prefix. Fails open to the
/// global store on any error — a bad BYO row must never break a generation.
///
/// Takes `db`/`secrets_key` directly rather than `&AppState`: the poller
/// (Task 11) is the only real caller and it has no `AppState` — it holds these
/// two fields on their own. A handler-side caller passes `&state.db,
/// state.secrets_key`.
pub(crate) async fn resolve_app_model3d_store(
    db: &std::sync::Arc<dyn crate::db::DatabaseStore>,
    secrets_key: Option<[u8; 32]>,
    app_id: &str,
) -> Option<(std::sync::Arc<dyn crate::proxy::storage::ImageStorage>, Option<String>)> {
    let secrets_key = secrets_key?;
    let row = match db.get_app_storage(app_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(app_id, error = %e, "byo 3d storage: lookup failed, using global store");
            return None;
        }
    };

    #[derive(serde::Deserialize)]
    struct StorageSecret { access_key_id: String, secret_access_key: String }

    let plaintext = crate::auth::secrets::decrypt(&secrets_key, &row.secret_ciphertext, &row.secret_nonce)
        .map_err(|e| tracing::warn!(app_id, error = %e, "byo 3d storage: decrypt failed"))
        .ok()?;
    let secret: StorageSecret = serde_json::from_slice(&plaintext)
        .map_err(|e| tracing::warn!(app_id, error = %e, "byo 3d storage: corrupt secret"))
        .ok()?;

    let cfg = crate::config::ImageStorageConfig {
        backend: row.backend.clone(),
        path_prefix: row.path_prefix.clone(),
        s3: Some(crate::config::S3StorageConfig {
            bucket_name: row.bucket_name.clone(),
            region: row.region.clone(),
            access_key_id: Some(secret.access_key_id),
            secret_access_key: Some(secret.secret_access_key),
            endpoint_url: row.endpoint_url.clone(),
            custom_public_url: row.custom_public_url.clone(),
        }),
    };
    match crate::proxy::storage::S3Storage::from_config(&cfg) {
        Ok(store) => Some((std::sync::Arc::new(store), row.path_prefix.clone())),
        Err(e) => {
            tracing::warn!(app_id, error = %e, "byo 3d storage: build failed, using global store");
            None
        }
    }
}

// ─── Key endpoint tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod key_endpoint_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use crate::db::sqlite::SqliteDatabase;
    use crate::capabilities::CapabilityRegistry;
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::LocalStore;
    use crate::proxy::materializer::{Materializer, MaterializeError, TempStorage};
    use crate::api::middleware::rate_limit::RateLimiter;
    use bytes::Bytes;

    struct NoopStorage;

    #[async_trait::async_trait]
    impl TempStorage for NoopStorage {
        async fn put(&self, key: &str, _bytes: Bytes, _ct: &str) -> Result<String, MaterializeError> {
            Ok(format!("local://{}", key))
        }
        async fn delete(&self, _key: &str) -> Result<(), MaterializeError> { Ok(()) }
    }

    async fn build_test_state() -> Arc<AppState> {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
        let registry = Arc::new(ProviderRegistry::new());
        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = Arc::new(ProxyRouter::new(registry, cache, config, image_store));
        let materializer = Arc::new(Materializer::new(
            Arc::new(NoopStorage), reqwest::Client::new(),
        ));
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); p.push("models");
        let cap_registry = Arc::new(
            CapabilityRegistry::from_dir(&p).expect("load shipped models")
        );
        Arc::new(AppState {
            router, db, master_key: None, registry: cap_registry, materializer,
            rate_limiter: Arc::new(RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    /// Build a minimal admin-only test router. Applies auth_middleware so the
    /// single-tenant default org/app is set in the KeyContext (required by the
    /// tenant-scoped key endpoints); master_key = None → all scopes allowed.
    fn build_keys_router(state: Arc<AppState>) -> axum::Router {
        use axum::routing::{delete, get, patch, post};
        use axum::middleware;
        use crate::api::middleware::auth_middleware;
        let auth_state = state.clone();
        axum::Router::new()
            .route("/v1/keys", post(create_api_key))
            .route("/v1/keys", get(list_api_keys))
            .route("/v1/keys/{id}", delete(revoke_api_key))
            .route("/v1/keys/{id}", get(get_api_key_handler))
            .route("/v1/keys/{id}", patch(patch_api_key_handler))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                let s = auth_state.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state)
    }

    #[tokio::test]
    async fn create_with_quota_and_get_by_id_roundtrip() {
        let state = build_test_state().await;
        let app = build_keys_router(state);

        // POST /v1/keys with quota fields
        let body = serde_json::to_vec(&serde_json::json!({
            "name": "test-key",
            "token_quota": 5.0,
            "rpm_limit": 10,
            "scopes": "generate"
        })).unwrap();

        let req = Request::builder()
            .method("POST").uri("/v1/keys")
            .header("content-type", "application/json")
            .body(Body::from(body)).unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED, "POST /v1/keys should return 201");

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(created["token_quota"], 5.0, "token_quota should be 5.0");
        assert_eq!(created["rpm_limit"], 10, "rpm_limit should be 10");
        assert_eq!(created["scopes"], "generate", "scopes should be 'generate'");

        // GET /v1/keys to extract the ID
        let req = Request::builder()
            .method("GET").uri("/v1/keys")
            .body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = list["data"][0]["id"].as_str().unwrap().to_string();

        // GET /v1/keys/{id}
        let req = Request::builder()
            .method("GET").uri(format!("/v1/keys/{}", id))
            .body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "GET /v1/keys/{{id}} should return 200");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let detail: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(detail["token_quota"], 5.0, "get-by-id should return token_quota");
        assert_eq!(detail["scopes"], "generate", "get-by-id should return scopes");
    }

    #[tokio::test]
    async fn patch_quota_updates_value() {
        let state = build_test_state().await;
        let app = build_keys_router(state);

        // Create key
        let body = serde_json::to_vec(&serde_json::json!({"name": "patch-test"})).unwrap();
        let req = Request::builder().method("POST").uri("/v1/keys")
            .header("content-type", "application/json").body(Body::from(body)).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Get the ID
        let req = Request::builder().method("GET").uri("/v1/keys").body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = list["data"][0]["id"].as_str().unwrap().to_string();

        // PATCH quota
        let patch_body = serde_json::to_vec(&serde_json::json!({"token_quota": 20.0})).unwrap();
        let req = Request::builder().method("PATCH").uri(format!("/v1/keys/{}", id))
            .header("content-type", "application/json").body(Body::from(patch_body)).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "PATCH should return 200");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let patched: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(patched["token_quota"], 20.0, "token_quota should be updated to 20.0");
    }

    #[tokio::test]
    async fn get_nonexistent_key_returns_404() {
        let state = build_test_state().await;
        let app = build_keys_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/keys/00000000-0000-0000-0000-000000000000")
            .body(Body::empty()).unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "GET nonexistent key should return 404");
    }
}

// ─── New endpoint tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod new_endpoint_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use crate::db::sqlite::SqliteDatabase;
    use crate::capabilities::CapabilityRegistry;
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::LocalStore;
    use crate::proxy::materializer::{Materializer, MaterializeError, TempStorage};
    use crate::api::middleware::rate_limit::RateLimiter;
    use bytes::Bytes;

    struct NoopStorage;
    #[async_trait::async_trait]
    impl TempStorage for NoopStorage {
        async fn put(&self, key: &str, _bytes: Bytes, _ct: &str) -> Result<String, MaterializeError> {
            Ok(format!("local://{}", key))
        }
        async fn delete(&self, _key: &str) -> Result<(), MaterializeError> { Ok(()) }
    }

    async fn build_state() -> Arc<AppState> {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
        let registry = Arc::new(ProviderRegistry::new());
        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = Arc::new(ProxyRouter::new(registry, cache, config, image_store));
        let materializer = Arc::new(Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); p.push("models");
        let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("models"));
        Arc::new(AppState {
            router, db, master_key: None, registry: cap_registry, materializer,
            rate_limiter: Arc::new(RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    fn build_test_router(state: Arc<AppState>) -> axum::Router {
        use axum::routing::{get, patch, post};
        use crate::api::middleware::auth_middleware;
        use axum::middleware;

        let auth_state = state.clone();
        axum::Router::new()
            .route("/v1/generations", get(list_generations))
            .route("/v1/generations/{id}", patch(cancel_generation))
            .route("/v1/keys", post(create_api_key))
            .route("/v1/keys/{id}/rotate", post(rotate_api_key))
            .route("/v1/keys/{id}/test-webhook", post(test_webhook))
            .route("/v1/logs", get(get_logs_filtered))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                let s = auth_state.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state)
    }

    // ── GET /v1/generations ─────────────────────────────────────────────────

    #[tokio::test]
    async fn list_generations_returns_owned_rows() {
        let state = build_state().await;
        state.db.insert_generation("lg-list-test-1", None, "mock/v", "mock", "video", None, 0.0, None, None).await.unwrap();

        let app = build_test_router(state);
        let req = Request::builder()
            .method("GET").uri("/v1/generations")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["total"], 1);
        assert_eq!(body["data"][0]["id"], "lg-list-test-1");
    }

    // ── PATCH /v1/generations/{id} cancel ──────────────────────────────────

    #[tokio::test]
    async fn cancel_generation_pending_returns_cancelled() {
        let state = build_state().await;
        state.db.insert_generation("lg-cancel-ep-1", None, "mock/v", "mock", "video", None, 0.0, None, None).await.unwrap();

        let app = build_test_router(state);
        let body = serde_json::to_vec(&serde_json::json!({"status": "cancelled"})).unwrap();
        let req = Request::builder()
            .method("PATCH").uri("/v1/generations/lg-cancel-ep-1")
            .header("content-type", "application/json")
            .body(Body::from(body)).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let gen: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(gen["status"], "cancelled");
    }

    #[tokio::test]
    async fn cancel_generation_completed_returns_409() {
        let state = build_state().await;
        state.db.insert_generation("lg-cancel-ep-2", None, "mock/v", "mock", "video", None, 0.0, None, None).await.unwrap();
        state.db.update_generation_status("lg-cancel-ep-2", "completed", 100, None, None, Some(chrono::Utc::now())).await.unwrap();

        let app = build_test_router(state);
        let body = serde_json::to_vec(&serde_json::json!({"status": "cancelled"})).unwrap();
        let req = Request::builder()
            .method("PATCH").uri("/v1/generations/lg-cancel-ep-2")
            .header("content-type", "application/json")
            .body(Body::from(body)).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    // ── POST /v1/keys/{id}/rotate ───────────────────────────────────────────

    #[tokio::test]
    async fn rotate_key_issues_new_secret_in_place() {
        let state = build_state().await;
        let create_body = serde_json::to_vec(&serde_json::json!({
            "name": "rotate-test", "scopes": "generate,read", "token_quota": 10.0
        })).unwrap();
        let app = build_test_router(state.clone());
        let req = Request::builder().method("POST").uri("/v1/keys")
            .header("content-type", "application/json").body(Body::from(create_body)).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let keys = state.db.list_api_keys().await.unwrap();
        let key_id = keys[0].id;
        let old_hash = keys[0].key_hash.clone();

        let req = Request::builder().method("POST")
            .uri(format!("/v1/keys/{}/rotate", key_id))
            .body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "rotate should return 200");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let rotated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        // (a) name/scopes/quota preserved; same row id; new sk_live_ secret returned
        assert_eq!(rotated["name"], "rotate-test");
        assert_eq!(rotated["scopes"], "generate,read");
        assert_eq!(rotated["token_quota"], 10.0);
        assert_eq!(rotated["id"].as_str().unwrap(), key_id.to_string());
        assert!(rotated["key"].as_str().unwrap().starts_with("sk_live_"));
        assert!(rotated["public_id"].as_str().unwrap().starts_with("pk_live_"));

        // (b) same id stays active, but the stored hash changed (secret rotated)
        let key = state.db.get_api_key(&key_id).await.unwrap().unwrap();
        assert!(key.is_active, "rotated key should stay active");
        assert_ne!(key.key_hash, old_hash, "key hash should change after rotate");
    }

    // ── POST /v1/keys/{id}/test-webhook ────────────────────────────────────

    #[tokio::test]
    async fn test_webhook_without_url_returns_400() {
        let state = build_state().await;
        let create_body = serde_json::to_vec(&serde_json::json!({"name": "no-hook"})).unwrap();
        let app = build_test_router(state.clone());
        let req = Request::builder().method("POST").uri("/v1/keys")
            .header("content-type", "application/json").body(Body::from(create_body)).unwrap();
        app.clone().oneshot(req).await.unwrap();
        let keys = state.db.list_api_keys().await.unwrap();
        let key_id = keys[0].id;

        let req = Request::builder().method("POST")
            .uri(format!("/v1/keys/{}/test-webhook", key_id))
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "no_webhook_configured");
    }

    #[tokio::test]
    async fn test_webhook_delivers_to_mock_server() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let state = build_state().await;
        let webhook_url = format!("{}/hook", server.uri());
        let create_body = serde_json::to_vec(&serde_json::json!({
            "name": "hook-key", "webhook_url": webhook_url
        })).unwrap();
        let app = build_test_router(state.clone());
        let req = Request::builder().method("POST").uri("/v1/keys")
            .header("content-type", "application/json").body(Body::from(create_body)).unwrap();
        app.clone().oneshot(req).await.unwrap();
        let keys = state.db.list_api_keys().await.unwrap();
        let key_id = keys[0].id;

        let req = Request::builder().method("POST")
            .uri(format!("/v1/keys/{}/test-webhook", key_id))
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["delivered"], true, "should be delivered");
        assert_eq!(result["status_code"], 200);
        server.verify().await;
    }

    #[tokio::test]
    async fn test_webhook_503_returns_delivered_false() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/hook503"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;

        let state = build_state().await;
        let webhook_url = format!("{}/hook503", server.uri());
        let create_body = serde_json::to_vec(&serde_json::json!({
            "name": "hook-503-key", "webhook_url": webhook_url
        })).unwrap();
        let app = build_test_router(state.clone());
        let req = Request::builder().method("POST").uri("/v1/keys")
            .header("content-type", "application/json").body(Body::from(create_body)).unwrap();
        app.clone().oneshot(req).await.unwrap();
        let keys = state.db.list_api_keys().await.unwrap();
        let key_id = keys[0].id;

        let req = Request::builder().method("POST")
            .uri(format!("/v1/keys/{}/test-webhook", key_id))
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["delivered"], false);
        assert_eq!(result["status_code"], 503);
        server.verify().await;
    }

    // ── GET /v1/logs?format=csv ─────────────────────────────────────────────

    #[tokio::test]
    async fn logs_csv_export_returns_csv_content_type() {
        let state = build_state().await;
        state.db.log_request("csv1", "mock/v", "mock", "completed", "image", 0.01, 100, None, None, None, None).await.unwrap();

        let app = build_test_router(state);
        let req = Request::builder()
            .method("GET").uri("/v1/logs?format=csv")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.starts_with("text/csv"), "content-type should be text/csv, got: {}", ct);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&bytes).unwrap();
        // Header row
        assert!(body.starts_with("id,"), "CSV should start with header row 'id,...', got: {}", &body[..body.len().min(50)]);
        // Data row contains our log
        assert!(body.contains("csv1"), "CSV should contain the log id");
    }

    // ── GET /v1/logs with filters ───────────────────────────────────────────

    #[tokio::test]
    async fn logs_filter_by_model_via_handler() {
        let state = build_state().await;
        state.db.log_request("log1", "mock/image-gen", "mock", "completed", "image", 0.0, 10, None, None, None, None).await.unwrap();
        state.db.log_request("log2", "openai/dall-e-3", "openai", "completed", "image", 0.01, 20, None, None, None, None).await.unwrap();
        state.db.log_request("log3", "mock/image-gen", "mock", "failed", "image", 0.0, 5, Some("err"), None, None, None).await.unwrap();

        let app = build_test_router(state);
        let req = Request::builder()
            .method("GET").uri("/v1/logs?model=mock%2Fimage-gen")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["total"], 2, "should return 2 mock logs");
        assert!(body["data"].as_array().unwrap().iter().all(|l| l["model"] == "mock/image-gen"));
    }

    /// Create a key via the API, then query the audit log for key.create action.
    #[tokio::test]
    async fn audit_log_records_key_create() {
        use axum::routing::{get, post};
        use axum::middleware;
        use crate::api::middleware::auth_middleware;

        let state = build_state().await;
        let auth_state = state.clone();
        let app = axum::Router::new()
            .route("/v1/keys", post(create_api_key))
            .route("/v1/audit", get(list_audit))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                let s = auth_state.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state);

        // POST /v1/keys
        let body = serde_json::to_vec(&serde_json::json!({
            "name": "audit-test-key",
            "scopes": "generate"
        })).unwrap();
        let req = Request::builder()
            .method("POST").uri("/v1/keys")
            .header("content-type", "application/json")
            .body(Body::from(body)).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED, "POST /v1/keys should return 201");

        // Give the audit log tokio::spawn a moment to complete.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // GET /v1/audit?action=key.create
        let req = Request::builder()
            .method("GET").uri("/v1/audit?action=key.create")
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["total"], 1, "should have 1 audit entry for key.create");
        assert_eq!(body["data"][0]["action"], "key.create");
        assert_eq!(body["data"][0]["actor_label"], "master-key");
    }
}

// ─── Task 17 permission-scoping tests ────────────────────────────────────────

#[cfg(test)]
mod permission_scoping_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::{middleware, routing::{delete, get}};
    use tower::ServiceExt;
    use crate::api::middleware::{auth_middleware, AppState};
    use crate::auth::tokens::{generate_csrf_token, generate_session_token};
    use crate::capabilities::CapabilityRegistry;
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::db::sqlite::SqliteDatabase;
    use crate::db::DatabaseStore;
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::materializer::{Materializer, MaterializeError, TempStorage};
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::LocalStore;
    use crate::types::{Role, Session, User};
    use bytes::Bytes;

    struct NoopStorage2;
    #[async_trait::async_trait]
    impl TempStorage for NoopStorage2 {
        async fn put(&self, key: &str, _b: Bytes, _ct: &str) -> Result<String, MaterializeError> {
            Ok(format!("local://{}", key))
        }
        async fn delete(&self, _k: &str) -> Result<(), MaterializeError> { Ok(()) }
    }

    async fn build_db() -> Arc<SqliteDatabase> {
        Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("db"))
    }

    async fn build_state_with_key(db: Arc<SqliteDatabase>) -> Arc<AppState> {
        let registry = Arc::new(ProviderRegistry::new());
        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = Arc::new(ProxyRouter::new(registry, cache, config, image_store));
        let mat = Arc::new(Materializer::new(Arc::new(NoopStorage2), reqwest::Client::new()));
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); p.push("models");
        let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("models"));
        Arc::new(AppState {
            router,
            db,
            master_key: Some("master".to_string()),
            registry: cap_registry,
            materializer: mat,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    async fn seed_user_session(db: &Arc<SqliteDatabase>, role: Role, email: &str) -> (User, String, String) {
        let user = User {
            id: format!("u-{}", uuid::Uuid::new_v4()),
            email: email.to_string(),
            password_hash: None,
            role,
            oauth_github_id: None,
            oauth_google_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            is_active: true,
        };
        db.create_user(&user).await.expect("create user");
        let session_token = generate_session_token();
        let csrf_token = generate_csrf_token();
        let sess = Session {
            id: session_token.clone(),
            user_id: user.id.clone(),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(7),
            ip: None, user_agent: None,
            csrf_token: csrf_token.clone(),
        };
        db.create_session(&sess).await.expect("create session");
        (user, session_token, csrf_token)
    }

    fn build_keys_router_authed(state: Arc<AppState>) -> axum::Router {
        let auth_state = state.clone();
        axum::Router::new()
            .route("/v1/keys", get(list_api_keys).post(create_api_key))
            .route("/v1/keys/{id}", delete(revoke_api_key))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                let s = auth_state.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state)
    }

    #[tokio::test]
    async fn member_session_only_sees_own_keys() {
        let db = build_db().await;
        let (member, member_sess, _) = seed_user_session(&db, Role::Member, "member@t.com").await;

        // Create one key as member (via DB directly, setting owner)
        db.create_api_key("member-key", "hash1", "lg-m", None, None, "generate,read", None).await.unwrap();
        let keys = db.list_api_keys().await.unwrap();
        db.set_api_key_owner(&keys[0].id, &member.id).await.unwrap();

        // Create another key not owned by member
        db.create_api_key("other-key", "hash2", "lg-o", None, None, "generate,read", None).await.unwrap();
        // no owner set for second key

        let app = build_keys_router_authed(build_state_with_key(db).await);
        let req = Request::builder()
            .uri("/v1/keys")
            .header("cookie", format!("litegen_session={}", member_sess))
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // Member should only see their own key (1 row)
        assert_eq!(json["data"].as_array().unwrap().len(), 1);
        assert_eq!(json["data"][0]["name"], "member-key");
    }

    #[tokio::test]
    async fn admin_sees_all_keys() {
        let db = build_db().await;
        let (_, admin_sess, _) = seed_user_session(&db, Role::Admin, "admin@t.com").await;

        // Create 2 keys with no owner
        db.create_api_key("k1", "h1", "lg-1", None, None, "generate", None).await.unwrap();
        db.create_api_key("k2", "h2", "lg-2", None, None, "generate", None).await.unwrap();

        let app = build_keys_router_authed(build_state_with_key(db).await);
        let req = Request::builder()
            .uri("/v1/keys")
            .header("cookie", format!("litegen_session={}", admin_sess))
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["data"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn member_cannot_revoke_other_users_key() {
        let db = build_db().await;
        let (member, member_sess, member_csrf) = seed_user_session(&db, Role::Member, "m@t.com").await;
        let (other, _, _) = seed_user_session(&db, Role::Member, "o@t.com").await;

        // Create a key owned by `other`
        db.create_api_key("others-key", "h1", "lg-x", None, None, "generate", None).await.unwrap();
        let keys = db.list_api_keys().await.unwrap();
        let key_id = keys[0].id;
        db.set_api_key_owner(&key_id, &other.id).await.unwrap();
        let _ = member.id; // suppress unused warning

        let app = build_keys_router_authed(build_state_with_key(db).await);
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/v1/keys/{}", key_id))
            .header("cookie", format!("litegen_session={}", member_sess))
            .header("x-csrf-token", member_csrf)
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}

#[cfg(test)]
mod health_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use crate::db::sqlite::SqliteDatabase;
    use crate::capabilities::CapabilityRegistry;
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::LocalStore;
    use crate::proxy::materializer::{Materializer, MaterializeError, TempStorage};
    use crate::api::middleware::rate_limit::RateLimiter;
    use bytes::Bytes;

    struct NoopStorage;
    #[async_trait::async_trait]
    impl TempStorage for NoopStorage {
        async fn put(&self, key: &str, _bytes: Bytes, _ct: &str) -> Result<String, MaterializeError> {
            Ok(format!("local://{}", key))
        }
        async fn delete(&self, _key: &str) -> Result<(), MaterializeError> { Ok(()) }
    }

    async fn build_state_with_real_db() -> Arc<AppState> {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
        let registry = Arc::new(ProviderRegistry::new());
        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = Arc::new(ProxyRouter::new(registry, cache, config, image_store));
        let materializer = Arc::new(Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); p.push("models");
        let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("models"));
        Arc::new(AppState {
            router, db, master_key: None, registry: cap_registry, materializer,
            rate_limiter: Arc::new(RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    fn build_health_router(state: Arc<AppState>) -> axum::Router {
        use axum::routing::get;
        axum::Router::new()
            .route("/health/live", get(liveness))
            .route("/health/ready", get(readiness))
            .with_state(state)
    }

    #[tokio::test]
    async fn live_always_returns_200() {
        let state = build_state_with_real_db().await;
        let app = build_health_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/health/live")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "/health/live must always return 200");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body.get("status").is_some(), "should have 'status' field");
    }

    #[tokio::test]
    async fn ready_returns_200_when_db_ok_even_with_no_global_providers() {
        // BYO model: providers are resolved per-request (global OR org-scoped), so
        // an empty global-provider list must NOT make the instance "not_ready" as
        // long as the DB is reachable.
        let state = build_state_with_real_db().await;
        let app = build_health_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/health/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "/health/ready should be 200 when DB is up, even with no global providers"
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ready");
        assert_eq!(body["checks"]["db"], true, "db should be true when sqlite is up");
        assert_eq!(body["checks"]["providers"], serde_json::json!([]), "no global providers configured");
    }

    #[tokio::test]
    async fn ready_returns_200_when_db_ok_and_mock_provider_healthy() {
        use crate::providers::image::mock::MockProvider;
        use crate::providers::{ImageProvider, ProviderInstanceConfig};
        use std::collections::HashMap;

        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
        let provider_registry = Arc::new(ProviderRegistry::new());

        // Register mock provider (always healthy)
        let mut mp = MockProvider::new();
        mp.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: String::new(), api_keys: vec![], api_base: None,
            model_mapping: HashMap::new(), extra_headers: HashMap::new(), options: None,
        });
        provider_registry.register_mock_image(Arc::new(mp)).await;

        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = Arc::new(ProxyRouter::new(provider_registry, cache, config, image_store));
        let materializer = Arc::new(Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); p.push("models");
        let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("models"));

        let state = Arc::new(AppState {
            router, db, master_key: None, registry: cap_registry, materializer,
            rate_limiter: Arc::new(RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),

            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        });

        let app = build_health_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/health/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "/health/ready should return 200 when DB ok + mock provider healthy"
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ready");
        assert_eq!(body["checks"]["db"], true);
    }
}

#[cfg(test)]
mod model_info_tests {
    use super::*;
    use crate::capabilities::CapabilityRegistry;

    fn shipped_registry() -> CapabilityRegistry {
        // <repo>/models, mirroring build_test_state() in key_endpoint_tests.
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        CapabilityRegistry::from_dir(&p).expect("load shipped models")
    }

    #[test]
    fn max_images_reflects_ref_inputs_max_total() {
        let reg = shipped_registry();

        // Multi-image model: ByteDance Seedream accepts up to 6 init images.
        let seedream = reg
            .get("bytedance/seedream-4-0-250828")
            .expect("seedream present");
        assert!(
            project_model_info(seedream).capabilities.max_images >= 2,
            "multi-image model must report max_images >= 2"
        );

        // Single-ref model: Stability SD3-Large caps at one init image.
        let sd3 = reg.get("stability/sd3-large").expect("sd3-large present");
        assert_eq!(project_model_info(sd3).capabilities.max_images, 1);

        // No-ref model: Recraft v3 has no ref_inputs → defaults to 1.
        let recraft = reg.get("recraft/recraftv3").expect("recraftv3 present");
        assert_eq!(project_model_info(recraft).capabilities.max_images, 1);
    }
}

// ─── Task 9 tenant-scoping tests ──────────────────────────────────────────────

#[cfg(test)]
mod tenant_scoping_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::{middleware, routing::{get, post}};
    use tower::ServiceExt;
    use crate::api::middleware::{auth_middleware, AppState, DEFAULT_ORG_ID, DEFAULT_APP_ID};
    use crate::capabilities::CapabilityRegistry;
    use crate::config::{AppConfig, CacheGlobalConfig, Mode};
    use crate::db::sqlite::SqliteDatabase;
    use crate::db::DatabaseStore;
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::materializer::{Materializer, MaterializeError, TempStorage};
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::LocalStore;
    use crate::types::{Application, Organization};
    use bytes::Bytes;

    struct NoopStorageT;
    #[async_trait::async_trait]
    impl TempStorage for NoopStorageT {
        async fn put(&self, key: &str, _b: Bytes, _ct: &str) -> Result<String, MaterializeError> {
            Ok(format!("local://{}", key))
        }
        async fn delete(&self, _k: &str) -> Result<(), MaterializeError> { Ok(()) }
    }

    async fn build_db() -> Arc<SqliteDatabase> {
        Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("db"))
    }

    fn make_org(id: &str, slug: &str) -> Organization {
        let now = chrono::Utc::now();
        Organization {
            id: id.to_string(), name: format!("Org {slug}"), slug: slug.to_string(),
            plan: "free".to_string(), status: "active".to_string(),
            created_at: now, updated_at: now,
        }
    }
    fn make_app(id: &str, org_id: &str, slug: &str) -> Application {
        let now = chrono::Utc::now();
        Application {
            id: id.to_string(), org_id: org_id.to_string(), name: format!("App {slug}"),
            slug: slug.to_string(), status: "active".to_string(),
            created_at: now, updated_at: now,
        }
    }

    async fn build_state(db: Arc<SqliteDatabase>, mode: Mode, master_key: Option<String>) -> Arc<AppState> {
        let registry = Arc::new(ProviderRegistry::new());
        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = Arc::new(ProxyRouter::new(registry, cache, config, image_store));
        let mat = Arc::new(Materializer::new(Arc::new(NoopStorageT), reqwest::Client::new()));
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); p.push("models");
        let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("models"));
        Arc::new(AppState {
            router, db, master_key, registry: cap_registry, materializer: mat,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    fn build_router(state: Arc<AppState>) -> axum::Router {
        let auth_state = state.clone();
        axum::Router::new()
            .route("/v1/keys", post(create_api_key).get(list_api_keys))
            .route("/v1/generations", get(list_generations))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                let s = auth_state.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state)
    }

    // POST /v1/keys returns a pk_live_ id and an sk_live_ secret (single_tenant master ctx).
    #[tokio::test]
    async fn create_key_returns_id_and_secret() {
        let db = build_db().await;
        let state = build_state(db, Mode::SingleTenant, None).await;
        let app = build_router(state);

        let body = serde_json::to_vec(&serde_json::json!({"name": "k", "scopes": "generate,read"})).unwrap();
        let req = Request::builder().method("POST").uri("/v1/keys")
            .header("content-type", "application/json").body(Body::from(body)).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(created["public_id"].as_str().unwrap().starts_with("pk_live_"), "public_id should be pk_live_");
        assert!(created["key"].as_str().unwrap().starts_with("sk_live_"), "secret should be sk_live_");
    }

    // A created sk_live_ secret authenticates via Bearer and is scoped to its tenant.
    #[tokio::test]
    async fn bearer_secret_authenticates_and_scopes() {
        let db = build_db().await;
        // master_key set so Bearer path validates DB keys (no dev bypass).
        let state = build_state(db.clone(), Mode::SingleTenant, Some("master".to_string())).await;
        let app = build_router(state.clone());

        // Create a key under the default-org master context.
        let body = serde_json::to_vec(&serde_json::json!({"name": "bk", "scopes": "generate,read,admin"})).unwrap();
        let req = Request::builder().method("POST").uri("/v1/keys")
            .header("authorization", "Bearer master")
            .header("content-type", "application/json").body(Body::from(body)).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let secret = created["key"].as_str().unwrap().to_string();

        // Use the secret as a Bearer token to call a read route → 200 + sees default-org data.
        db.insert_generation("g-bearer-1", None, "mock/v", "mock", "video", None, 0.0, Some(DEFAULT_ORG_ID), Some(DEFAULT_APP_ID)).await.unwrap();
        let req = Request::builder().method("GET").uri("/v1/generations")
            .header("authorization", format!("Bearer {}", secret))
            .body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "bearer sk_live_ should authenticate");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let listed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(listed["total"], 1, "should see the default-org generation");
    }

    // A hosted master/platform key (org_id None) is rejected by tenant-data routes.
    #[tokio::test]
    async fn tenant_data_route_rejects_no_org() {
        let db = build_db().await;
        let state = build_state(db, Mode::Hosted, Some("master".to_string())).await;
        let app = build_router(state);

        // GET /v1/keys with the hosted master key → 403 (no active org).
        let req = Request::builder().method("GET").uri("/v1/keys")
            .header("authorization", "Bearer master").body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "hosted master GET /v1/keys → 403");

        // GET /v1/generations likewise → 403.
        let req = Request::builder().method("GET").uri("/v1/generations")
            .header("authorization", "Bearer master").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "hosted master GET /v1/generations → 403");
    }

    // A generation under app A is not visible to a Bearer key scoped to app B.
    #[tokio::test]
    async fn generations_are_isolated_per_tenant_via_endpoint() {
        let db = build_db().await;
        // Two orgs/apps; one key in each.
        db.create_organization(&make_org("org-a", "org-a")).await.unwrap();
        db.create_organization(&make_org("org-b", "org-b")).await.unwrap();
        db.create_application(&make_app("app-a", "org-a", "app-a")).await.unwrap();
        db.create_application(&make_app("app-b", "org-b", "app-b")).await.unwrap();

        let kp_a = crate::auth::secrets::generate_key_pair();
        let kp_b = crate::auth::secrets::generate_key_pair();
        db.create_api_key_scoped("org-a", "app-a", &kp_a.public_id, "ka", &kp_a.secret_hash, &kp_a.prefix, None, None, "generate,read", None, None).await.unwrap();
        db.create_api_key_scoped("org-b", "app-b", &kp_b.public_id, "kb", &kp_b.secret_hash, &kp_b.prefix, None, None, "generate,read", None, None).await.unwrap();

        // One generation per tenant.
        db.insert_generation("gen-org-a", None, "mock/v", "mock", "video", None, 0.0, Some("org-a"), Some("app-a")).await.unwrap();
        db.insert_generation("gen-org-b", None, "mock/v", "mock", "video", None, 0.0, Some("org-b"), Some("app-b")).await.unwrap();

        let state = build_state(db, Mode::Hosted, Some("master".to_string())).await;
        let app = build_router(state);

        // Caller with app-A key sees only A's generation.
        let req = Request::builder().method("GET").uri("/v1/generations")
            .header("authorization", format!("Bearer {}", kp_a.secret)).body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let listed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(listed["total"], 1, "app-A key should see exactly one row");
        assert_eq!(listed["data"][0]["id"], "gen-org-a");
        assert!(listed["data"].as_array().unwrap().iter().all(|g| g["id"] != "gen-org-b"), "must not see app-B's row");
    }

    // SSRF: in hosted mode a tenant must not be able to register a webhook URL
    // that targets an internal address. Uses literal IPs so the test needs no DNS.
    #[tokio::test]
    async fn hosted_rejects_private_webhook_url_on_key_create() {
        let db = build_db().await;
        db.create_organization(&make_org("org-a", "org-a")).await.unwrap();
        db.create_application(&make_app("app-a", "org-a", "app-a")).await.unwrap();
        let kp = crate::auth::secrets::generate_key_pair();
        db.create_api_key_scoped("org-a", "app-a", &kp.public_id, "ka", &kp.secret_hash, &kp.prefix, None, None, "generate,read", None, None).await.unwrap();

        let state = build_state(db, Mode::Hosted, Some("master".to_string())).await;
        let app = build_router(state);

        // Cloud metadata endpoint → rejected.
        let body = serde_json::to_vec(&serde_json::json!({
            "name": "evil", "webhook_url": "http://169.254.169.254/latest/meta-data/"
        })).unwrap();
        let req = Request::builder().method("POST").uri("/v1/keys")
            .header("authorization", format!("Bearer {}", kp.secret))
            .header("content-type", "application/json")
            .body(Body::from(body)).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "private webhook_url must be rejected in hosted mode");

        // A public literal IP is accepted (no DNS needed).
        let body = serde_json::to_vec(&serde_json::json!({
            "name": "ok", "webhook_url": "https://93.184.216.34/hook"
        })).unwrap();
        let req = Request::builder().method("POST").uri("/v1/keys")
            .header("authorization", format!("Bearer {}", kp.secret))
            .header("content-type", "application/json")
            .body(Body::from(body)).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED, "public webhook_url should be accepted");
    }

    // In single-tenant mode the operator is trusted; internal webhook URLs are allowed.
    #[tokio::test]
    async fn single_tenant_allows_private_webhook_url() {
        let db = build_db().await;
        let state = build_state(db, Mode::SingleTenant, None).await;
        let app = build_router(state);

        let body = serde_json::to_vec(&serde_json::json!({
            "name": "local", "webhook_url": "http://127.0.0.1:9000/hook"
        })).unwrap();
        let req = Request::builder().method("POST").uri("/v1/keys")
            .header("content-type", "application/json")
            .body(Body::from(body)).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED, "single-tenant operator may use internal webhook URLs");
    }
}

// ─── get_3d_status: completed-without-mesh must fail, never report success ──
//
// The consumer contract is absolute: a `completed` 3D generation ALWAYS
// carries exactly one mesh asset, because the downstream consumer refunds a
// slot on `failed` and would NOT on `completed`. A provider that reports
// success with no files is a contract violation the handler must convert to
// `failed`, not pass through. `MockModel3dProvider` never produces this state
// (it always returns files on success), so this needs a purpose-built fake
// provider — registered via `ProviderRegistry::register_mock_model3d`, which
// is `#[cfg(test)]`-gated and therefore only reachable from an in-crate unit
// test (an integration test under `tests/` links the lib WITHOUT `--cfg
// test` and cannot see it — see `litegen-core/tests/harness/mod.rs` for the
// `init_from_config` workaround used there, which only ever wires up the
// real `MockModel3dProvider`).
#[cfg(test)]
mod model3d_completion_contract_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware;
    use tower::ServiceExt;
    use bytes::Bytes;
    use crate::api::middleware::auth_middleware;
    use crate::capabilities::CapabilityRegistry;
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::db::sqlite::SqliteDatabase;
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::materializer::{MaterializeError, Materializer, MaterializedRequest, TempStorage};
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::LocalStore;
    use crate::providers::{
        HealthCheckResult, Model3dExtras, Model3dGenerationHandle, Model3dGenerationPollResult,
        Model3dProvider, ProviderError, ProviderInstanceConfig,
    };

    struct NoopStorage;

    #[async_trait::async_trait]
    impl TempStorage for NoopStorage {
        async fn put(&self, key: &str, _bytes: Bytes, _ct: &str) -> Result<String, MaterializeError> {
            Ok(format!("local://{}", key))
        }
        async fn delete(&self, _key: &str) -> Result<(), MaterializeError> { Ok(()) }
    }

    /// Always reports `Completed` with zero files — the exact contract
    /// violation `get_3d_status` must convert to `failed`.
    struct NoMeshModel3dProvider;

    #[async_trait::async_trait]
    impl Model3dProvider for NoMeshModel3dProvider {
        fn name(&self) -> &str { "mock" }
        fn configure(&mut self, _config: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }

        async fn generate(
            &self,
            _model: &crate::capabilities::ModelSchema,
            _base: &BaseGenerationRequest,
            _extras: &Model3dExtras,
            _materialized: &MaterializedRequest,
        ) -> Result<Model3dGenerationHandle, ProviderError> {
            Ok(Model3dGenerationHandle {
                provider_job_id: "no-mesh-job-1".to_string(),
                provider: "mock".to_string(),
                model: "mock/mesh-3d".to_string(),
                stage_context: None,
            })
        }

        async fn poll_status(
            &self,
            _handle: &Model3dGenerationHandle,
        ) -> Result<Model3dGenerationPollResult, ProviderError> {
            Ok(Model3dGenerationPollResult {
                status: GenerationStatus::Completed,
                progress: 100,
                files: vec![], // contract violation under test
                error: None,
                metadata: Default::default(),
            })
        }

        async fn estimate_cost(
            &self,
            model: &crate::capabilities::ModelSchema,
            _request: &Model3dGenerationRequest,
        ) -> Result<CostEstimate, ProviderError> {
            Ok(crate::providers::build_cost_estimate(
                model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None,
            ))
        }

        async fn health_check(&self) -> HealthCheckResult {
            HealthCheckResult { healthy: true, message: "ok".into(), latency_ms: Some(0) }
        }
    }

    async fn build_state_with_no_mesh_provider() -> Arc<AppState> {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory db"));
        let provider_registry = Arc::new(ProviderRegistry::new());
        provider_registry.register_mock_model3d(Arc::new(NoMeshModel3dProvider)).await;

        let config = Arc::new(AppConfig::default());
        let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
        let image_store = Arc::new(LocalStore);
        let router = Arc::new(ProxyRouter::new(provider_registry, cache, config, image_store));
        let materializer = Arc::new(Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));

        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); p.push("models");
        let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("load shipped models"));

        Arc::new(AppState {
            router, db, master_key: None, registry: cap_registry, materializer,
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    fn build_3d_router(state: Arc<AppState>) -> axum::Router {
        use axum::routing::{get, post};
        let auth_state = state.clone();
        axum::Router::new()
            .route("/v1/models3d/generations", post(generate_3d))
            .route("/v1/models3d/{id}", get(get_3d_status))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                let s = auth_state.clone();
                async move {
                    let headers = req.headers().clone();
                    auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state)
    }

    #[tokio::test]
    async fn completed_without_mesh_is_reported_as_failed_not_completed() {
        let state = build_state_with_no_mesh_provider().await;
        let app = build_3d_router(state);

        let resp = app.clone().oneshot(
            Request::post("/v1/models3d/generations")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"mock/mesh-3d","prompt":"a fox"}"#))
                .unwrap(),
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let submitted: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = submitted["id"].as_str().unwrap().to_string();

        let resp = app.oneshot(
            Request::get(format!("/v1/models3d/{id}")).body(Body::empty()).unwrap(),
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let polled: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(polled["status"], "failed", "completed-without-mesh must be reported as failed: {polled}");
        assert!(
            polled["error"].as_str().is_some_and(|e| !e.is_empty()),
            "failed response must carry a non-empty error: {polled}"
        );
        assert!(polled.get("assets").is_none(), "a failed generation carries no assets: {polled}");
    }
}
