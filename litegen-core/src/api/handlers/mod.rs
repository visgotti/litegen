pub mod auth_password;
pub mod oauth;
pub mod users;
pub mod account;
pub mod orgs;

pub use auth_password::{
    csrf_token, login, logout, me, password_reset_confirm, password_reset_request, signup,
};
pub use oauth::{github_callback, github_start, google_callback, google_start};
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

use crate::providers::{ImageExtras, VideoExtras};
use crate::types::*;

use super::middleware::{AppState, KeyContext};
use super::middleware::validator::{ValidatedImage, ValidatedVideo, dropped_header};

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
    // Acquire an in-flight slot; 503 immediately if at capacity.
    let _permit = match state.in_flight.try_acquire() {
        Some(p) => p,
        None => return (
            StatusCode::SERVICE_UNAVAILABLE,
            [("retry-after", "1")],
            Json(error_response("server at capacity, retry shortly", 503)),
        ).into_response(),
    };

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

    match state.router.generate_image(&validated.schema, &validated.request.base, &extras, &materialized).await {
        Ok(response) => {
            let latency = start.elapsed().as_millis() as i64;
            let cost = response.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);

            // Post-charge quota if a DB key was used
            let mut quota_exceeded = false;
            if let Some(key_id) = key_ctx.as_ref().and_then(|c| c.key_id) {
                if cost > 0.0 {
                    match state.db.atomic_charge_tokens(&key_id, cost).await {
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, key_id = %key_id, "Quota charge failed after generation");
                            quota_exceeded = true;
                        }
                    }
                }
            }

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
            if quota_exceeded {
                resp.headers_mut().insert(
                    "x-litegen-quota-exceeded",
                    axum::http::HeaderValue::from_static("true"),
                );
            }
            resp
        }
        Err(e) => {
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
    validated: ValidatedImage,
) -> impl IntoResponse {
    match state.router.estimate_image_cost(&validated.schema, &validated.request).await {
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
    // Acquire an in-flight slot; 503 immediately if at capacity.
    let _permit = match state.in_flight.try_acquire() {
        Some(p) => p,
        None => return (
            StatusCode::SERVICE_UNAVAILABLE,
            [("retry-after", "1")],
            Json(error_response("server at capacity, retry shortly", 503)),
        ).into_response(),
    };

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

    match state.router.generate_video(&validated.schema, &validated.request.base, &extras, &materialized).await {
        Ok(response) => {
            let latency = start.elapsed().as_millis() as i64;
            let cost = response.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);

            // Post-charge quota if a DB key was used
            let mut quota_exceeded = false;
            if let Some(key_id) = key_ctx.as_ref().and_then(|c| c.key_id) {
                if cost > 0.0 {
                    match state.db.atomic_charge_tokens(&key_id, cost).await {
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, key_id = %key_id, "Quota charge failed after video generation");
                            quota_exceeded = true;
                        }
                    }
                }
            }

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
            if quota_exceeded {
                resp.headers_mut().insert(
                    "x-litegen-quota-exceeded",
                    axum::http::HeaderValue::from_static("true"),
                );
            }
            resp
        }
        Err(e) => {
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
#[utoipa::path(
    get,
    path = "/v1/generations/{id}",
    params(("id" = String, Path, description = "Generation ID (litegen-vid-...)")),
    responses(
        (status = 200, description = "Generation detail", body = crate::types::Generation),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Videos"
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
    validated: ValidatedVideo,
) -> impl IntoResponse {
    match state.router.estimate_video_cost(&validated.schema, &validated.request).await {
        Ok(estimate) => (StatusCode::OK, Json(serde_json::to_value(estimate).unwrap())).into_response(),
        Err(e) => {
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

// ─── Models ─────────────────────────────────────────────────────────────────

/// GET /v1/models — List all available models.
#[utoipa::path(
    get,
    path = "/v1/models",
    responses(
        (status = 200, description = "List of available models", body = ModelListResponse),
    ),
    tag = "Models"
)]
pub async fn list_models(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let models: Vec<ModelInfo> = state.registry.all()
        .map(project_model_info)
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
/// Returns 200 only if DB is reachable and at least one provider is healthy.
/// Returns 503 otherwise. No auth required.
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

    let providers_ok = !healthy_providers.is_empty();

    let body = ReadinessResponse {
        status: if db_ok && providers_ok { "ready".to_string() } else { "not_ready".to_string() },
        checks: ReadinessChecks {
            db: db_ok,
            providers: healthy_providers,
        },
    };

    let status_code = if db_ok && providers_ok {
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

/// GET /v1/logs — Get request logs (paginated).
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
pub async fn get_logs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    match state.db.get_request_logs(params.page, params.per_page).await {
        Ok((logs, total)) => {
            let total_pages = ((total as f64) / (params.per_page as f64)).ceil() as u32;
            let response = PaginatedResponse {
                data: logs,
                total,
                page: params.page,
                per_page: params.per_page,
                total_pages,
            };
            (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to get logs");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response()
        }
    }
}

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
}

fn default_key_scopes() -> String { "generate,read".to_string() }

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
pub async fn list_generations(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<KeyContext>,
    Query(query): Query<PaginationParams>,
) -> impl IntoResponse {
    use crate::auth::permissions::Permission;

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
        // Read-own member: tenant-scope the rows then filter to their owned keys.
        let all_gens = match state.db.list_generations_for_tenant(org_id, app_id, 1, 10000).await {
            Ok(g) => g,
            Err(e) => {
                error!(error = %e, "Failed to list generations");
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
            }
        };
        let filtered: Vec<_> = all_gens.into_iter()
            .filter(|g| g.key_id.map(|kid| owned_key_ids.contains(&kid)).unwrap_or(false))
            .collect();
        let total = filtered.len() as i64;
        let offset = ((query.page.saturating_sub(1)) * query.per_page) as usize;
        let paged: Vec<_> = filtered.into_iter().skip(offset).take(query.per_page as usize).collect();
        let total_pages = ((total as f64) / (query.per_page as f64)).ceil() as u32;
        let response = PaginatedResponse {
            data: paged,
            total: total as u64,
            page: query.page,
            per_page: query.per_page,
            total_pages,
        };
        return (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
    }

    let (total, gens) = match tokio::try_join!(
        state.db.count_generations_for_tenant(org_id, app_id),
        state.db.list_generations_for_tenant(org_id, app_id, query.page, query.per_page),
    ) {
        Ok(pair) => pair,
        Err(e) => {
            error!(error = %e, "Failed to list generations");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };

    let total_pages = ((total as f64) / (query.per_page as f64)).ceil() as u32;
    let response = PaginatedResponse {
        data: gens,
        total: total as u64,
        page: query.page,
        per_page: query.per_page,
        total_pages,
    };
    (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response()
}

#[derive(Deserialize)]
pub struct CancelGenerationBody {
    pub status: String,
}

/// PATCH /v1/generations/{id} — Soft-cancel a generation.
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
    match state.db.get_generation(&id).await {
        Ok(Some(g)) if g.org_id.as_deref() == Some(ctx_org) => {}
        Ok(_) => return (StatusCode::NOT_FOUND, Json(error_response("Generation not found", 404))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to get generation for cancel");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };

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
    let _existing = match state.db.get_api_key(&id).await {
        Ok(Some(k)) if k.org_id.as_deref() == Some(ctx_org) => k,
        Ok(_) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to fetch key for rotate");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    }; // fetched only to verify org ownership

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
    (StatusCode::OK, Json(serde_json::json!({
        "id": rotated.id,
        "public_id": kp.public_id,
        "key": kp.secret,
        "prefix": rotated.key_prefix,
        "name": rotated.name,
        "scopes": rotated.scopes,
        "token_quota": rotated.token_quota,
        "rpm_limit": rotated.rpm_limit,
        "webhook_url": rotated.webhook_url,
        "expires_at": rotated.expires_at,
    }))).into_response()
}

// ─── Key Test-Webhook ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct WebhookTestResult {
    pub delivered: bool,
    pub status_code: Option<u16>,
    pub error: Option<String>,
}

/// POST /v1/keys/{id}/test-webhook — Fire one synthetic webhook and return the result.
pub async fn test_webhook(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let key = match state.db.get_api_key(&id).await {
        Ok(Some(k)) => k,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to fetch key for test-webhook");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };

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

    let client = reqwest::Client::new();
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
    let page = query.page.unwrap_or(1);
    let per_page = if is_csv { 10000 } else { query.per_page.unwrap_or(50) };

    // Fetch all tenant rows, then apply the optional model/provider/status/date
    // filters in-handler (the tenant-scoped query is not itself filter-aware).
    let (all_logs, _) = match state.db.get_request_logs_for_tenant(org_id, app_id, 1, 100_000).await {
        Ok(pair) => pair,
        Err(e) => {
            error!(error = %e, "Failed to get tenant logs");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };
    let filtered: Vec<RequestLog> = all_logs
        .into_iter()
        .filter(|l| query.model.as_deref().map(|m| l.model == m).unwrap_or(true))
        .filter(|l| query.provider.as_deref().map(|p| l.provider == p).unwrap_or(true))
        .filter(|l| query.status.as_deref().map(|s| format!("{}", l.status) == s).unwrap_or(true))
        // `from`/`to` params are compared lexicographically after formatting as RFC 3339
        .filter(|l| query.from.as_deref().map(|f| l.created_at.to_rfc3339().as_str() >= f).unwrap_or(true))
        .filter(|l| query.to.as_deref().map(|t| l.created_at.to_rfc3339().as_str() <= t).unwrap_or(true))
        .collect();

    if is_csv {
        let date = chrono::Utc::now().format("%Y%m%d").to_string();
        let filename = format!("logs-{}.csv", date);
        return csv_response(logs_to_csv(&filtered), &filename);
    }

    let total = filtered.len() as u64;
    let offset = ((page.saturating_sub(1)) * per_page) as usize;
    let paged: Vec<RequestLog> = filtered.into_iter().skip(offset).take(per_page as usize).collect();
    let total_pages = ((total as f64) / (per_page as f64)).ceil() as u32;
    let response = PaginatedResponse {
        data: paged,
        total,
        page,
        per_page,
        total_pages,
    };
    (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response()
}

// ─── Webhook Delivery Log ────────────────────────────────────────────────────

/// GET /v1/keys/{id}/webhook-deliveries — List webhook deliveries for a key (admin scope).
pub async fn list_webhook_deliveries(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    let key_id_str = id.to_string();
    match state.db.list_webhook_deliveries(&key_id_str, params.page, params.per_page).await {
        Ok((deliveries, total)) => {
            let total_pages = ((total as f64) / (params.per_page as f64)).ceil() as u32;
            let response = PaginatedResponse {
                data: deliveries,
                total: total as u64,
                page: params.page,
                per_page: params.per_page,
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
    let page = query.page.unwrap_or(1);
    let per_page = if is_csv { 10000 } else { query.per_page.unwrap_or(50) };

    // Fetch all org-scoped entries, then apply the optional actor/action/date
    // filters in-handler (the tenant-scoped query is not itself filter-aware).
    let (all_entries, _) = match state.db.list_audit_log_for_tenant(org_id, 1, 100_000).await {
        Ok(pair) => pair,
        Err(e) => {
            error!(error = %e, "Failed to list audit log");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response(&e.to_string(), 500))).into_response();
        }
    };
    let filtered: Vec<AuditLogEntry> = all_entries
        .into_iter()
        .filter(|e| query.actor_key_id.as_deref().map(|a| e.actor_key_id.as_deref() == Some(a)).unwrap_or(true))
        .filter(|e| query.action.as_deref().map(|a| e.action == a).unwrap_or(true))
        // `from`/`to` params are compared lexicographically after formatting as RFC 3339
        .filter(|e| query.from.as_deref().map(|f| e.created_at.to_rfc3339().as_str() >= f).unwrap_or(true))
        .filter(|e| query.to.as_deref().map(|t| e.created_at.to_rfc3339().as_str() <= t).unwrap_or(true))
        .collect();

    if is_csv {
        let date = chrono::Utc::now().format("%Y%m%d").to_string();
        let filename = format!("audit-{}.csv", date);
        return csv_response(audit_to_csv(&filtered), &filename);
    }
    let total = filtered.len() as u64;
    let offset = ((page.saturating_sub(1)) * per_page) as usize;
    let paged: Vec<AuditLogEntry> = filtered.into_iter().skip(offset).take(per_page as usize).collect();
    let total_pages = ((total as f64) / (per_page as f64)).ceil() as u32;
    let response = PaginatedResponse {
        data: paged,
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
    ),
    tag = "Admin"
)]
pub async fn clear_cache(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    state.router.cache.clear().await;
    Json(CacheClearedResponse { cleared: true })
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
        .layer(middleware::from_fn(|req: axum::extract::Request, next: middleware::Next| async {
            check_scope(Scope::Generate, req, next).await
        }));

    // Read routes — require Scope::Read
    let read_routes = axum::Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/models/{*id}", get(get_model_schema))
        .route("/health", get(health_check))
        .route("/v1/stats", get(get_stats))
        .route("/v1/logs", get(get_logs_filtered))
        .route("/v1/logs/{id}/artifact", get(get_log_artifact))
        .route("/v1/generations", get(list_generations))
        .route("/v1/generations/{id}", get(get_generation))
        .route("/v1/generations/{id}", patch(cancel_generation))
        .layer(middleware::from_fn(|req: axum::extract::Request, next: middleware::Next| async {
            check_scope(Scope::Read, req, next).await
        }));

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
        }));

    // Auth-required routes (auth middleware inserts KeyContext)
    let auth_required_routes = axum::Router::new()
        .route("/v1/auth/logout", post(logout))
        .route("/v1/auth/me", get(me))
        .route("/v1/auth/csrf", get(csrf_token))
        // Users management (requires session + permissions checked per handler)
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
            "/v1/apps/{app_id}/provider-credentials",
            get(orgs::list_provider_credentials).post(orgs::create_provider_credential),
        )
        .route(
            "/v1/apps/{app_id}/provider-credentials/{provider}",
            delete(orgs::delete_provider_credential),
        )
        .layer(middleware::from_fn_with_state(
            csrf_state.clone(),
            csrf_middleware,
        ));

    // Unauthenticated auth routes
    let unauth_routes = axum::Router::new()
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
        .route("/v1/auth/oauth/google/callback", get(google_callback));

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
        .with_state(state)
}

// ─── Artifact endpoint ──────────────────────────────────────────────────────

/// GET /v1/logs/{id}/artifact — Retrieve the stored artifact for a request log.
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
            // Cross-tenant isolation: if the artifact carries an org_id and it does not
            // match the caller's org, return 404 (same as not-found — do not reveal existence).
            if artifact.org_id.as_deref().map(|o| o != ctx_org).unwrap_or(false) {
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

fn error_response(message: &str, code: u16) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "message": message,
            "type": match code {
                400 => "invalid_request_error",
                401 => "authentication_error",
                403 => "forbidden",
                404 => "not_found_error",
                429 => "rate_limit_error",
                502 => "provider_error",
                503 => "service_unavailable",
                _ => "internal_error",
            },
            "code": code
        }
    })
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
    async fn ready_returns_503_when_no_providers_registered() {
        // With no providers registered, providers list is empty → not_ready
        let state = build_state_with_real_db().await;
        let app = build_health_router(state);

        let req = Request::builder()
            .method("GET")
            .uri("/health/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        // No providers → 503 even if DB is ok
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "/health/ready should return 503 when no providers healthy"
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["checks"]["db"], true, "db should be true when sqlite is up");
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
        db.create_api_key_scoped("org-a", "app-a", &kp_a.public_id, "ka", &kp_a.secret_hash, &kp_a.prefix, None, None, "generate,read", None).await.unwrap();
        db.create_api_key_scoped("org-b", "app-b", &kp_b.public_id, "kb", &kp_b.secret_hash, &kp_b.prefix, None, None, "generate,read", None).await.unwrap();

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
}
