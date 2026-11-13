pub mod auth_password;
pub mod oauth;
pub mod users;
pub mod account;
pub mod orgs;
pub mod convert;
pub mod management;

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

use crate::access::present::FileOwner;
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
// Pure reads of a generation row, now in types (engine boundary, D4); kept at
// these paths for the handlers and their callers.
pub(crate) use crate::types::generation::{generation_assets, model3d_response_from_row, row_requested_formats, usage_from_row_cost};

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
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "1–255 visible ASCII characters. The same key and the same request within 24 hours replay the first response (`Idempotent-Replayed: true`) and bill nothing; the same key with a different request is a 409. A request refused before any vendor was called does not spend the key."),
        ("X-Litegen-Keep-Outputs" = Option<String>, Header, description = "`false` keeps nothing for this request: no copy of the output is stored anywhere, and the answer carries the image itself or the vendor's own link (docs/storage-access.md). A request can keep less than its app does, never more; any other value is a 400 `invalid_keep_outputs_header`."),
        ("Prefer" = Option<String>, Header, description = "RFC 7240. `respond-async` answers 202 as soon as the vendor has the job; `wait=N` waits up to N seconds (at most the vendor timeout) and answers 202 if the job is still running then. Honoured only for a model whose vendor runs jobs, a request that keeps its output and isn't an edit litegen finishes itself; otherwise the call is synchronous (`Preference-Applied` absent). See docs/async-images.md."),
    ),
    responses(
        (status = 200, description = "Image generated successfully", body = ImageGenerationResponse,
            headers(
                ("X-Litegen-Dropped-Params" = String, description = "Comma-separated names of the fields and params this request sent that the route or model does not take, dropped instead of refused because `strict` was false; a field inside an object is named by its path (`reference_images[0].weight`). At most 32 names: any further name, and any name that is not 1–64 visible ASCII characters without a comma, is counted in a last `+N more`. Absent when nothing was dropped."),
                ("X-Litegen-Cache" = String, description = "What the cache did: `hit` (answered from it), `miss` (looked up, not found) or `bypass` (off for this model, or the request sent `Cache-Control: no-cache`). See docs/gateway-settings.md."),
                ("Idempotent-Replayed" = String, description = "`true` when this is the stored response of an earlier request with the same `Idempotency-Key`; nothing ran and nothing was billed. Absent otherwise."),
            )),
        (status = 400, description = "Bad request", body = ErrorResponse),
        (status = 404, description = "`model_not_found`", body = ErrorResponse),
        (status = 410, description = "`model_retired`: the model is past its retirement; `replaced_by` names its successor", body = ErrorResponse),
        (status = 422, description = "`region_too_small`: an edit region smaller than the vendor takes. A reference that names a generation litegen can't use (`reference_images`: unreadable, too large, a format the model doesn't take).", body = ErrorResponse),
        (status = 502, description = "Provider error", body = ErrorResponse),
        (status = 409, description = "`idempotency_key_reused`: the Idempotency-Key was used for a different request. `idempotency_request_in_progress`: a request with this key is still running (`Retry-After` is set).", body = ErrorResponse),
        (status = 202, description = "The vendor has the job and litegen delivers its image later (`Prefer: respond-async`, or a `wait` it outlasted): poll `GET /v1/generations/{id}` or wait for the key's webhook.", body = ImageGenerationAccepted,
            headers(
                ("Location" = String, description = "`/v1/generations/{id}`."),
                ("Preference-Applied" = String, description = "The preference honoured: `respond-async`, `wait=N`, or both."),
                ("X-Litegen-Dropped-Params" = String, description = "Comma-separated names of the fields and params this request sent that the route or model does not take, dropped instead of refused because `strict` was false; a field inside an object is named by its path (`reference_images[0].weight`). At most 32 names: any further name, and any name that is not 1–64 visible ASCII characters without a comma, is counted in a last `+N more`. Absent when nothing was dropped."),
            )),
        (status = 504, description = "`vendor_timeout`: the vendor did not answer in time. Never re-sent, since the vendor may still run and bill the job; `vendor_job_id` names it when the vendor had accepted one. When that job can still be delivered, `generation_id` names the generation it goes to and `status` is `processing`.", body = ErrorResponse),
        (status = 408, description = "`body_timeout`: the request body did not arrive within `server.request_body_timeout_secs` (default 300 s).", body = ErrorResponse),
        (status = 413, description = "`body_too_large`: the request body is over 25 MiB, as declared or as sent.", body = ErrorResponse),
        (status = 503, description = "`server_at_capacity`: litegen is running as many requests as it may (`backpressure.max_in_flight`), or holds as many request bodies as it may; retry shortly (`Retry-After` is set).", body = ErrorResponse),
    ),
    tag = "Images"
)]
pub async fn generate_image(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    attribution: crate::management::enforce::RequestAttribution,
    headers: axum::http::HeaderMap,
    validated: ValidatedImage,
) -> impl IntoResponse {
    // The in-flight slot was acquired in the Validated* extractor (before the
    // body was buffered) and is held for this request via `validated._permit`.

    let start = std::time::Instant::now();
    // Refused before anything runs, references included.
    let keep_header = match crate::access::keep_header(&headers) {
        Ok(h) => h,
        Err(e) => return keep_header_rejection(&e),
    };

    let refs = match resolve_generation_refs(&state, key_ctx.as_ref(), &validated.request.base.reference_images).await {
        Ok(r) => r,
        Err(e) => return reference_error_response(&e, &validated.schema.id),
    };
    let mut materialized = match state.materializer.materialize(
        &validated.schema,
        refs,
        &validated.ctx,
    ).await {
        Ok(m) => m,
        Err(e) => return validation_rejection_response(&e.to_string(), 400, &validated.schema.id),
    };
    // A partial edit is checked against its init image and turned into the
    // model's own form here, before anything is reserved or billed.
    let edit = match apply_image_edit(&validated.schema, &validated.request, &mut materialized, state.materializer.publisher()).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };

    let extras = ImageExtras {
        vendor_job: Default::default(),
        size: validated.request.size.clone(),
        aspect_ratio: validated.request.aspect_ratio.clone(),
        resolution: validated.request.resolution.clone(),
        quality: validated.request.quality.clone(),
        style: validated.request.style.clone(),
        steps: validated.request.steps,
        guidance_scale: validated.request.guidance_scale,
        strength: validated.request.strength,
        response_format: validated.request.response_format.clone(),
        views: match (&validated.request.views, validated.schema.params.get("views")) {
            (Some(v), _) => v.clone(),
            (None, Some(crate::capabilities::ParamSpec::StringArray(spec))) if !spec.default.is_empty() => spec.default.clone(),
            (None, Some(crate::capabilities::ParamSpec::StringArray(spec))) => spec.enum_values.clone(),
            _ => Vec::new(),
        },
        edit,
        expand: validated.request.expand,
        layout: validated.request.layout.clone(),
        mask_prompt: validated.request.mask_prompt.clone(),
        preserve_unmasked: validated.request.preserve_unmasked,
        feather_px: validated.request.feather_px,
        palette: validated.request.palette.clone(),
        directions: validated.request.directions,
        extra: validated.request.base.extra.clone(),
        output_format: validated.request.output_format.clone(),
        background: validated.request.background.clone(),
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
    // A model route's deployments get the org's credential for their own
    // providers, looked up the same way (BYO-ROUTE).
    let org_creds = match org_credentials_for(&state, &key_ctx, &validated.schema, app_creds.clone()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    // Keeping nothing (docs/storage-access.md): the image comes back inline,
    // and no store, cache or history copy sees it.
    let keep = crate::access::keeps_outputs(
        &crate::access::policy_for(&state, key_ctx.as_ref().and_then(|c| c.app_id.as_deref())).await,
        keep_header,
    );
    // Resolve the calling app's BYO image store (None → global store fallback).
    let app_store = if keep {
        resolve_app_image_store(&state, &key_ctx).await
    } else {
        Some(Arc::new(crate::proxy::storage::LocalStore) as Arc<dyn crate::proxy::storage::ImageStore>)
    };

    // Tenant scope for the (process-global) cache. Composed so distinct
    // principals can NEVER share a bucket: org+app keeps an app-scoped request
    // separate from a sibling app and from the org scope, and when no tenant is
    // resolved we fall back to the calling principal (key/user/master) instead
    // of a shared global bucket. See `cache_scope`.
    let cache_scope = match key_ctx.as_ref() {
        Some(ctx) => Some(cache_scope_for(ctx, crate::access::policy_for(&state, ctx.app_id.as_deref()).await)),
        None => None,
    };

    // Reserve the estimated cost against the key's quota BEFORE dispatch (the
    // real spend cap; see reserve_quota). Settled to the actual cost on success
    // and released on failure so a failed request is never billed.
    let charge_key = key_ctx.as_ref().and_then(|c| c.key_id);
    // This request's own price (3D doc §3G): what the quote said and the
    // charge will take.
    let price = charged_price(&state, crate::capabilities::pricing::request_price_usd(
        &validated.schema,
        &validated.request.base,
        &serde_json::to_value(&extras).unwrap_or_default(),
    ));
    let reserved = match reserve_quota(&state, charge_key, price).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    // Org/app/member/end-user/key limits (budgets, model allowlists, rpm and
    // parallel caps). Settled alongside the key quota below.
    let mut limits = match crate::management::enforce::begin(
        &state, key_ctx.as_ref(), &attribution, &validated.schema.id, price,
    ).await {
        Ok(g) => g,
        Err(resp) => {
            settle_quota(&state, reserved, 0.0).await;
            return resp;
        }
    };
    let attributed = limits.attribution();

    let log_metadata = validated.request.base.metadata.clone();
    // `Cache-Control: no-cache` / `no-store` opt this request out of the cache.
    let cache_policy = if keep {
        crate::proxy::cache::CachePolicy::from_headers(&headers)
    } else {
        crate::proxy::cache::CachePolicy::bypass()
    };
    // A job the vendor takes can be delivered late (docs/async-images.md)
    // when its adapter can ask about it again, the result is kept, and no
    // edit is left for litegen to finish. Only then does `Prefer:
    // respond-async` / `wait=N` (RFC 7240) shorten the wait; otherwise the
    // call is synchronous, as the RFC lets a server choose.
    let preserve = extras.edit.as_ref().is_some_and(|x| x.preserve.is_some());
    let late_ok = keep
        && !preserve
        && state
            .router
            .registry
            .image_provider_for_request(&validated.schema.provider, app_creds.clone())
            .await
            .is_some_and(|p| p.resumable(&validated.schema));
    let prefer = image_wait_preference(&headers).filter(|_| late_ok);
    let call = state.router.generate_image_cached(&validated.schema, &validated.request.base, &extras, &materialized, org_creds, app_store, cache_scope.as_deref(), cache_policy);
    let outcome = match prefer.as_ref() {
        Some((wait, _)) => crate::providers::with_wait(*wait, call).await,
        None => call.await,
    };
    match outcome {
        Ok((mut response, cache_outcome)) => {
            response.metadata = validated.request.base.metadata_map();
            let latency = start.elapsed().as_millis() as i64;
            let cost = response.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);

            // Settle the reservation to the actual cost.
            settle_quota(&state, reserved, cost).await;
            limits.settle(&state, cost).await;

            // Build artifact for drill-down storage
            let artifact = {
                let req_id = response.id.clone();
                let prompt = validated.request.base.prompt.clone();
                let neg = validated.request.base.negative_prompt.clone();
                let params = serde_json::to_value(&extras).ok();
                let refs_meta = build_refs_meta(&validated.request.base.reference_images);
                let first = response.data.first();
                let (output_kind, output_value, output_mime, output_truncated) = match first {
                    // Keeping nothing covers the request log too.
                    Some(img) if !keep => ("not_kept".to_string(), None, Some(img.content_type.clone()), false),
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

            // Keep the images and give the result a row (L3): a client can
            // then read it back by id, chain it as a reference, and download it
            // for 7 days with a litegen key. Awaited, so the row exists when the
            // response does; but the caller already holds the image, so a
            // failed write is logged rather than failing the request.
            // A cached answer is the earlier generation's: its row and files
            // are kept already, and its links come from them.
            let earlier = match cache_outcome {
                crate::proxy::cache::CacheOutcome::Hit => state.db.get_generation(&response.id).await.ok().flatten(),
                _ => None,
            };
            let kept = match earlier.as_ref() {
                Some(row) => generation_assets(row),
                None => keep_image_result(&state, key_ctx.as_ref(), &response, &validated, cost, attributed.end_user.as_deref(), keep).await,
            };
            // An image returned by URL (an S3 image store) goes out as the
            // app's access mode says; inline images are the caller's already.
            if response.data.iter().any(|d| d.url.is_some()) {
                let owner = earlier.as_ref().map_or_else(|| FileOwner::made_by(&response.id, key_ctx.as_ref()), FileOwner::of);
                let presenter = presenter_for(&state, key_ctx.as_ref(), &owner).await;
                for (i, image) in response.data.iter_mut().enumerate() {
                    let Some(stored) = image.url.clone() else { continue };
                    // The file route's index: its position among what was kept.
                    let index = kept.iter().position(|a| a.index as usize == i);
                    let (url, expires) = presenter.url(index, &stored).await;
                    image.url = Some(url);
                    image.expires_at = expires;
                }
            }

            // Log request + artifact async
            let db = state.db.clone();
            let id = response.id.clone();
            let model = response.model.clone();
            let provider = response.provider.clone();
            let org_id = key_ctx.as_ref().and_then(|c| c.org_id.clone());
            let app_id = key_ctx.as_ref().and_then(|c| c.app_id.clone());
            tokio::spawn(async move {
                let _ = db
                    .log_request(&id, &model, &provider, "completed", "image", cost, latency, None, log_metadata.as_ref(), org_id.as_deref(), app_id.as_deref())
                    .await;
                crate::management::enforce::attribute(&db, &id, &attributed).await;
                if let Err(e) = db.insert_request_artifact(&artifact).await {
                    tracing::warn!(error = %e, request_id = %artifact.request_id, "Failed to store request artifact");
                }
            });
            let mut resp = (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
            resp.extensions_mut().insert(crate::api::middleware::idempotency::VendorAttempted);
            resp.headers_mut().insert("x-litegen-cache", axum::http::HeaderValue::from_static(cache_outcome.as_str()));
            if let Some((k, v)) = dropped_header(&validated.dropped) {
                resp.headers_mut().insert(k, v);
            }
            resp
        }
        Err(e) => {
            // A job the vendor took and is still running is delivered later,
            // never lost to litegen's deadline: it gets a row the poller finishes.
            if let (crate::proxy::router::ProxyError::VendorTimeout { .. }, Some(job), true) = (&e, extras.vendor_job.get(), late_ok) {
                let late = LateImage { job, resume: extras.vendor_job.resume(), response_format: extras.response_format.clone(), price, prefer };
                return accept_late_image(&state, key_ctx.as_ref(), &validated, &extras, late, reserved, limits, attributed, start).await;
            }
            // Nothing was delivered — release the reservation so a failed
            // request is never billed.
            settle_quota(&state, reserved, 0.0).await;
            limits.settle(&state, 0.0).await;
            error!(error = %e, "Image generation failed");
            log_failed_generation(&state, key_ctx.as_ref(), FailedGeneration {
                id_prefix: "litegen-img",
                media_type: "image",
                schema: &validated.schema,
                latency_ms: start.elapsed().as_millis() as i64,
                error: &e.to_string(),
                prompt: &validated.request.base.prompt,
                metadata: log_metadata.as_ref(),
                attributed: &attributed,
            })
            .await;
            generation_error_response(&e)
        }
    }
}

// ─── Image Segmentation (PixelGenie L7) ──────────────────────────────────────

/// POST /v1/images/segmentations — Masks for what point, box or text prompts
/// select in an image, answered at once.
#[utoipa::path(
    post,
    path = "/v1/images/segmentations",
    request_body = ImageSegmentationRequest,
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "1–255 visible ASCII characters. The same key and the same request within 24 hours replay the first response (`Idempotent-Replayed: true`) and bill nothing; the same key with a different request is a 409. A request refused before any vendor was called does not spend the key."),
    ),
    responses(
        (status = 200, description = "What the prompts selected: one mask per object, best first, billed once per request at the row's price", body = ImageSegmentationResponse,
            headers(
                ("Idempotent-Replayed" = String, description = "`true` when this is the stored response of an earlier request with the same `Idempotency-Key`; nothing ran and nothing was billed. Absent otherwise."),
                ("X-Litegen-Dropped-Params" = String, description = "Comma-separated names of the fields and params this request sent that the route or model does not take, dropped instead of refused because `strict` was false; a field inside an object is named by its path (`reference_images[0].weight`). At most 32 names: any further name, and any name that is not 1–64 visible ASCII characters without a comma, is counted in a last `+N more`. Absent when nothing was dropped."),
            )),
        (status = 400, description = "Refused before anything is billed. `param_unsupported`: a field the request has no such name for (`prompt` for `prompts`, say), when `strict` is true (the default). `param_required`: no prompts, or an empty text prompt. `capability_unsupported`: a prompt type the model does not take (its `segmentation_prompts`). `param_out_of_range`: more prompts than the model takes, more than one text prompt, a point label other than 0 or 1, or a box with no area. `param_conflict`: a text prompt mixed with point or box prompts, or only label-0 points. `prompt_out_of_bounds`: a point or box outside the image. `image_unreadable`: not base64 of a PNG, JPEG, WebP or GIF. `model_task_mismatch`: not a segmentation model. `model_media_type_mismatch`: not an image model.", body = ErrorResponse),
        (status = 404, description = "`model_not_found`", body = ErrorResponse),
        (status = 410, description = "`model_retired`: the model is past its retirement; `replaced_by` names its successor", body = ErrorResponse),
        (status = 409, description = "`idempotency_key_reused`: the Idempotency-Key was used for a different request. `idempotency_request_in_progress`: a request with this key is still running (`Retry-After` is set).", body = ErrorResponse),
        (status = 402, description = "`quota_exceeded` / `budget_exceeded`: the key's quota or a budget it counts against is spent.", body = ErrorResponse),
        (status = 429, description = "`rate_limit_exceeded`: the key's requests per minute (`Retry-After` is set); or the vendor's own rate limit, retryable.", body = ErrorResponse),
        (status = 503, description = "litegen is at capacity (`server_at_capacity`: as many requests running as `backpressure.max_in_flight` allows, or as many request bodies held as it may; `Retry-After` is set), or the app's bucket can't be reached; retry shortly.", body = ErrorResponse),
        (status = 408, description = "`body_timeout`: the request body did not arrive within `server.request_body_timeout_secs` (default 300 s).", body = ErrorResponse),
        (status = 413, description = "`body_too_large`: the request body is over 25 MiB, as declared or as sent.", body = ErrorResponse),
        (status = 502, description = "Provider error", body = ErrorResponse),
        (status = 504, description = "`vendor_timeout`: the vendor did not answer in time. Never re-sent.", body = ErrorResponse),
    ),
    tag = "Images"
)]
pub async fn segment_image(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    attribution: crate::management::enforce::RequestAttribution,
    validated: crate::api::middleware::validator::ValidatedSegmentation,
) -> impl IntoResponse {
    let start = std::time::Instant::now();
    let schema = &validated.schema;
    let app_creds = match resolve_org_provider_credential(&state, &key_ctx, &schema.provider).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if app_creds.is_none() && !state.router.has_image_provider(&schema.provider).await {
        return provider_not_configured_response(&schema.provider);
    }
    // Reserved at the row's price before the vendor is called; settled to the
    // charge, or released when nothing came back (as generate_image does).
    let charge_key = key_ctx.as_ref().and_then(|c| c.key_id);
    let price = charged_price(&state, crate::capabilities::pricing::row_price_usd(schema));
    let reserved = match reserve_quota(&state, charge_key, price).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let mut limits = match crate::management::enforce::begin(&state, key_ctx.as_ref(), &attribution, &schema.id, price).await {
        Ok(g) => g,
        Err(resp) => {
            settle_quota(&state, reserved, 0.0).await;
            return resp;
        }
    };
    let attributed = limits.attribution();
    let result = state.router.segment_image(schema, &validated.input, app_creds).await;
    let latency = start.elapsed().as_millis() as i64;
    let (org_id, app_id) = (key_ctx.as_ref().and_then(|c| c.org_id.clone()), key_ctx.as_ref().and_then(|c| c.app_id.clone()));
    let db = state.db.clone();
    let log_metadata = validated.metadata.clone();
    match result {
        Ok(mut response) => {
            response.metadata = log_metadata.clone().and_then(|m| serde_json::from_value(m).ok());
            let cost = response.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);
            settle_quota(&state, reserved, cost).await;
            limits.settle(&state, cost).await;
            let (id, model, provider) = (response.id.clone(), response.model.clone(), response.provider.clone());
            tokio::spawn(async move {
                let _ = db
                    .log_request(&id, &model, &provider, "completed", "image", cost, latency, None, log_metadata.as_ref(), org_id.as_deref(), app_id.as_deref())
                    .await;
                crate::management::enforce::attribute(&db, &id, &attributed).await;
            });
            let mut resp = (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
            resp.extensions_mut().insert(crate::api::middleware::idempotency::VendorAttempted);
            if let Some((k, v)) = dropped_header(&validated.dropped) {
                resp.headers_mut().insert(k, v);
            }
            resp
        }
        Err(e) => {
            settle_quota(&state, reserved, 0.0).await;
            limits.settle(&state, 0.0).await;
            error!(error = %e, "Image segmentation failed");
            let (model, provider, err_msg) = (schema.id.clone(), schema.provider.clone(), e.to_string());
            tokio::spawn(async move {
                let id = format!("litegen-seg-{}", Uuid::new_v4());
                let _ = db
                    .log_request(&id, &model, &provider, "failed", "image", 0.0, latency, Some(&err_msg), log_metadata.as_ref(), org_id.as_deref(), app_id.as_deref())
                    .await;
                crate::management::enforce::attribute(&db, &id, &attributed).await;
            });
            generation_error_response(&e)
        }
    }
}

/// `resp` with `X-Litegen-Dropped-Params` naming `dropped`, when anything
/// was (a quote drops what its generation would, N1).
fn with_dropped(mut resp: axum::response::Response, dropped: &[String]) -> axum::response::Response {
    if let Some((k, v)) = dropped_header(dropped) {
        resp.headers_mut().insert(k, v);
    }
    resp
}

/// POST /v1/images/cost — Estimate cost for image generation.
#[utoipa::path(
    post,
    path = "/v1/images/cost",
    request_body = ImageGenerationRequest,
    responses(
        (status = 200, description = "Cost estimate", body = CostEstimate,
            headers(
                ("X-Litegen-Dropped-Params" = String, description = "Comma-separated names of the fields and params this request sent that the route or model does not take, dropped instead of refused because `strict` was false; a field inside an object is named by its path (`reference_images[0].weight`). At most 32 names: any further name, and any name that is not 1–64 visible ASCII characters without a comma, is counted in a last `+N more`. Absent when nothing was dropped."),
            )),
        (status = 400, description = "Bad request (validation failed)", body = ErrorResponse),
        (status = 408, description = "`body_timeout`: the request body did not arrive within `server.request_body_timeout_secs` (default 300 s).", body = ErrorResponse),
        (status = 413, description = "`body_too_large`: the request body is over 25 MiB, as declared or as sent.", body = ErrorResponse),
        (status = 503, description = "`server_at_capacity`: litegen is running as many requests as it may (`backpressure.max_in_flight`), or holds as many request bodies as it may; retry shortly (`Retry-After` is set).", body = ErrorResponse),
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
        Ok(estimate) => with_dropped((StatusCode::OK, Json(serde_json::to_value(estimate).unwrap())).into_response(), &validated.dropped),
        Err(e) => {
            generation_error_response(&e)
        }
    }
}

// ─── Video Generation ───────────────────────────────────────────────────────

/// POST /v1/videos/generations — Start video generation.
#[utoipa::path(
    post,
    path = "/v1/videos/generations",
    request_body = VideoGenerationRequest,
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "1–255 visible ASCII characters. The same key and the same request within 24 hours replay the first response (`Idempotent-Replayed: true`) and bill nothing; the same key with a different request is a 409. A request refused before any vendor was called does not spend the key."),
        ("X-Litegen-Keep-Outputs" = Option<String>, Header, description = "`false` keeps nothing for this request: no copy of the output is stored anywhere, and the answer carries the image itself or the vendor's own link (docs/storage-access.md). A request can keep less than its app does, never more; any other value is a 400 `invalid_keep_outputs_header`."),
    ),
    responses(
        (status = 200, description = "Video generation started", body = VideoGenerationResponse,
            headers(
                ("X-Litegen-Dropped-Params" = String, description = "Comma-separated names of the fields and params this request sent that the route or model does not take, dropped instead of refused because `strict` was false; a field inside an object is named by its path (`reference_images[0].weight`). At most 32 names: any further name, and any name that is not 1–64 visible ASCII characters without a comma, is counted in a last `+N more`. Absent when nothing was dropped."),
                ("Idempotent-Replayed" = String, description = "`true` when this is the stored response of an earlier request with the same `Idempotency-Key`; nothing ran and nothing was billed. Absent otherwise."),
            )),
        (status = 400, description = "Bad request", body = ErrorResponse),
        (status = 404, description = "`model_not_found`", body = ErrorResponse),
        (status = 410, description = "`model_retired`: the model is past its retirement; `replaced_by` names its successor", body = ErrorResponse),
        (status = 422, description = "A `source_video` or reference generation litegen can't use: unreadable, too large, or a format the model doesn't take.", body = ErrorResponse),
        (status = 409, description = "`idempotency_key_reused`: the Idempotency-Key was used for a different request. `idempotency_request_in_progress`: a request with this key is still running (`Retry-After` is set).", body = ErrorResponse),
        (status = 504, description = "`vendor_timeout`: the vendor did not answer in time. Never re-sent, since the vendor may still run and bill the job; `vendor_job_id` names it when the vendor had accepted one.", body = ErrorResponse),
        (status = 500, description = "`generation_record_failed`: the vendor accepted the job but litegen could not record it, so it has no id to poll; `vendor_job_id` names the vendor's job. A retry with the same `Idempotency-Key` gets this answer, not a second job.", body = ErrorResponse),
        (status = 408, description = "`body_timeout`: the request body did not arrive within `server.request_body_timeout_secs` (default 300 s).", body = ErrorResponse),
        (status = 413, description = "`body_too_large`: the request body is over 25 MiB, as declared or as sent.", body = ErrorResponse),
        (status = 503, description = "`server_at_capacity`: litegen is running as many requests as it may (`backpressure.max_in_flight`), or holds as many request bodies as it may; retry shortly (`Retry-After` is set).", body = ErrorResponse),
    ),
    tag = "Videos"
)]
pub async fn generate_video(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    attribution: crate::management::enforce::RequestAttribution,
    headers: axum::http::HeaderMap,
    validated: ValidatedVideo,
) -> impl IntoResponse {
    // The in-flight slot was acquired in the Validated* extractor (before the
    // body was buffered) and is held for this request via `validated._permit`.

    let start = std::time::Instant::now();
    // Refused before anything runs. The decision is taken now and saved on
    // the row, so whichever observer sees the video finish honours it.
    let keep_header = match crate::access::keep_header(&headers) {
        Ok(h) => h,
        Err(e) => return keep_header_rejection(&e),
    };
    let keep = crate::access::keeps_outputs(
        &crate::access::policy_for(&state, key_ctx.as_ref().and_then(|c| c.app_id.as_deref())).await,
        keep_header,
    );

    let refs = match resolve_generation_refs(&state, key_ctx.as_ref(), &validated.request.base.reference_images).await {
        Ok(r) => r,
        Err(e) => return reference_error_response(&e, &validated.schema.id),
    };
    let materialized = match state.materializer.materialize(
        &validated.schema,
        refs,
        &validated.ctx,
    ).await {
        Ok(m) => m,
        Err(e) => return validation_rejection_response(&e.to_string(), 400, &validated.schema.id),
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
    // A model route's deployments get the org's credential for their own
    // providers, looked up the same way (BYO-ROUTE).
    let org_creds = match org_credentials_for(&state, &key_ctx, &validated.schema, app_creds.clone()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    // Keeping nothing hands out the vendor's own link: a model whose vendor
    // returns only bytes (or a link only it can read) has none, so the
    // request is refused now, before the vendor runs and bills it.
    if !keep
        && state
            .router
            .registry
            .video_provider_for_request(&validated.schema.provider, app_creds.clone())
            .await
            .is_some_and(|p| !p.hands_out_links(&validated.schema))
    {
        return validation_rejection_with_code(
            "keep_outputs_unsupported",
            &format!(
                "'{}' returns its video as bytes, with no link litegen could hand out: keeping nothing needs a model that hands out the vendor's link",
                validated.schema.id
            ),
            &validated.schema.id,
        );
    }
    // Which vendor account the job runs on: a vendor that extends only its
    // own generations takes only those made on this account (3D doc §5).
    let vendor_account = match (&app_creds, key_ctx.as_ref().and_then(|c| c.org_id.as_deref())) {
        (Some(_), Some(org)) => format!("org:{org}"),
        _ => "platform".to_string(),
    };

    // 3D doc §5: a video operation's source, read and measured before
    // anything is reserved.
    let source = match validated.request.source_video.as_ref() {
        Some(src) => match resolve_source_video(&state, key_ctx.as_ref(), &validated.schema, src, validated.request.time_range, &vendor_account).await {
            Ok(s) => Some(s),
            Err(resp) => return resp,
        },
        None => None,
    };
    let extras = VideoExtras {
        source,
        direction: validated.request.direction.clone(),
        time_range: validated.request.time_range,
        replace: validated.request.replace.clone(),
        duration_seconds: crate::api::middleware::validator::resolve_duration_seconds(
            &validated.schema,
            validated.request.duration_seconds,
        ),
        aspect_ratio: validated.request.aspect_ratio.clone(),
        resolution: validated.request.resolution.clone(),
        fps: validated.request.fps,
        extra: validated.request.base.extra.clone(),

        camera: validated.request.camera.clone(),
        audio: validated.request.audio,
        r#loop: validated.request.r#loop,
    };

    // Reserve the estimated cost against the key's quota BEFORE dispatch, then
    // settle to the actual cost on success / release on failure.
    let charge_key = key_ctx.as_ref().and_then(|c| c.key_id);
    // This request's own price (3D doc §3G): what the quote said and the
    // charge will take.
    let price = charged_price(&state, crate::capabilities::pricing::request_price_usd(
        &validated.schema,
        &validated.request.base,
        &serde_json::to_value(&extras).unwrap_or_default(),
    ));
    let reserved = match reserve_quota(&state, charge_key, price).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    // Org/app/member/end-user/key limits (budgets, model allowlists, rpm and
    // parallel caps). Settled alongside the key quota below.
    let mut limits = match crate::management::enforce::begin(
        &state, key_ctx.as_ref(), &attribution, &validated.schema.id, price,
    ).await {
        Ok(g) => g,
        Err(resp) => {
            settle_quota(&state, reserved, 0.0).await;
            return resp;
        }
    };
    let attributed = limits.attribution();

    let segments = plan_segments(&validated.schema, &extras);
    // A job the router can't record is cancelled with the credential of the
    // provider that took it (a route's deployment may be another vendor).
    let cancel_creds_by_provider = org_creds.clone();
    match state.router.generate_video(&validated.schema, &validated.request.base, &extras, &materialized, org_creds).await {
        Ok(mut response) => {
            response.segments = segments.clone();
            response.metadata = validated.request.base.metadata_map();
            let latency = start.elapsed().as_millis() as i64;
            let cost = response.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);

            // Who it was charged against goes on the row, for a refund if the
            // job fails (L18). The reservation is settled with the row below.
            let charged_to = limits.charged_to();

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
            let request = request_record(&validated.request.base, serde_json::to_value(&extras).ok(), &validated.ctx);
            let log_metadata = validated.request.base.metadata.clone();

            // The row is written BEFORE the response (L2): the caller learns the
            // id only from it and polls `GET /v1/generations/{id}` at once, and
            // after a restart. Run in a task the handler awaits, so a client that
            // disconnects now cannot cancel the writes for a job the vendor took.
            let vendor_job = provider_job_id_for_insert.clone();
            let (cancel_provider, cancel_model) = (provider.clone(), model.clone());
            let cancel_creds = cancel_creds_by_provider.get(&provider);
            let writes = {
                let state = state.clone();
                let db = db.clone();
                let id = id.clone();
                tokio::spawn(async move {
                    if let Err(e) = db
                        .log_request(&id, &model, &provider, "pending", "video", cost, latency, None, log_metadata.as_ref(), org_id.as_deref(), app_id.as_deref())
                        .await
                    {
                        tracing::warn!(generation_id = %id, error = %e, "failed to log the video request");
                    }
                    crate::management::enforce::attribute(&db, &id, &attributed).await;
                    let inserted = db.insert_generation(
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
                    // Settled to the actual cost once the job has its row; the
                    // caller of a job litegen could not record is told it
                    // failed, so it is charged nothing and its log says so.
                    let charge = if inserted.is_ok() { cost } else { 0.0 };
                    settle_quota(&state, reserved, charge).await;
                    limits.settle(&state, charge).await;
                    if let Err(e) = inserted {
                        if let Err(e) = db.update_request_log_status(&id, "failed", Some("generation_record_failed")).await {
                            tracing::warn!(generation_id = %id, error = %e, "failed to mark the unrecorded video request failed");
                        }
                        return Err(e);
                    }
                    // `end_user`: who it was made for, which `file_access` hooks compare.
                    let mut recorded = serde_json::json!({ "request": request, "billing": { "limits": charged_to }, "vendor_account": vendor_account, "end_user": attributed.end_user });
                    if !keep {
                        recorded["keep_outputs"] = false.into();
                    }
                    if let Some(segments) = &segments {
                        recorded["segments"] = serde_json::json!(segments);
                    }
                    if let Err(e) = db.merge_generation_metadata_if_active(&id, &recorded).await {
                        tracing::warn!(generation_id = %id, error = %e, "failed to record the video request on its row");
                    }
                    Ok::<(), sqlx::Error>(())
                })
            };
            let failure = match writes.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(format!("{e}")),
                Err(e) => Some(format!("{e}")),
            };
            if let Some(err) = failure {
                tracing::error!(generation_id = %id, error = %err, "video row could not be written; failing the submit");
                // Nothing will ever poll this job: let it go here, and stop it
                // at the vendor (best effort), as the 3D path does.
                state.router.forget_video_job(&id).await;
                if let Some(job) = vendor_job.clone() {
                    let state = state.clone();
                    tokio::spawn(async move {
                        let Some(vendor) = state.router.registry.video_provider_for_request(&cancel_provider, cancel_creds).await else { return };
                        let handle = crate::providers::VideoGenerationHandle { provider_job_id: job, provider: cancel_provider, model: cancel_model };
                        if let Err(e) = vendor.cancel(&handle).await {
                            tracing::warn!(error = %e, "could not cancel an unrecorded video job at the vendor");
                        }
                    });
                }
                return generation_record_failed_response(&id, vendor_job.as_deref());
            }
            // The artifact is diagnostic only, so it alone stays off the response path.
            tokio::spawn(async move {
                if let Err(e) = db.insert_request_artifact(&video_artifact).await {
                    tracing::warn!(error = %e, request_id = %video_artifact.request_id, "Failed to store video artifact");
                }
            });
            let mut resp = (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
            resp.extensions_mut().insert(crate::api::middleware::idempotency::VendorAttempted);
            if let Some((k, v)) = dropped_header(&validated.dropped) {
                resp.headers_mut().insert(k, v);
            }
            resp
        }
        Err(e) => {
            // Nothing was delivered — release the reservation.
            settle_quota(&state, reserved, 0.0).await;
            limits.settle(&state, 0.0).await;
            error!(error = %e, "Video generation failed");
            log_failed_generation(&state, key_ctx.as_ref(), FailedGeneration {
                id_prefix: "litegen-vid",
                media_type: "video",
                schema: &validated.schema,
                latency_ms: start.elapsed().as_millis() as i64,
                error: &e.to_string(),
                prompt: &validated.request.base.prompt,
                metadata: validated.request.base.metadata.as_ref(),
                attributed: &attributed,
            })
            .await;
            generation_error_response(&e)
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
    // Read once: a row that cannot be read is an error, never a pass to the
    // router's in-flight job, which may be another tenant's.
    let row = match state.db.get_generation(&id).await {
        Ok(row) => row,
        Err(e) => {
            error!(generation_id = %id, error = %e, "could not read the video row");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response("the generation could not be read; retry shortly", 500))).into_response();
        }
    };
    if let Some(gen) = row.as_ref() {
        if gen.org_id.as_deref() != Some(ctx_org) {
            return (StatusCode::NOT_FOUND, Json(error_response("Not found", 404))).into_response();
        }
        // Read:own boundary (same as get_generation) when the row is persisted.
        if let Err(resp) = authorize_generation_for_session(
            &state, key_ctx.as_ref(), gen,
            crate::auth::permissions::Permission::GenerationReadAny,
            crate::auth::permissions::Permission::GenerationReadOwn,
            "generation:read:own",
        ).await {
            return resp;
        }
        // A terminal row is the decided outcome: answer from it. A cancel drops
        // the router's job, so without this every poll after a cancel answered
        // 404 "video job not found" for a generation that exists and is
        // `cancelled`. Release the router entry too, as the 3D path does.
        if gen.media_type == "video" && gen.status.is_terminal() {
            state.router.forget_video_job(&id).await;
            return answer_video(&state, key_ctx.as_ref(), gen).await;
        }
    }
    // Kept or not as the request decided at submit (`metadata.keep_outputs`).
    let keep = crate::access::row_keeps_outputs(row.as_ref().and_then(|g| g.metadata.as_ref()));
    // Keep a finished video in the tenant's own bucket, as the poller does.
    let app_id = row.as_ref()
        .and_then(|g| g.app_id.clone())
        .or_else(|| key_ctx.as_ref().and_then(|c| c.app_id.clone()));
    let (app_store, app_prefix) = match app_id.as_deref().filter(|_| keep) {
        Some(app_id) => match app_store_for_writing(&state.db, state.secrets_key, &state.router.config.aws, tenant_bucket_of(&state), app_id).await {
            Ok(found) => found.map_or((None, None), |(s, p)| (Some(s), p)),
            // Its bucket is down and litegen's store may not stand in: the
            // video waits, as the row says.
            Err(reason) => {
                tracing::warn!(generation_id = %id, reason = %reason, "a finished video has nowhere to be kept yet");
                return match row.as_ref() {
                    Some(gen) => answer_video(&state, key_ctx.as_ref(), gen).await,
                    None => (StatusCode::SERVICE_UNAVAILABLE, Json(error_response("the app's bucket can't be reached; retry shortly", 503))).into_response(),
                };
            }
        },
        None => (None, None),
    };
    // A finished video is kept by one observer: the claim, when this poll has
    // it, is held until the outcome below is written.
    match state.router.get_video_status_claimed(&id, app_store, app_prefix.as_deref(), keep, &state.db).await {
        Ok((mut resp, assets, _claim)) => {
            // `created` is when the job was submitted, on every read: the row's;
            // an operation's segments are its plan, recorded at submit.
            if let Some(row) = row.as_ref() {
                resp.created = row.created_at.timestamp();
                resp.segments = row.metadata.as_ref().and_then(|m| serde_json::from_value(m.get("segments")?.clone()).ok());
                resp.metadata = row.request_metadata();
            }
            // A terminal answer is persisted here as well as by the poller, so
            // it reads the same however often it is read: the router has let
            // go of the job, and the row is what answers from now on (L3).
            if resp.status.is_terminal() {
                let written = state.db.update_generation_if_active(
                    &id, &resp.status.to_string(), resp.progress as i32,
                    resp.video_url.as_deref(), resp.error.as_deref(), Some(chrono::Utc::now()),
                    assets.as_ref(),
                ).await;
                match written {
                    Ok(crate::db::GenerationWrite::Written) => {
                        let refunded = crate::proxy::poller::settle_terminal(&state.db, &id, &resp.status.to_string(), resp.error.as_deref()).await;
                        if refunded > 0.0 {
                            resp.usage = Some(usage_from_row_cost(0.0));
                        }
                        if let Some(row) = row.as_ref() {
                            let gen = crate::types::Generation {
                                status: resp.status,
                                progress: resp.progress as i32,
                                result_url: resp.video_url.clone(),
                                error_message: resp.error.clone(),
                                completed_at: Some(chrono::Utc::now()),
                                metadata: assets.clone().or_else(|| row.metadata.clone()),
                                ..row.clone()
                            };
                            crate::proxy::poller::dispatch_terminal_webhook(
                                &state.db, &crate::proxy::webhook::webhook_client(state.mode), state.mode, gen,
                                Some(crate::access::present::Presenting::of(&state)),
                            ).await;
                        }
                    }
                    Ok(crate::db::GenerationWrite::AlreadyTerminal) => {
                        // Another observer decided first: that is the answer.
                        if let Ok(Some(gen)) = state.db.get_generation(&id).await {
                            return answer_video(&state, key_ctx.as_ref(), &gen).await;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(generation_id = %id, error = %e, "failed to persist the video outcome"),
                }
            }
            if resp.video_url.is_some() {
                let owner = row.as_ref().map_or_else(|| FileOwner::made_by(&id, key_ctx.as_ref()), FileOwner::of);
                let kept: Vec<crate::types::GenerationAsset> = assets.as_ref()
                    .and_then(|m| m.get("assets"))
                    .and_then(|a| serde_json::from_value(a.clone()).ok())
                    .unwrap_or_default();
                presenter_for(&state, key_ctx.as_ref(), &owner).await.present_video(&mut resp, &kept).await;
            }
            (StatusCode::OK, Json(serde_json::to_value(resp).unwrap())).into_response()
        }
        Err(e) => {
            if let Some(gen) = row.filter(|g| g.media_type == "video") {
                return answer_video(&state, key_ctx.as_ref(), &gen).await;
            }
            generation_error_response(&e)
        }
    }
}

/// A presenter for `owner`'s files as the caller may use them
/// (`docs/storage-access.md`).
async fn presenter_for(state: &AppState, key_ctx: Option<&KeyContext>, owner: &FileOwner) -> crate::access::present::Presenter {
    crate::access::present::Presenting::of(state).presenter(owner, crate::access::present::Viewer::Caller(key_ctx)).await
}

/// A video generation answered from its row, its URL as the caller may use it.
async fn answer_video(state: &AppState, key_ctx: Option<&KeyContext>, gen: &crate::types::Generation) -> axum::response::Response {
    let mut resp = video_response_from_row(gen);
    if resp.video_url.is_some() {
        presenter_for(state, key_ctx, &FileOwner::of(gen)).await.present_video(&mut resp, &generation_assets(gen)).await;
    }
    (StatusCode::OK, Json(serde_json::to_value(resp).unwrap())).into_response()
}

/// A 3D answer, its files' URLs as the caller may use them.
async fn answer_3d(state: &AppState, key_ctx: Option<&KeyContext>, owner: FileOwner, mut resp: Model3dGenerationResponse) -> axum::response::Response {
    if !resp.assets.is_empty() {
        presenter_for(state, key_ctx, &owner).await.present_3d(&mut resp).await;
    }
    (StatusCode::OK, Json(serde_json::to_value(resp).unwrap())).into_response()
}

/// Generation rows, their files' URLs as the caller may use them.
async fn present_rows(state: &AppState, key_ctx: Option<&KeyContext>, rows: &mut [crate::types::Generation]) {
    let presenting = crate::access::present::Presenting::of(state);
    // Each app's policy and stores are resolved once for the whole page.
    let mut cache = crate::access::present::PresenterCache::default();
    for gen in rows.iter_mut() {
        presenting
            .presenter_cached(&FileOwner::of(gen), crate::access::present::Viewer::Caller(key_ctx), &mut cache)
            .await
            .present_row(gen)
            .await;
    }
}

/// A video generation answered from its row: the decided outcome, once the
/// router has let go of the job.
fn video_response_from_row(gen: &crate::types::Generation) -> VideoGenerationResponse {
    VideoGenerationResponse {
        video_url_expires_at: None,
        id: gen.id.clone(),
        status: gen.status,
        model: gen.model.clone(),
        provider: gen.provider.clone(),
        video_url: gen.result_url.clone(),
        progress: gen.progress.clamp(0, 100) as u8,
        error: gen.error_message.clone(),
        usage: Some(usage_from_row_cost(gen.cost_usd)),
        created: gen.created_at.timestamp(),
        media: gen.metadata.as_ref().and_then(crate::types::VideoMedia::from_assets),
        segments: gen.metadata.as_ref().and_then(|m| serde_json::from_value(m.get("segments")?.clone()).ok()),
        metadata: gen.request_metadata(),
    }
}

/// What a video operation's finished clip will hold, stretch by stretch,
/// from the request and its source (3D doc §5): litegen splices nothing, so
/// this is what the model's documented behaviour makes.
fn plan_segments(schema: &crate::capabilities::ModelSchema, extras: &VideoExtras) -> Option<Vec<crate::types::VideoSegment>> {
    use crate::capabilities::{ExtendOutput, ModelTask, ParamSpec};
    use crate::types::{SegmentOrigin::*, VideoSegment};
    let source = extras.source.as_ref()?;
    let d = source.duration_s;
    let seg = |start_s: f64, end_s: f64, origin| VideoSegment { start_s, end_s, origin };
    let window = |r: crate::types::TimeRange| {
        [seg(0.0, r.start_s, Source), seg(r.start_s, r.end_s, Generated), seg(r.end_s, d, Source)]
            .into_iter()
            .filter(|s| s.end_s > s.start_s)
            .collect::<Vec<_>>()
    };
    match schema.task {
        ModelTask::Extend => {
            let add = extras.duration_seconds;
            let full = !matches!(schema.params.get("source_video"), Some(ParamSpec::SourceVideo(s)) if s.extend_output == Some(ExtendOutput::Added));
            Some(match (full, extras.direction.as_deref()) {
                (false, _) => vec![seg(0.0, add, Generated)],
                (true, Some("backward")) => vec![seg(0.0, add, Generated), seg(add, add + d, Source)],
                (true, _) => vec![seg(0.0, d, Source), seg(d, d + add, Generated)],
            })
        }
        ModelTask::Retake => extras.time_range.map(window),
        ModelTask::VideoEdit => Some(extras.time_range.map(window).unwrap_or_else(|| vec![seg(0.0, d, Generated)])),
        _ => None,
    }
}

/// Read a video operation's `source_video` (3D doc §5): a generation after
/// the same access check `GET /v1/generations/{id}` makes, a URL fetched as a
/// public one, or base64; then measured, and refused before billing when the
/// model cannot take it — another account's clip where only the vendor's own
/// chain (`source_not_chainable`), a clip litegen cannot read or longer than
/// the model takes, or a `time_range` past its end.
async fn resolve_source_video(
    state: &Arc<AppState>,
    key_ctx: Option<&crate::api::middleware::KeyContext>,
    schema: &crate::capabilities::ModelSchema,
    src: &crate::types::SourceRef,
    time_range: Option<crate::types::TimeRange>,
    vendor_account: &str,
) -> Result<crate::providers::VideoSource, axum::response::Response> {
    use crate::proxy::model3d_source::SourceError;
    use crate::types::SourceKind;
    let refuse = |e: SourceError| {
        let status = StatusCode::from_u16(e.status()).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY);
        (status, Json(crate::api::middleware::validator::err_body(e.code(), &e.message(), Some("source_video"), &schema.id))).into_response()
    };
    let spec = match schema.params.get("source_video") {
        Some(crate::capabilities::ParamSpec::SourceVideo(s)) => s,
        _ => return Err(refuse(SourceError::FormatUnsupported("this model takes no source_video".into()))),
    };
    let not_chainable = |msg: String| {
        (StatusCode::BAD_REQUEST, Json(crate::api::middleware::validator::err_body("source_not_chainable", &msg, Some("source_video"), &schema.id))).into_response()
    };
    // A vendor that works only on its own earlier generations takes no URL
    // or upload: refused before anything is fetched.
    if spec.own_generations_only && src.kind != SourceKind::Generation {
        return Err(not_chainable(format!("'{}' works only on its own vendor's generations, made on this account: send one's generation id", schema.id)));
    }
    let (bytes, content_type, vendor_job_id) = match src.kind {
        SourceKind::Generation => {
            let gen = chainable_generation(state, key_ctx, &src.value)
                .await
                .ok_or_else(|| refuse(SourceError::NotFound(format!("generation {} is not one you can read", src.value))))?;
            if gen.media_type != "video" {
                return Err(refuse(SourceError::NotFound(format!("generation {} is not a video", gen.id))));
            }
            if gen.status != GenerationStatus::Completed {
                return Err(refuse(SourceError::NotReady(format!("generation {} has not finished", gen.id))));
            }
            // The account that made it: rows from before it was recorded ran
            // on the platform's.
            let made_on = gen.metadata.as_ref().and_then(|m| m.get("vendor_account")).and_then(|v| v.as_str()).unwrap_or("platform");
            let own = gen.provider == schema.provider && made_on == vendor_account;
            if spec.own_generations_only && !own {
                return Err(not_chainable(format!(
                    "'{}' works only on its own vendor's generations, made on this account; {} was made by {} on another",
                    schema.id, gen.id, gen.provider
                )));
            }
            let asset = generation_assets(&gen)
                .into_iter()
                .find(|a| a.kind == "video")
                .ok_or_else(|| refuse(SourceError::NotFound(format!("generation {} has no stored video", gen.id))))?;
            let mut stores = vec![state.router.model3d_store.clone()];
            if let Some(app) = gen.app_id.as_deref() {
                if let Some((store, _)) = resolve_app_model3d_store(&state.db, state.secrets_key, &state.router.config.aws, tenant_bucket_of(state), app).await {
                    stores.insert(0, store);
                }
            }
            let mut found = None;
            for store in &stores {
                if let Some(key) = store.key_of(&asset.url) {
                    found = store.get(&key).await.ok().flatten();
                    break;
                }
            }
            let (bytes, ct) = found.ok_or_else(|| refuse(SourceError::NotFound(format!("generation {}'s video is no longer stored", gen.id))))?;
            let vendor_job = own.then(|| gen.provider_job_id.clone()).flatten();
            (bytes, ct, vendor_job)
        }
        SourceKind::Base64 => {
            use base64::Engine;
            let b64 = src.value.split_once(";base64,").map(|(_, b)| b).unwrap_or(&src.value);
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| refuse(SourceError::FormatUnsupported(format!("source_video is not base64: {e}"))))?;
            (bytes::Bytes::from(bytes), video_content_type(src.format.as_deref()), None)
        }
        SourceKind::Url => {
            crate::util::ssrf::validate_public_url(&src.value).await.map_err(|e| refuse(SourceError::Fetch(e)))?;
            let client = crate::util::ssrf::public_only_download_client(std::time::Duration::from_secs(120), std::time::Duration::from_secs(30));
            let (bytes, _) = crate::util::ssrf::download_capped(&client, &src.value, crate::proxy::model3d_source::MAX_SOURCE_BYTES)
                .await
                .map_err(|e| refuse(crate::proxy::model3d_source::fetch_error(e)))?;
            (bytes::Bytes::from(bytes), video_content_type(src.format.as_deref()), None)
        }
    };
    let media = crate::proxy::video_probe::probe(&bytes);
    let duration_s = media
        .duration_s
        .filter(|d| *d > 0.0)
        .ok_or_else(|| refuse(SourceError::FormatUnsupported("litegen cannot read the source clip's length".into())))?;
    if let Some(max) = spec.max_duration_s.filter(|max| duration_s > *max + 1e-6) {
        return Err(refuse(SourceError::FormatUnsupported(format!("the source is {duration_s} s; this model takes at most {max} s"))));
    }
    if let Some(r) = time_range.filter(|r| r.end_s > duration_s + 1e-6) {
        let msg = format!("time_range ends at {} s; the source is {duration_s} s long", r.end_s);
        return Err((StatusCode::BAD_REQUEST, Json(crate::api::middleware::validator::err_body("param_out_of_range", &msg, Some("time_range"), &schema.id))).into_response());
    }
    Ok(crate::providers::VideoSource { bytes, content_type, duration_s, fps: media.fps, vendor_job_id })
}

/// A video container's content type from its format name.
fn video_content_type(format: Option<&str>) -> String {
    match format.map(str::to_ascii_lowercase).as_deref() {
        Some("gif") => "image/gif",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        _ => "video/mp4",
    }
    .to_string()
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
            let mut gen = gen;
            present_rows(&state, key_ctx.as_ref(), std::slice::from_mut(&mut gen)).await;
            (StatusCode::OK, Json(serde_json::to_value(&gen).unwrap())).into_response()
        }
        Ok(_) => (StatusCode::NOT_FOUND, Json(error_response("Generation not found", 404))).into_response(),
        Err(e) => {
            internal_error_response("Failed to get generation", &e)
        }
    }
}

/// A generation the caller may read — the same checks as
/// `GET /v1/generations/{id}` — or the answer to send instead.
async fn readable_generation(
    state: &Arc<AppState>,
    key_ctx: Option<&crate::api::middleware::KeyContext>,
    id: &str,
) -> Result<crate::types::Generation, axum::response::Response> {
    let Some(ctx_org) = key_ctx.and_then(|c| c.org_id.as_deref()) else {
        return Err(forbidden_no_org());
    };
    match state.db.get_generation(id).await {
        Ok(Some(gen)) if gen.org_id.as_deref() == Some(ctx_org) => {
            authorize_generation_for_session(
                state, key_ctx, &gen,
                crate::auth::permissions::Permission::GenerationReadAny,
                crate::auth::permissions::Permission::GenerationReadOwn,
                "generation:read:own",
            ).await?;
            Ok(gen)
        }
        Ok(_) => Err((StatusCode::NOT_FOUND, Json(error_response("Generation not found", 404))).into_response()),
        Err(e) => {
            Err(internal_error_response("Failed to get generation", &e))
        }
    }
}

/// A generation whose stored file the caller may feed into a new job: one
/// [`readable_generation`] gives and, where the app keeps files to their
/// owners (`signed`, `private`), inside its owner scope, as the file route
/// (`get_generation_asset`) holds it. Under `public` the stored URL is open
/// to any reader of the generation anyway.
async fn chainable_generation(
    state: &Arc<AppState>,
    key_ctx: Option<&crate::api::middleware::KeyContext>,
    id: &str,
) -> Option<crate::types::Generation> {
    let gen = readable_generation(state, key_ctx, id).await.ok()?;
    let policy = crate::access::policy_for(state, gen.app_id.as_deref()).await;
    let kept_to_owners = policy.mode != crate::access::AccessMode::Public;
    if kept_to_owners && !crate::access::present::in_owner_scope(policy.owner_scope, key_ctx, &FileOwner::of(&gen)) {
        return None;
    }
    Some(gen)
}

/// GET /v1/generations/{id}/assets — Every file litegen kept for a generation.
#[utoipa::path(
    get,
    path = "/v1/generations/{id}/assets",
    params(("id" = String, Path, description = "Generation ID")),
    responses(
        (status = 200, description = "The generation's stored files, in order. Kept for at least 7 days.", body = GenerationAssetList),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Generations"
)]
pub async fn list_generation_assets(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match readable_generation(&state, key_ctx.as_ref(), &id).await {
        Ok(gen) => {
            let mut data = generation_assets(&gen);
            presenter_for(&state, key_ctx.as_ref(), &FileOwner::of(&gen)).await.present_assets(&mut data).await;
            let list = crate::types::GenerationAssetList { generation_id: gen.id.clone(), data };
            (StatusCode::OK, Json(serde_json::to_value(list).unwrap())).into_response()
        }
        Err(resp) => resp,
    }
}

/// GET /v1/generations/{id}/assets/{index} — The bytes of one stored file.
#[utoipa::path(
    get,
    path = "/v1/generations/{id}/assets/{index}",
    params(
        ("id" = String, Path, description = "Generation ID"),
        ("index" = u32, Path, description = "The asset's `index` in `GET /v1/generations/{id}/assets`"),
    ),
    responses(
        (status = 200, description = "The file, with its content type. A file litegen keeps carries `Accept-Ranges: bytes`"),
        (status = 206, description = "The bytes one `Range: bytes=…` header asked for, with `Content-Range`; several ranges, or an `If-Range`, get the whole file (200)"),
        (status = 307, description = "The file is held by storage litegen does not read back (an S3 image store); follow `Location`"),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "No such generation or asset", body = ErrorResponse),
        (status = 416, description = "`range_not_satisfiable`: the range starts past the end of the file; `Content-Range: bytes */<length>` when the length is known", body = ErrorResponse),
    ),
    tag = "Generations"
)]
pub async fn get_generation_asset(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path((id, index)): Path<(String, usize)>,
    hooks: Option<Extension<Arc<crate::hooks::HookSet>>>,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let gen = match readable_generation(&state, key_ctx.as_ref(), &id).await {
        Ok(g) => g,
        Err(resp) => return resp,
    };
    // Reading a generation is org-wide; opening its files follows the app's
    // owner scope (`docs/storage-access.md`), in every access mode.
    let owner = FileOwner::of(&gen);
    let policy = crate::access::policy_for(&state, gen.app_id.as_deref()).await;
    if !crate::access::present::in_owner_scope(policy.owner_scope, key_ctx.as_ref(), &owner) {
        let whose = match policy.owner_scope {
            crate::access::OwnerScope::Key => "another key",
            crate::access::OwnerScope::App => "another app",
            crate::access::OwnerScope::Org => "another org",
        };
        let body = serde_json::json!({ "error": {
            "type": "forbidden",
            "code": "not_file_owner",
            "message": format!("this file belongs to {whose}; the app's owner scope is {}", policy.owner_scope.as_str()),
        }});
        return (StatusCode::FORBIDDEN, Json(body)).into_response();
    }
    let Some(asset) = generation_assets(&gen).into_iter().nth(index) else {
        return (StatusCode::NOT_FOUND, Json(error_response("Asset not found", 404))).into_response();
    };
    // Then the operator's own rule, which can only narrow what passed.
    if let Some(Extension(hooks)) = hooks.as_ref() {
        let end_user = gen.metadata.as_ref().and_then(|m| m.get("end_user")).and_then(|v| v.as_str());
        let file = serde_json::json!({
            "generation_id": gen.id,
            "index": index,
            "kind": asset.kind,
            "format": asset.format,
            "size_bytes": asset.size_bytes,
            "media_type": gen.media_type,
            "model": gen.model,
            "created_at": gen.created_at,
            "created_by": {
                "key_id": gen.key_id.map(|k| k.to_string()),
                "app_id": gen.app_id,
                "org_id": gen.org_id,
                "end_user": end_user,
            },
        });
        if let Err(response) = crate::hooks::file_access(hooks, &uri, &headers, key_ctx.as_ref(), file).await {
            return response;
        }
    }
    serve_generation_file(&state, &gen, asset, requested_range(&headers)).await
}

/// GET /v1/generations/{id}/assets/{index}/signed — One file, through a link litegen signed.
#[utoipa::path(
    get,
    path = "/v1/generations/{id}/assets/{index}/signed",
    params(
        ("id" = String, Path, description = "Generation ID"),
        ("index" = u32, Path, description = "The asset's `index` in `GET /v1/generations/{id}/assets`"),
        ("expires" = i64, Query, description = "When the link stops working (unix seconds)"),
        ("issued" = i64, Query, description = "When litegen issued the link (unix seconds)"),
        ("signature" = String, Query, description = "litegen's signature over the file, `issued` and `expires`"),
    ),
    responses(
        (status = 200, description = "The file, with its content type. No credentials: the signature is the permission. A file litegen keeps carries `Accept-Ranges: bytes`"),
        (status = 206, description = "The bytes one `Range: bytes=…` header asked for, with `Content-Range`; several ranges, or an `If-Range`, get the whole file (200)"),
        (status = 403, description = "`invalid_link`, `link_expired` or `link_revoked`", body = ErrorResponse),
        (status = 404, description = "No such generation or asset", body = ErrorResponse),
        (status = 410, description = "`vendor_file_gone`: the vendor no longer has a file litegen didn't keep", body = ErrorResponse),
        (status = 416, description = "`range_not_satisfiable`: the range starts past the end of the file; `Content-Range: bytes */<length>` when the length is known", body = ErrorResponse),
    ),
    tag = "Generations"
)]
pub async fn get_signed_generation_asset(
    State(state): State<Arc<AppState>>,
    Path((id, index)): Path<(String, usize)>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let refuse = |code: &str, message: &str| {
        let body = serde_json::json!({ "error": { "type": "forbidden", "code": code, "message": message } });
        (StatusCode::FORBIDDEN, Json(body)).into_response()
    };
    let number = |k: &str| query.get(k).and_then(|v| v.parse::<i64>().ok());
    let (Some(expires), Some(issued), Some(signature)) = (number("expires"), number("issued"), query.get("signature")) else {
        return refuse("invalid_link", "this link is not one litegen signed: it needs expires, issued and signature");
    };
    if !state.router.link_signer.verify_file_route(&id, index, issued, expires, signature) {
        return refuse("invalid_link", "this link is not one litegen signed for this file");
    }
    if expires < chrono::Utc::now().timestamp() {
        return refuse("link_expired", "this link has expired; read the generation again for a fresh one");
    }
    let gen = match state.db.get_generation(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(error_response("Not found", 404))).into_response(),
        Err(e) => {
            error!(generation_id = %id, error = %e, "could not read the generation of a signed link");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response("the generation could not be read; retry shortly", 500))).into_response();
        }
    };
    if let Err(resp) = revocation_check(&state, gen.app_id.as_deref(), &gen.id, Some(issued)).await {
        return resp;
    }
    // Private means the file route only: a link signed while the app was
    // `signed` closes with it, as the local store's do.
    if crate::access::policy_for(&state, gen.app_id.as_deref()).await.mode == crate::access::AccessMode::Private {
        return (StatusCode::NOT_FOUND, Json(error_response("Not found", 404))).into_response();
    }
    let Some(asset) = generation_assets(&gen).into_iter().nth(index) else {
        return (StatusCode::NOT_FOUND, Json(error_response("Asset not found", 404))).into_response();
    };
    serve_generation_file(&state, &gen, asset, requested_range(&headers)).await
}

/// 403 `link_revoked` when a revocation covering `generation_id` (its own, its
/// app's or the gateway's) came after the link was issued.
async fn revocation_check(state: &AppState, app_id: Option<&str>, generation_id: &str, issued: Option<i64>) -> Result<(), axum::response::Response> {
    match state.db.links_revoked_at(app_id, generation_id).await {
        Ok(at) if crate::access::link_is_revoked(issued, at) => {
            let body = serde_json::json!({ "error": {
                "type": "forbidden",
                "code": "link_revoked",
                "message": "this link was revoked; read the generation again for a fresh one",
            }});
            Err((StatusCode::FORBIDDEN, Json(body)).into_response())
        }
        Ok(_) => Ok(()),
        Err(e) => {
            error!(generation_id = %generation_id, error = %e, "could not check a link's revocation");
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response("could not check whether this link was revoked; retry shortly", 500))).into_response())
        }
    }
}

/// May the caller change `gen` (revoke its links, delete its files)? A
/// dashboard session needs `generation:delete:any`, or `:own` for its own; an
/// API key must pass the app's owner scope, as opening its files does.
async fn authorize_generation_change(state: &AppState, key_ctx: Option<&KeyContext>, gen: &crate::types::Generation) -> Result<(), axum::response::Response> {
    authorize_generation_for_session(
        state,
        key_ctx,
        gen,
        crate::auth::permissions::Permission::GenerationDeleteAny,
        crate::auth::permissions::Permission::GenerationDeleteOwn,
        "generation:delete:own",
    )
    .await?;
    let policy = crate::access::policy_for(state, gen.app_id.as_deref()).await;
    if key_ctx.is_some_and(|c| c.user.is_none()) && !crate::access::present::in_owner_scope(policy.owner_scope, key_ctx, &FileOwner::of(gen)) {
        let body = serde_json::json!({ "error": {
            "type": "forbidden",
            "code": "not_file_owner",
            "message": format!("this generation belongs to another key than the app's owner scope ({}) allows", policy.owner_scope.as_str()),
        }});
        return Err((StatusCode::FORBIDDEN, Json(body)).into_response());
    }
    Ok(())
}

/// POST /v1/generations/{id}/revoke-links — end every litegen link to one generation's files.
#[utoipa::path(
    post,
    path = "/v1/generations/{id}/revoke-links",
    params(("id" = String, Path, description = "Generation ID")),
    responses(
        (status = 200, description = "litegen's links to this generation's files issued before `revoked_at` no longer work. A bucket's own presigned links work until they expire.", body = crate::access::LinksRevoked),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Generations"
)]
pub async fn revoke_generation_links(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<String>,
) -> axum::response::Response {
    let gen = match readable_generation(&state, key_ctx.as_ref(), &id).await {
        Ok(g) => g,
        Err(resp) => return resp,
    };
    if let Err(resp) = authorize_generation_change(&state, key_ctx.as_ref(), &gen).await {
        return resp;
    }
    let at = crate::access::revocation_time();
    if let Err(e) = state.db.revoke_links("generation", &gen.id, at).await {
        error!(generation_id = %id, error = %e, "could not revoke a generation's links");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response("the links could not be revoked; retry shortly", 500))).into_response();
    }
    log_audit(state.db.clone(), key_ctx.as_ref(), "generation.revoke_links", "generation", &id, None, Some(serde_json::json!({ "revoked_at": at })));
    (StatusCode::OK, Json(crate::access::LinksRevoked { revoked_at: at })).into_response()
}

/// DELETE /v1/generations/{id} — delete a finished generation's files and request.
#[utoipa::path(
    delete,
    path = "/v1/generations/{id}",
    params(("id" = String, Path, description = "Generation ID")),
    responses(
        (status = 200, description = "The generation as it now stands: its kept files deleted (`files_deleted_at`), its request (prompt and params) removed from it and from the request log. The billing record stays. Files in an app's own bucket are deleted too; a vendor's own files (`kept: false`) are the vendor's.", body = Generation),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 409, description = "`generation_running`: cancel it first", body = ErrorResponse),
        (status = 502, description = "`files_delete_failed`: a file could not be deleted; nothing was marked, so try again", body = ErrorResponse),
    ),
    tag = "Generations"
)]
pub async fn delete_generation(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<String>,
) -> axum::response::Response {
    let gen = match readable_generation(&state, key_ctx.as_ref(), &id).await {
        Ok(g) => g,
        Err(resp) => return resp,
    };
    if let Err(resp) = authorize_generation_change(&state, key_ctx.as_ref(), &gen).await {
        return resp;
    }
    if !gen.status.is_terminal() {
        let body = serde_json::json!({ "error": {
            "type": "invalid_request_error",
            "code": "generation_running",
            "message": format!("generation {id} is still running; cancel it first (PATCH /v1/generations/{id})"),
        }});
        return (StatusCode::CONFLICT, Json(body)).into_response();
    }
    if gen.files_deleted_at.is_none() {
        let stores = crate::proxy::retention::stores_for(&crate::access::present::Presenting::of(&state), gen.app_id.as_deref()).await;
        if let Err(e) = crate::proxy::retention::delete_generation_files(&stores, &gen).await {
            let body = serde_json::json!({ "error": {
                "type": "provider_error",
                "code": "files_delete_failed",
                "message": format!("the files could not all be deleted, so nothing was marked; try again ({e}). If the app's own bucket stays unusable, fix its settings or remove them (`DELETE /v1/apps/{{app_id}}/storage`) first."),
            }});
            return (StatusCode::BAD_GATEWAY, Json(body)).into_response();
        }
        if let Err(e) = state.db.mark_files_deleted(&id, chrono::Utc::now(), "deleted").await {
            error!(generation_id = %id, error = %e, "files deleted, but the row could not be marked");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response("the files are deleted, but the generation could not be updated; try again", 500))).into_response();
        }
    }
    if let Err(e) = state.db.redact_generation_request(&id).await {
        error!(generation_id = %id, error = %e, "could not remove a deleted generation's request");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response("the files are deleted, but the request could not be removed; try again", 500))).into_response();
    }
    log_audit(state.db.clone(), key_ctx.as_ref(), "generation.delete", "generation", &id, None, None);
    let mut gen = match state.db.get_generation(&id).await {
        Ok(Some(g)) => g,
        _ => gen,
    };
    presenter_for(&state, key_ctx.as_ref(), &FileOwner::of(&gen)).await.present_row(&mut gen).await;
    (StatusCode::OK, Json(serde_json::to_value(&gen).unwrap())).into_response()
}

/// Delete `gens`' files from litegen's own store, in the background: what
/// goes with a deleted app or org. Files in an app's own bucket are the
/// customer's, so they stay.
pub(crate) fn delete_files_in_litegens_store(state: &AppState, gens: Vec<crate::types::Generation>) {
    if gens.is_empty() {
        return;
    }
    let stores = crate::proxy::retention::FileStores { stores: vec![state.router.model3d_store.clone()], unavailable: None };
    tokio::spawn(async move {
        for gen in gens {
            if let Err(e) = crate::proxy::retention::delete_generation_files(&stores, &gen).await {
                tracing::warn!(generation_id = %gen.id, error = %e, "a deleted app's file could not be deleted");
            }
        }
    });
}

/// The headers litegen serves every file from its own origin with: never
/// sniffed into anything else, and sandboxed if a browser renders it (an SVG
/// can carry script).
fn own_origin_file_headers(content_type: String) -> [(axum::http::HeaderName, String); 4] {
    [
        (axum::http::header::CONTENT_TYPE, content_type),
        (axum::http::header::CACHE_CONTROL, "private, max-age=86400".to_string()),
        (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        (axum::http::header::CONTENT_SECURITY_POLICY, "sandbox".to_string()),
    ]
}

/// The one byte range a request's `Range` header asks for, or `None` when
/// the whole file is served: no header, several ranges, an `If-Range`, or a
/// header litegen can't read (RFC 9110 §14.2 lets a server ignore any of
/// these and answer 200).
fn requested_range(headers: &axum::http::HeaderMap) -> Option<crate::store::ByteRange> {
    use crate::store::ByteRange;
    if headers.contains_key(axum::http::header::IF_RANGE) {
        return None;
    }
    let spec = headers.get(axum::http::header::RANGE)?.to_str().ok()?.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (first, last) = spec.split_once('-')?;
    match (first.trim(), last.trim()) {
        ("", "") => None,
        ("", n) => n.parse().ok().map(ByteRange::Last),
        (a, "") => a.parse().ok().map(ByteRange::From),
        (a, b) => {
            let (a, b) = (a.parse().ok()?, b.parse().ok()?);
            (a <= b).then_some(ByteRange::FromTo(a, b))
        }
    }
}

/// A stored file as a GET's answer, streamed as the store reads it, never
/// held whole: `headers` (its type and the route's own), `Content-Length`
/// when the store knows it, `Accept-Ranges` from a store that reads ranges,
/// and one asked-for range answered 206 with `Content-Range`, or 416 when
/// it lies outside the file.
fn stored_file_response(
    file: crate::store::StoredFile,
    headers: impl IntoIterator<Item = (axum::http::HeaderName, String)>,
) -> axum::response::Response {
    use crate::store::Served;
    use axum::http::{header, HeaderValue};
    let (status, length, content_range) = match file.served {
        Served::Whole => (StatusCode::OK, file.len, None),
        Served::Part { start, end } => {
            let of = file.len.map_or_else(|| "*".to_string(), |n| n.to_string());
            (StatusCode::PARTIAL_CONTENT, Some(end - start + 1), Some(format!("bytes {start}-{end}/{of}")))
        }
        Served::Unsatisfiable => {
            let body = serde_json::json!({ "error": {
                "type": "invalid_request_error",
                "code": "range_not_satisfiable",
                "message": "the requested range lies outside the file",
            }});
            let mut resp = (StatusCode::RANGE_NOT_SATISFIABLE, Json(body)).into_response();
            // Only a length the store reported: never one made up.
            if let Some(Ok(v)) = file.len.map(|n| HeaderValue::from_str(&format!("bytes */{n}"))) {
                resp.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            return resp;
        }
    };
    let mut resp = axum::response::Response::new(axum::body::Body::from_stream(file.body));
    *resp.status_mut() = status;
    let out = resp.headers_mut();
    for (name, value) in headers {
        if let Ok(v) = HeaderValue::from_str(&value) {
            out.insert(name, v);
        }
    }
    if let Some(n) = length {
        out.insert(header::CONTENT_LENGTH, HeaderValue::from(n));
    }
    if let Some(Ok(v)) = content_range.map(|r| HeaderValue::from_str(&r)) {
        out.insert(header::CONTENT_RANGE, v);
    }
    if file.accepts_ranges {
        out.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    }
    resp
}

/// The bytes of one of `gen`'s files, from whichever store minted its URL (the
/// app's own bucket when it has one, else litegen's), or streamed from the
/// vendor when litegen kept no copy. A stored file streams, and answers
/// `range` (see [`stored_file_response`]).
async fn serve_generation_file(
    state: &Arc<AppState>,
    gen: &crate::types::Generation,
    asset: GenerationAsset,
    range: Option<crate::store::ByteRange>,
) -> axum::response::Response {
    if let Some(at) = gen.files_deleted_at {
        let why = match gen.files_deleted_reason.as_deref() {
            Some("retention") => "after its app's retention",
            _ => "on request",
        };
        let body = serde_json::json!({ "error": {
            "type": "not_found_error",
            "code": "files_deleted",
            "message": format!("this generation's files were deleted {why}, at {}", at.to_rfc3339()),
        }});
        return (StatusCode::GONE, Json(body)).into_response();
    }
    if !asset.kept {
        return vendor_file_response(state, &asset.url).await;
    }
    let mut stores = vec![state.router.model3d_store.clone()];
    let mut bucket_down = None;
    if let Some(app) = gen.app_id.as_deref() {
        match app_store_for_files(&state.db, state.secrets_key, &state.router.config.aws, tenant_bucket_of(state), app).await {
            Ok(Some(s)) => stores.insert(0, s),
            Ok(None) => {}
            Err(why) => bucket_down = Some(why),
        }
    }
    for store in &stores {
        let Some(key) = store.key_of(&asset.url) else { continue };
        match store.open(&key, range).await {
            Ok(Some(file)) => {
                let headers = own_origin_file_headers(file.content_type.clone());
                return stored_file_response(file, headers);
            }
            Ok(None) => {
                return (StatusCode::NOT_FOUND, Json(error_response("The asset is no longer stored", 404))).into_response();
            }
            Err(e) => {
                error!(generation_id = %gen.id, error = %e, "failed to read a stored asset");
                return (StatusCode::BAD_GATEWAY, Json(error_response("The asset could not be read from storage", 502))).into_response();
            }
        }
    }
    // The file may be in the app's own bucket, which can't be used right now:
    // try again later, rather than being sent to the bucket's own URL.
    if let Some(why) = bucket_down {
        error!(generation_id = %gen.id, reason = %why, "the app's bucket can't be used to read a stored asset");
        return (StatusCode::BAD_GATEWAY, Json(error_response("The asset could not be read from storage", 502))).into_response();
    }
    if asset.url.starts_with("https://") || asset.url.starts_with("http://") {
        return (StatusCode::TEMPORARY_REDIRECT, [(axum::http::header::LOCATION, asset.url)]).into_response();
    }
    (StatusCode::NOT_FOUND, Json(error_response("Asset not found", 404))).into_response()
}

/// The most of a vendor's file litegen streams through: a video's cap.
const MAX_VENDOR_FILE_BYTES: u64 = 1024 * 1024 * 1024;

/// Stream a vendor's own file (`kept: false`) to a caller litegen let in, so
/// the vendor's link never leaves litegen. Only a public host is fetched, with
/// no redirects; a vendor that no longer has the file answers 410.
async fn vendor_file_response(state: &Arc<AppState>, url: &str) -> axum::response::Response {
    use futures::StreamExt;
    let failed = |status: StatusCode, code: &str, message: String| {
        let kind = if status == StatusCode::GONE { "not_found_error" } else { "provider_error" };
        (status, Json(serde_json::json!({ "error": { "type": kind, "code": code, "message": message } }))).into_response()
    };
    if !state.router.config.storage_access.unchecked_vendor_hosts {
        if let Err(e) = crate::util::ssrf::check_public_url(url).await {
            return failed(StatusCode::BAD_GATEWAY, "vendor_file_unreachable", format!("the vendor's link is not a public URL litegen fetches: {e}"));
        }
    }
    let resp = match crate::proxy::router::vendor_download_client().get(url).send().await {
        Ok(r) => r,
        Err(e) => return failed(StatusCode::BAD_GATEWAY, "vendor_file_unreachable", format!("the vendor's file could not be fetched: {}", e.without_url())),
    };
    match resp.status().as_u16() {
        200 => {}
        403 | 404 | 410 => {
            return failed(
                StatusCode::GONE,
                "vendor_file_gone",
                "the vendor no longer has this file, and litegen kept no copy (keep_outputs: false)".into(),
            )
        }
        other => return failed(StatusCode::BAD_GATEWAY, "vendor_file_unreachable", format!("the vendor answered HTTP {other} for this file")),
    }
    if resp.content_length().is_some_and(|n| n > MAX_VENDOR_FILE_BYTES) {
        return failed(StatusCode::BAD_GATEWAY, "vendor_file_unreachable", "the vendor's file is larger than litegen serves".into());
    }
    let content_type = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    // Counted as it streams, so a vendor that sends more than it announced
    // is cut off at the cap.
    let mut sent: u64 = 0;
    let body = resp.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        sent += chunk.len() as u64;
        if sent > MAX_VENDOR_FILE_BYTES {
            return Err(std::io::Error::other("the vendor's file is larger than litegen serves"));
        }
        Ok(chunk)
    });
    (StatusCode::OK, own_origin_file_headers(content_type), axum::body::Body::from_stream(body)).into_response()
}

/// POST /v1/videos/cost — Estimate cost for video generation.
#[utoipa::path(
    post,
    path = "/v1/videos/cost",
    request_body = VideoGenerationRequest,
    responses(
        (status = 200, description = "Cost estimate", body = CostEstimate,
            headers(
                ("X-Litegen-Dropped-Params" = String, description = "Comma-separated names of the fields and params this request sent that the route or model does not take, dropped instead of refused because `strict` was false; a field inside an object is named by its path (`reference_images[0].weight`). At most 32 names: any further name, and any name that is not 1–64 visible ASCII characters without a comma, is counted in a last `+N more`. Absent when nothing was dropped."),
            )),
        (status = 400, description = "Bad request (validation failed)", body = ErrorResponse),
        (status = 408, description = "`body_timeout`: the request body did not arrive within `server.request_body_timeout_secs` (default 300 s).", body = ErrorResponse),
        (status = 413, description = "`body_too_large`: the request body is over 25 MiB, as declared or as sent.", body = ErrorResponse),
        (status = 503, description = "`server_at_capacity`: litegen is running as many requests as it may (`backpressure.max_in_flight`), or holds as many request bodies as it may; retry shortly (`Retry-After` is set).", body = ErrorResponse),
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
        Ok(estimate) => with_dropped((StatusCode::OK, Json(serde_json::to_value(estimate).unwrap())).into_response(), &validated.dropped),
        Err(e) => {
            generation_error_response(&e)
        }
    }
}

// ─── 3D Model Generation ────────────────────────────────────────────────────

/// POST /v1/models3d/generations — Start a 3D (mesh) generation.
#[utoipa::path(
    post,
    path = "/v1/models3d/generations",
    request_body = Model3dGenerationRequest,
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "1–255 visible ASCII characters. The same key and the same request within 24 hours replay the first response (`Idempotent-Replayed: true`) and bill nothing; the same key with a different request is a 409. A request refused before any vendor was called does not spend the key."),
        ("X-Litegen-Keep-Outputs" = Option<String>, Header, description = "`false` keeps nothing for this request: no copy of the output is stored anywhere, and the answer carries the image itself or the vendor's own link (docs/storage-access.md). A request can keep less than its app does, never more; any other value is a 400 `invalid_keep_outputs_header`."),
    ),
    responses(
        (status = 200, description = "3D generation started", body = Model3dGenerationResponse,
            headers(
                ("X-Litegen-Dropped-Params" = String, description = "Comma-separated names of the fields and params this request sent that the route or model does not take, dropped instead of refused because `strict` was false; a field inside an object is named by its path (`reference_images[0].weight`). At most 32 names: any further name, and any name that is not 1–64 visible ASCII characters without a comma, is counted in a last `+N more`. Absent when nothing was dropped."),
                ("Idempotent-Replayed" = String, description = "`true` when this is the stored response of an earlier request with the same `Idempotency-Key`; nothing ran and nothing was billed. Absent otherwise."),
            )),
        (status = 400, description = "Bad request: a request that fails validation (`param_*`, with `param`), or one that keeps nothing (`X-Litegen-Keep-Outputs: false`) yet asks litegen for something it makes from a file it keeps: a container it converts (`format_needs_keeping`) or an output option it applies (`option_needs_keeping`, naming the option in `param`, e.g. `output_packages`, `ktx2_textures`, `dae_morphs`, `usdz_root`, `usd_clips`)", body = ErrorResponse),
        (status = 404, description = "`model_not_found`", body = ErrorResponse),
        (status = 410, description = "`model_retired`: the model is past its retirement; `replaced_by` names its successor", body = ErrorResponse),
        (status = 422, description = "A `source_model` litegen can't use (unreadable, too large, too many triangles, no skin to animate), or the vendor's free preflight refused it (its own code).", body = ErrorResponse),
        (status = 409, description = "`idempotency_key_reused`: the Idempotency-Key was used for a different request. `idempotency_request_in_progress`: a request with this key is still running (`Retry-After` is set).", body = ErrorResponse),
        (status = 504, description = "`vendor_timeout`: the vendor did not answer in time. Never re-sent, since the vendor may still run and bill the job; `vendor_job_id` names it when the vendor had accepted one.", body = ErrorResponse),
        (status = 500, description = "`generation_record_failed`: the vendor accepted the job but litegen could not record it, so it has no id to poll; `vendor_job_id` names the vendor's job. A retry with the same `Idempotency-Key` gets this answer, not a second job.", body = ErrorResponse),
        (status = 408, description = "`body_timeout`: the request body did not arrive within `server.request_body_timeout_secs` (default 300 s).", body = ErrorResponse),
        (status = 413, description = "`body_too_large`: the request body is over 25 MiB, as declared or as sent.", body = ErrorResponse),
        (status = 503, description = "`server_at_capacity`: litegen is running as many requests as it may (`backpressure.max_in_flight`), or holds as many request bodies as it may; retry shortly (`Retry-After` is set).", body = ErrorResponse),
    ),
    tag = "Models3D"
)]
pub async fn generate_3d(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    attribution: crate::management::enforce::RequestAttribution,
    headers: axum::http::HeaderMap,
    validated: ValidatedModel3d,
) -> impl IntoResponse {
    let start = std::time::Instant::now();
    // Refused before anything runs. The decision is taken now and saved in
    // the row's delivery plan, so whichever observer delivers honours it.
    let keep_header = match crate::access::keep_header(&headers) {
        Ok(h) => h,
        Err(e) => return keep_header_rejection(&e),
    };
    let keep = crate::access::keeps_outputs(
        &crate::access::policy_for(&state, key_ctx.as_ref().and_then(|c| c.app_id.as_deref())).await,
        keep_header,
    );
    // `ktx2_textures` is an option litegen applies on delivery to any model's
    // mesh; a word it does not take is refused before anything is reserved.
    let ktx2_textures = match validated.request.ktx2_textures.as_deref() {
        None => None,
        Some(v) => match crate::types::Model3dKtx2Textures::parse(v) {
            Some(option) => Some(option),
            None => {
                return param_rejection("param_invalid", "ktx2_textures", &format!("`ktx2_textures` is {v:?}; it takes png or keep"), &validated.schema.id)
            }
        },
    };
    // Converting writes a file, so keeping nothing can promise only the
    // containers the vendor makes itself.
    if !keep {
        let native = validated.schema.output_formats_spec().map(|s| s.enum_values).unwrap_or_default();
        let requested = validated.schema.resolve_output_formats(validated.request.output_formats.as_deref());
        let converted: Vec<&String> = requested.iter().filter(|f| !native.iter().any(|n| n.eq_ignore_ascii_case(f))).collect();
        if !converted.is_empty() {
            let message = format!(
                "litegen makes {} by converting a file it keeps, and this request keeps nothing (keep_outputs: false); {} makes {} itself",
                converted.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(", "),
                validated.schema.id,
                native.join(", "),
            );
            let body = serde_json::json!({ "error": { "message": message, "type": "invalid_request_error", "code": "format_needs_keeping" } });
            return (StatusCode::BAD_REQUEST, Json(body)).into_response();
        }
        // An output option is applied to the file litegen keeps; keeping
        // nothing, the vendor's files go out as they came, so it couldn't
        // apply. Refused, never silently dropped.
        if validated.request.output_packages == Some(true) {
            return option_needs_keeping("output_packages", "zips each container with its sidecars", &validated.schema.id);
        }
        if ktx2_textures == Some(crate::types::Model3dKtx2Textures::Keep) {
            return option_needs_keeping("ktx2_textures", "writes the KTX2 textures into a glb or gltf", &validated.schema.id);
        }
        if validated.request.dae_morphs == Some(crate::types::Model3dDaeMorphs::Keep) {
            return option_needs_keeping("dae_morphs", "writes a Collada file's morph targets as morph controllers", &validated.schema.id);
        }
        if validated.request.usdz_root == Some(crate::types::Model3dUsdzRoot::Usdc) {
            return option_needs_keeping("usdz_root", "roots the USDZ it writes in a crate", &validated.schema.id);
        }
        if validated.request.usd_clips == Some(crate::types::Model3dUsdClips::All) {
            return option_needs_keeping("usd_clips", "writes every animation clip into the USD files it makes", &validated.schema.id);
        }
    }

    let refs = match resolve_generation_refs(&state, key_ctx.as_ref(), &validated.request.base.reference_images).await {
        Ok(r) => r,
        Err(e) => return reference_error_response(&e, &validated.schema.id),
    };
    let materialized = match state.materializer.materialize(
        &validated.schema,
        refs,
        &validated.ctx,
    ).await {
        Ok(m) => m,
        Err(e) => return validation_rejection_response(&e.to_string(), 400, &validated.schema.id),
    };

    let mut extras = Model3dExtras {
        output_formats: validated.schema.resolve_output_formats(
            validated.request.output_formats.as_deref(),
        ),
        texture: validated.request.texture,
        pbr: validated.request.pbr,
        target_polycount: validated.request.target_polycount,
        symmetry: validated.request.symmetry.clone(),
        topology: validated.request.topology.clone(),
        rig: validated.request.rig,
        skeleton: validated.request.skeleton.clone(),
        joint_naming: validated.request.joint_naming.clone(),
        height_m: validated.request.height_m,
        texture_resolution: validated.request.texture_resolution,
        keep_uvs: validated.request.keep_uvs,
        ground: validated.request.ground,
        center: validated.request.center,
        animations: match (validated.request.animations.as_deref(), validated.schema.params.get("animations")) {
            (Some(clips), Some(crate::capabilities::ParamSpec::AnimationList(spec))) => {
                crate::providers::AnimationClip::resolve(clips, spec)
            }
            _ => Vec::new(),
        },
        fps: validated.request.fps,
        fbx_color_space: validated.request.fbx_color_space,
        fbx_light_units: validated.request.fbx_light_units,
        usd_light_units: validated.request.usd_light_units,
        dae_morphs: validated.request.dae_morphs,
        usdz_root: validated.request.usdz_root,
        usd_clips: validated.request.usd_clips,
        format_source: validated.request.format_source,
        output_packages: validated.request.output_packages,
        ktx2_textures,
        extra: validated.request.base.extra.clone(),
        source: None,
        preflight: None,
    };

    let app_creds = match resolve_org_provider_credential(&state, &key_ctx, &validated.schema.provider).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if app_creds.is_none() && !state.router.has_model3d_provider(&validated.schema.provider).await {
        return provider_not_configured_response(&validated.schema.provider);
    }
    // Which vendor account this job runs on: a vendor's task ids are only good
    // on the account that made them, so chaining by task id needs it to match.
    let vendor_account = match (&app_creds, key_ctx.as_ref().and_then(|c| c.org_id.as_deref())) {
        (Some(_), Some(org)) => format!("org:{org}"),
        _ => "platform".to_string(),
    };

    // An operation's source (3D doc §3A): read, checked and converted before
    // anything is reserved or billed (§2 rule 6).
    let mut lineage: Option<serde_json::Value> = None;
    if let (Some(src), Some(crate::capabilities::ParamSpec::SourceModel(spec))) =
        (&validated.request.source_model, validated.schema.params.get("source_model"))
    {
        match resolve_source_model(&state, key_ctx.as_ref(), spec, src, &validated.schema.provider, &vendor_account).await {
            Ok((input, record)) => {
                extras.source = Some(input);
                lineage = Some(record);
            }
            Err(e) => {
                let status = StatusCode::from_u16(e.status()).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY);
                return (status, Json(crate::api::middleware::validator::err_body(e.code(), &e.message(), Some("source_model"), &validated.schema.id))).into_response();
            }
        }
    }

    let charge_key = key_ctx.as_ref().and_then(|c| c.key_id);
    // This request's own price (3D doc §3G): what the quote said and the
    // charge will take.
    let price = charged_price(&state, crate::capabilities::pricing::request_price_usd(
        &validated.schema,
        &validated.request.base,
        &serde_json::to_value(&extras).unwrap_or_default(),
    ));
    let reserved = match reserve_quota(&state, charge_key, price).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    // Org/app/member/end-user/key limits (budgets, model allowlists, rpm and
    // parallel caps). Settled alongside the key quota below.
    let mut limits = match crate::management::enforce::begin(
        &state, key_ctx.as_ref(), &attribution, &validated.schema.id, price,
    ).await {
        Ok(g) => g,
        Err(resp) => {
            settle_quota(&state, reserved, 0.0).await;
            return resp;
        }
    };
    let attributed = limits.attribution();

    // The vendor's free preflight (3D doc §3B), once the caller may run this
    // model at all: a mesh the vendor says it cannot rig is refused here,
    // and nothing reserved above is charged.
    let preflight = state.router.preflight_model3d(&validated.schema, &extras, app_creds.clone()).await;
    match preflight {
        Ok(crate::providers::Preflight::Proceed { context }) => extras.preflight = context,
        Ok(crate::providers::Preflight::Refuse { code, message }) => {
            settle_quota(&state, reserved, 0.0).await;
            limits.settle(&state, 0.0).await;
            let param = validated.request.source_model.is_some().then_some("source_model");
            return (StatusCode::UNPROCESSABLE_ENTITY, Json(crate::api::middleware::validator::err_body(code, &message, param, &validated.schema.id))).into_response();
        }
        Err(e) => {
            settle_quota(&state, reserved, 0.0).await;
            limits.settle(&state, 0.0).await;
            error!(error = %e, "3D preflight failed");
            log_failed_generation(&state, key_ctx.as_ref(), FailedGeneration {
                id_prefix: "litegen-3d",
                media_type: "model3d",
                schema: &validated.schema,
                latency_ms: start.elapsed().as_millis() as i64,
                error: &e.to_string(),
                prompt: &validated.request.base.prompt,
                metadata: validated.request.base.metadata.as_ref(),
                attributed: &attributed,
            })
            .await;
            return generation_error_response(&e);
        }
    }

    match state.router.generate_model3d_keeping(
        &validated.schema, &validated.request.base, &extras, &materialized, app_creds, keep,
    ).await {
        Ok(mut response) => {
            response.metadata = validated.request.base.metadata_map();
            let latency = start.elapsed().as_millis() as i64;
            let cost = response.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);

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
            let key_id = key_ctx.as_ref().and_then(|c| c.key_id);
            let org_id = key_ctx.as_ref().and_then(|c| c.org_id.clone());
            let app_id = key_ctx.as_ref().and_then(|c| c.app_id.clone());
            let requested_formats = extras.output_formats.clone();
            // The plan the router's job holds too, so the poller delivers
            // exactly what the client's poll would.
            let delivery = crate::proxy::model3d_deliver::DeliveryPlan {
                keep_nothing: !keep,
                ..crate::proxy::model3d_deliver::DeliveryPlan::for_request(&validated.schema, &extras)
            };
            let request = request_record(&validated.request.base, serde_json::to_value(&extras).ok(), &validated.ctx);
            let log_metadata = validated.request.base.metadata.clone();
            // Who the job is charged against, for a refund if it fails (L18).
            let charged_to = limits.charged_to();
            let source_record = lineage.clone();
            let source_delivery = extras.source.as_ref().and_then(|s| s.delivery()).map(|d| d.as_str());
            let vendor_account = vendor_account.clone();

            // Everything below runs in a task the handler awaits, not on the
            // request future itself: the vendor has accepted (and will bill)
            // the job, so a client that disconnects now must not cancel the
            // settlement or the rows that let the poller drive it. Awaited, so
            // the writes still land before the response, as below explains.
            let writes = {
                let state = state.clone();
                let db = db.clone();
                let id = id.clone();
                tokio::spawn(async move {
                    let provider_job_id = state.router.get_model3d_provider_job_id(&id).await;
                    let stage_context = state.router.get_model3d_stage_context(&id).await;

                    // The row is written BEFORE the response, not spawned after it. The
                    // caller learns the id only from this response, so nothing can poll
                    // the job until these writes have landed. Spawned, a client polling
                    // fast enough terminalised a job whose row did not exist yet: the
                    // guarded terminal write found nothing to update, the GET path could
                    // not dispatch the webhook (it reads the key off the row), and the
                    // generation sat `pending` until the poller healed it. Three small
                    // writes cost nothing beside the vendor round-trip that preceded
                    // them. The log goes first so the generation can never be terminal
                    // while its log row is still to come — that order is what keeps a
                    // terminal `update_request_log_status` from being overwritten by a
                    // late `pending` insert.
                    if let Err(e) = db.log_request(&id, &model, &provider, "pending", "model3d", cost, latency, None, log_metadata.as_ref(), org_id.as_deref(), app_id.as_deref()).await {
                        tracing::warn!(generation_id = %id, error = %e, "failed to log the 3d request");
                    }
                    crate::management::enforce::attribute(&db, &id, &attributed).await;
                    // A job with no row cannot be polled, resumed or reaped, so
                    // the submit fails rather than answer with its id (L2) — and
                    // a submit that failed is charged nothing and logged failed.
                    let inserted = db.insert_generation(
                        &id, key_id.as_ref(), &model, &provider, "model3d",
                        provider_job_id.as_deref(), cost, org_id.as_deref(), app_id.as_deref(),
                    ).await;
                    let charge = if inserted.is_ok() { cost } else { 0.0 };
                    settle_quota(&state, reserved, charge).await;
                    limits.settle(&state, charge).await;
                    if inserted.is_err() {
                        if let Err(e) = db.update_request_log_status(&id, "failed", Some("generation_record_failed")).await {
                            tracing::warn!(generation_id = %id, error = %e, "failed to mark the unrecorded 3d request failed");
                        }
                    }
                    inserted.map_err(|e| (e, provider_job_id.clone()))?;
                    // The containers promised to this caller, recorded on the row so
                    // the poller and the row-answered path can enforce them — neither
                    // can reach the router's in-memory job map, and the row outlives
                    // it anyway. `insert_generation` takes no metadata, so this is a
                    // second statement; a lost write degrades to the GLB floor in
                    // `row_requested_formats`, never to "no guard at all". Guarded all
                    // the same: the background poller can reach the row between the
                    // insert and this write, and an unguarded write landing after a
                    // terminal one would replace its assets.
                    let mut meta = serde_json::json!({
                        "requested_formats": requested_formats,
                        "delivery": delivery,
                        "request": request,
                        "vendor_account": vendor_account,
                        "billing": { "limits": charged_to },
                        // Who it was made for, which `file_access` hooks compare.
                        "end_user": attributed.end_user,
                    });
                    if !keep {
                        meta["keep_outputs"] = false.into();
                    }
                    // Lineage (3D doc §3D): the source's generation and hash,
                    // and how it reached the vendor.
                    if let Some(mut record) = source_record {
                        record["delivery"] = serde_json::json!(source_delivery);
                        meta["source"] = record;
                    }
                    // The adapter's own continuation state — fal's status_url and
                    // response_url, Meshy's preview task id. The poller rebuilds the
                    // handle from this row and cannot reach the router's in-memory
                    // job map, so an adapter that needs more than a job id to poll
                    // with only works if this is written here.
                    if let Some(ctx) = stage_context {
                        meta["stage_context"] = ctx;
                    }
                    if let Err(e) = db.merge_generation_metadata_if_active(&id, &meta).await {
                        tracing::warn!(generation_id = %id, error = %e, "failed to record requested 3d formats");
                    }
                    Ok::<(), (sqlx::Error, Option<String>)>(())
                })
            };
            let failure = match writes.await {
                Ok(Ok(())) => None,
                Ok(Err((e, vendor_job))) => Some((e.to_string(), vendor_job)),
                Err(e) => Some((e.to_string(), state.router.get_model3d_provider_job_id(&id).await)),
            };
            if let Some((err, vendor_job)) = failure {
                tracing::error!(generation_id = %id, error = %err, "3d row could not be written; failing the submit");
                // Nothing will ever poll this job: stop it at the vendor (best effort).
                if let Some(job) = state.router.forget_model3d_job(&id).await {
                    let (handle, provider) = job;
                    tokio::spawn(async move {
                        if let Err(e) = provider.cancel(&handle).await {
                            tracing::warn!(error = %e, "could not cancel an unrecorded 3d job at the vendor");
                        }
                    });
                }
                return generation_record_failed_response(&id, vendor_job.as_deref());
            }
            // The artifact is diagnostic only — no observer of completion reads
            // it — so it alone stays off the response path.
            tokio::spawn(async move {
                if let Err(e) = db.insert_request_artifact(&artifact).await {
                    tracing::warn!(error = %e, request_id = %artifact.request_id, "Failed to store 3d artifact");
                }
            });

            let mut resp = (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
            resp.extensions_mut().insert(crate::api::middleware::idempotency::VendorAttempted);
            if let Some((k, v)) = dropped_header(&validated.dropped) {
                resp.headers_mut().insert(k, v);
            }
            resp
        }
        Err(e) => {
            settle_quota(&state, reserved, 0.0).await;
            limits.settle(&state, 0.0).await;
            error!(error = %e, "3D generation failed");
            log_failed_generation(&state, key_ctx.as_ref(), FailedGeneration {
                id_prefix: "litegen-3d",
                media_type: "model3d",
                schema: &validated.schema,
                latency_ms: start.elapsed().as_millis() as i64,
                error: &e.to_string(),
                prompt: &validated.request.base.prompt,
                metadata: validated.request.base.metadata.as_ref(),
                attributed: &attributed,
            })
            .await;
            generation_error_response(&e)
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
    // Read the row ONCE, and treat a DB error as an error. The old
    // `if let Ok(Some(gen))` skipped the tenant check on a transient `Err` and
    // the fallback at the bottom then returned the row unchecked, so a caller in
    // org B holding an org-A generation id could read A's mesh URLs.
    let row = match state.db.get_generation(&id).await {
        Ok(r) => r,
        Err(e) => {
            error!(generation_id = %id, error = %e, "failed to load 3d generation row");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_response("Failed to load generation", 500)),
            ).into_response();
        }
    };
    if let Some(gen) = row.as_ref() {
        // A video id is not a 3D generation: answered from its row it would
        // read as a 3D job that "completed without a mesh", i.e. `failed`.
        if gen.org_id.as_deref() != Some(ctx_org) || gen.media_type != "model3d" {
            return (StatusCode::NOT_FOUND, Json(error_response("Not found", 404))).into_response();
        }
        if let Err(resp) = authorize_generation_for_session(
            &state, key_ctx.as_ref(), gen,
            crate::auth::permissions::Permission::GenerationReadAny,
            crate::auth::permissions::Permission::GenerationReadOwn,
            "generation:read:own",
        ).await {
            return resp;
        }
        // Read-through once the row is terminal: answer FROM the row and never
        // poll the provider again. A terminal row is the decided outcome — for
        // `cancelled` especially, polling on would see the provider job run to
        // `Completed`, re-host the assets and persist `completed` over the
        // cancel, i.e. cancellation would only ever delay a job by one poll.
        // (`forget_model3d_job` drops the in-memory job on cancel; this is the
        // other half, covering a row terminalised by the poller, by a restart,
        // or by a cancel that raced an in-flight poll.)
        if gen.status.is_terminal() {
            // Whoever terminalised the row (usually the poller) never touched
            // the router's in-memory job, so release it here.
            state.router.forget_model3d_job(&id).await;
            return answer_3d(&state, key_ctx.as_ref(), FileOwner::of(gen), model3d_response_from_row(gen)).await;
        }
    }

    // Re-host into the tenant's OWN bucket, exactly as the poller does. Which of
    // the two observers wins the race must not decide whose bucket a mesh lands
    // in. `app_id` comes from the row when we have it (the poller's source of
    // truth) and from the caller's key context otherwise — the row insert is
    // spawned at submit, so an early poll can legitimately outrun it.
    let app_id = row.as_ref()
        .and_then(|g| g.app_id.clone())
        .or_else(|| key_ctx.as_ref().and_then(|c| c.app_id.clone()));
    let keep = crate::access::row_keeps_outputs(row.as_ref().and_then(|g| g.metadata.as_ref()));
    let (app_store, app_prefix) = match app_id.as_deref().filter(|_| keep) {
        Some(app_id) => match app_store_for_writing(&state.db, state.secrets_key, &state.router.config.aws, tenant_bucket_of(&state), app_id).await {
            Ok(found) => found.map_or((None, None), |(s, p)| (Some(s), p)),
            // Its bucket is down and litegen's store may not stand in: the
            // mesh waits, as the row says.
            Err(reason) => {
                tracing::warn!(generation_id = %id, reason = %reason, "a finished 3d job has nowhere to be kept yet");
                return match row.as_ref() {
                    Some(gen) => answer_3d(&state, key_ctx.as_ref(), FileOwner::of(gen), model3d_response_from_row(gen)).await,
                    None => (StatusCode::SERVICE_UNAVAILABLE, Json(error_response("the app's bucket can't be reached; retry shortly", 503))).into_response(),
                };
            }
        },
        None => (None, None),
    };

    match state.router.get_model3d_status_claimed(&id, app_store, app_prefix.as_deref(), &state.db).await {
        // The delivery claim, when this poll delivered the job, is held until
        // the outcome below is written: no other observer delivers it again
        // in between. It drops (and releases) at the end of this arm.
        Ok((mut resp, _delivery_claim)) => {
            // `created` is when the job was submitted, on every read: the row's.
            if let Some(gen) = row.as_ref() {
                resp.created = gen.created_at.timestamp();
                resp.metadata = gen.request_metadata();
            }
            // Persist the terminal result so `GET /v1/generations/{id}`, the
            // gallery, and any webhook see the same assets the poller would have
            // written — whichever path observed completion first.
            if resp.status == GenerationStatus::Completed {
                if resp.mesh().is_some() {
                    // Same source the poller reads, so both observers persist the
                    // same promise. `get_model3d_status` has already enforced it
                    // against the router's copy; this carries it onto the row.
                    let requested = row.as_ref().map(row_requested_formats).unwrap_or_else(
                        || vec![crate::types::DEFAULT_MODEL3D_FORMAT.to_string()],
                    );
                    if persist_model3d_result(&state, &resp, &requested).await {
                        dispatch_3d_webhook(&state, row.as_ref(), &resp).await;
                    } else {
                        resp = decided_or(&state, &id, resp).await;
                    }
                } else {
                    // Contract: a completed 3D generation ALWAYS carries a mesh
                    // asset (one per promised container, which the router has
                    // already enforced). A provider that reports success without
                    // any has produced a generation the caller paid for and
                    // cannot use, so this is a failure, not a degraded success —
                    // the caller refunds on `failed`, and would not on
                    // `completed`. The poller applies the identical rule when it
                    // is the path that observes completion first.
                    let err = "provider reported success without a mesh asset";
                    tracing::warn!(generation_id = %id, "3d completed without a mesh asset, failing");
                    resp.status = GenerationStatus::Failed;
                    resp.error = Some(err.to_string());
                    resp.assets.clear();
                    if let Some(refunded) = fail_model3d_row(&state, &id, resp.progress as i32, err).await {
                        if refunded > 0.0 {
                            resp.usage = Some(usage_from_row_cost(0.0));
                        }
                        dispatch_3d_webhook(&state, row.as_ref(), &resp).await;
                    } else {
                        resp = decided_or(&state, &id, resp).await;
                    }
                }
            } else if resp.status == GenerationStatus::Failed {
                let err = resp.error.clone().unwrap_or_else(|| "3d generation failed".to_string());
                if let Some(refunded) = fail_model3d_row(&state, &id, resp.progress as i32, &err).await {
                    if refunded > 0.0 {
                        resp.usage = Some(usage_from_row_cost(0.0));
                    }
                    dispatch_3d_webhook(&state, row.as_ref(), &resp).await;
                } else {
                    resp = decided_or(&state, &id, resp).await;
                }
            }
            let owner = row.as_ref().map_or_else(|| FileOwner::made_by(&id, key_ctx.as_ref()), FileOwner::of);
            answer_3d(&state, key_ctx.as_ref(), owner, resp).await
        }
        Err(e) => {
            // Fall through to the row we ALREADY tenant-checked above — never a
            // second, unchecked read. Once the poller has terminalised a job the
            // router no longer tracks it, which is the common way to get here.
            if let Some(gen) = row {
                return answer_3d(&state, key_ctx.as_ref(), FileOwner::of(&gen), model3d_response_from_row(&gen)).await;
            }
            generation_error_response(&e)
        }
    }
}

/// The answer after this observer's guarded terminal write did NOT take: if
/// another observer (a cancel, the poller, a reap) already decided the row,
/// that decision is the answer, because clients stop on the first terminal
/// status they see and every later read returns the row. On anything else (a
/// DB error, no row yet) the router's own verdict stands.
async fn decided_or(state: &AppState, id: &str, resp: Model3dGenerationResponse) -> Model3dGenerationResponse {
    match state.db.get_generation(id).await {
        Ok(Some(gen)) if gen.status.is_terminal() => model3d_response_from_row(&gen),
        _ => resp,
    }
}

/// Terminalise a 3D row as `failed`, guarded, and settle it. `Some` with the
/// amount refunded when THIS call wrote it (so the caller dispatches the
/// webhook and reports the refund); `None` when another observer decided first.
async fn fail_model3d_row(state: &AppState, id: &str, progress: i32, error: &str) -> Option<f64> {
    match state.db.update_generation_if_active(
        id, "failed", progress, None, Some(error), Some(chrono::Utc::now()), None,
    ).await {
        Ok(w) if w.owns_outcome() => {}
        Ok(_) => return None, // the poller (or a cancel) got there first
        Err(e) => {
            tracing::warn!(generation_id = %id, error = %e, "failed to persist the 3d failure");
            return None;
        }
    }
    Some(crate::proxy::poller::settle_terminal(&state.db, id, "failed", Some(error)).await)
}

/// Fire the terminal webhook for a 3D generation this handler terminalised.
///
/// The poller is normally the only dispatch site, but it only ever sees rows
/// that are still `pending`/`processing`: when the client's GET terminalises
/// first (the SDK polls every 2s, the poller ticks every 5s) the row never
/// appears in its window again and the caller got NO delivery at all. Called
/// only when the guarded UPDATE reported this observer as the writer, so the
/// webhook still fires at most once per generation.
async fn dispatch_3d_webhook(
    state: &AppState,
    row: Option<&crate::types::Generation>,
    resp: &Model3dGenerationResponse,
) {
    // No row means the submit handler's spawned insert has not landed; there is
    // no key_id to look a webhook_url up on, and the poller will drive the row
    // once it exists.
    let Some(row) = row else { return };
    if row.key_id.is_none() {
        return;
    }
    let gen = crate::types::Generation {
        status: resp.status,
        progress: resp.progress as i32,
        result_url: resp.mesh().map(|m| m.url.clone()),
        error_message: resp.error.clone(),
        completed_at: Some(chrono::Utc::now()),
        metadata: if resp.assets.is_empty() {
            row.metadata.clone()
        } else {
            Some(serde_json::json!({ "assets": resp.assets }))
        },
        ..row.clone()
    };
    crate::proxy::poller::dispatch_terminal_webhook(
        &state.db,
        &crate::proxy::webhook::webhook_client(state.mode),
        state.mode,
        gen,
        Some(crate::access::present::Presenting::of(state)),
    ).await;
}

/// POST /v1/models3d/cost — Estimate cost for a 3D generation.
#[utoipa::path(
    post,
    path = "/v1/models3d/cost",
    request_body = Model3dGenerationRequest,
    responses(
        (status = 200, description = "Cost estimate", body = CostEstimate,
            headers(
                ("X-Litegen-Dropped-Params" = String, description = "Comma-separated names of the fields and params this request sent that the route or model does not take, dropped instead of refused because `strict` was false; a field inside an object is named by its path (`reference_images[0].weight`). At most 32 names: any further name, and any name that is not 1–64 visible ASCII characters without a comma, is counted in a last `+N more`. Absent when nothing was dropped."),
            )),
        (status = 400, description = "Bad request", body = ErrorResponse),
        (status = 408, description = "`body_timeout`: the request body did not arrive within `server.request_body_timeout_secs` (default 300 s).", body = ErrorResponse),
        (status = 413, description = "`body_too_large`: the request body is over 25 MiB, as declared or as sent.", body = ErrorResponse),
        (status = 503, description = "`server_at_capacity`: litegen is running as many requests as it may (`backpressure.max_in_flight`), or holds as many request bodies as it may; retry shortly (`Retry-After` is set).", body = ErrorResponse),
    ),
    tag = "Models3D"
)]
pub async fn estimate_3d_cost(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    validated: ValidatedModel3d,
) -> impl IntoResponse {
    // Same credential resolution as `generate_3d` (and `estimate_video_cost`).
    // A single-tenant install commonly stores its 3D vendor key as an ORG
    // credential with no platform default: generations worked while this
    // endpoint answered 424 for the very same model, and aipix calls it before
    // every non-mock generation.
    let app_creds = match resolve_org_provider_credential(&state, &key_ctx, &validated.schema.provider).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if app_creds.is_none() && !state.router.has_model3d_provider(&validated.schema.provider).await {
        return provider_not_configured_response(&validated.schema.provider);
    }
    match state.router.estimate_model3d_cost(&validated.schema, &validated.request, app_creds).await {
        Ok(est) => with_dropped((StatusCode::OK, Json(serde_json::to_value(est).unwrap())).into_response(), &validated.dropped),
        Err(e) => {
            generation_error_response(&e)
        }
    }
}

/// Write a completed 3D result to its generation row: `result_url` is the
/// primary mesh (so every existing cross-modal consumer keeps working), and the
/// full asset list rides `metadata.assets`.
/// Returns whether THIS call terminalised the row. `false` means the row was
/// already terminal (the poller, or a cancel, got there first) or the write
/// failed — in both cases the caller must not dispatch a webhook.
pub(crate) async fn persist_model3d_result(
    state: &AppState,
    resp: &Model3dGenerationResponse,
    requested_formats: &[String],
) -> bool {
    let mesh_url = resp.mesh().map(|m| m.url.clone());
    // The terminal write MERGES into `metadata` (`db::merge_metadata`), so what
    // submit recorded survives it. The requested formats are re-stated anyway:
    // a row whose submit-time write was lost then still carries them, instead
    // of `model3d_response_from_row` falling back to the GLB floor.
    let meta = serde_json::json!({
        "assets": resp.assets,
        "requested_formats": requested_formats,
    });
    // ONE guarded, atomic UPDATE. Split across two statements, a transient
    // failure of the second left the row permanently `completed` with metadata
    // NULL — out of the poller's window, with the router job dropped — so every
    // later read reported `completed` with no assets: the state Ruling R21
    // forbids, where the consumer refunds and litegen billed.
    match state.db.update_generation_if_active(
        &resp.id, "completed", resp.progress as i32,
        mesh_url.as_deref(), None, Some(chrono::Utc::now()), Some(&meta),
    ).await {
        Ok(w) if w.owns_outcome() => {}
        Ok(_) => {
            tracing::info!(generation_id = %resp.id, "3d row was already terminal, leaving it untouched");
            return false;
        }
        Err(e) => {
            tracing::warn!(generation_id = %resp.id, error = %e, "failed to persist 3d status");
            return false;
        }
    }
    // The submit handler logged the request `pending` under this same id.
    if let Err(e) = state.db.update_request_log_status(&resp.id, "completed", None).await {
        tracing::warn!(generation_id = %resp.id, error = %e, "failed to mark the 3d request log completed");
    }
    true
}

// ─── Models ─────────────────────────────────────────────────────────────────

/// Optional filters for `GET /v1/models`.
#[derive(serde::Deserialize)]
pub struct ListModelsQuery {
    /// Optional `image` | `video` | `model3d` filter.
    #[serde(default)]
    pub media_type: Option<String>,
    /// List every model the gateway serves, not only the ones the caller's
    /// org pool and app access allow (generations still enforce both). The
    /// dashboard's Providers page edits the org's pool from this list.
    #[serde(default)]
    pub all: bool,
}

/// GET /v1/models — The models the caller may use (`?all=true`: every model the gateway serves).
#[utoipa::path(
    get,
    path = "/v1/models",
    params(
        ("media_type" = Option<String>, Query, description = "Filter by media type"),
        ("all" = Option<bool>, Query, description = "true: every model the gateway serves, including those outside the caller's org pool and app access, which generations still enforce. Default: only the models the caller may use."),
    ),
    responses(
        (status = 200, description = "List of available models", body = ModelListResponse),
    ),
    tag = "Models"
)]
pub async fn list_models(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Query(q): Query<ListModelsQuery>,
) -> impl IntoResponse {
    // Only the models this tenant may generate with (org pool / app access).
    let allowed = match crate::management::enforce::tenant_model_filter(
        &state,
        key_ctx.as_ref().and_then(|c| c.org_id.as_deref()),
        key_ctx.as_ref().and_then(|c| c.app_id.as_deref()),
    ).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(error = %e, "model access lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response("internal server error", 500))).into_response();
        }
    };
    let mut served = state.registry.served();
    served.sort_by(|a, b| a.id.cmp(&b.id));
    // Available means a generation would reach the vendor (L14): the caller's
    // org stored its own credential for the provider, or litegen holds a
    // platform default for the model's medium — the test a generation makes
    // before it answers provider_not_configured.
    let own: std::collections::HashSet<String> = match (key_ctx.as_ref().and_then(|c| c.org_id.as_deref()), state.secrets_key) {
        (Some(org), Some(_)) => match state.db.list_org_provider_credentials(org).await {
            Ok(creds) => creds.into_iter().map(|c| c.provider).collect(),
            Err(e) => {
                tracing::warn!(error = %e, "provider credential lookup failed; listing platform availability only");
                Default::default()
            }
        },
        _ => Default::default(),
    };
    let mut platform: std::collections::HashMap<(String, &'static str), bool> = std::collections::HashMap::new();
    for m in &served {
        let key = (m.provider.clone(), m.media_type.as_str());
        if platform.contains_key(&key) {
            continue;
        }
        let has = match m.media_type {
            crate::capabilities::MediaType::Image => state.router.has_image_provider(&m.provider).await,
            crate::capabilities::MediaType::Video => state.router.has_video_provider(&m.provider).await,
            crate::capabilities::MediaType::Model3d => state.router.has_model3d_provider(&m.provider).await,
        };
        platform.insert(key, has);
    }
    let models: Vec<ModelInfo> = served.iter()
        .filter(|m| q.all || allowed.as_ref().is_none_or(|set| set.contains(&m.id)))
        .map(|m| ModelInfo {
            is_available: own.contains(&m.provider) || platform[&(m.provider.clone(), m.media_type.as_str())],
            ..project_model_info(m)
        })
        .filter(|m| match q.media_type.as_deref() {
            None | Some("") => true,
            Some(want) => serde_json::to_value(m.media_type)
                .ok()
                .and_then(|v| v.as_str().map(|s| s == want))
                .unwrap_or(false),
        })
        .collect();
    Json(ModelListResponse { object: "list".to_string(), data: models, catalog_version: state.registry.catalog_version() }).into_response()
}

/// GET /v1/models/{id} — Full schema for one model, as a caller should read it.
///
/// Served, not raw. `params.output_formats` lists every container a caller may
/// request — the vendor's own plus what litegen derives locally — with
/// `derivable_values` saying which share one vendor job (see
/// `ParamSpecStringArray`). A model still declaring the superseded scalar
/// `output_format` is served under the canonical plural, so a client only ever
/// has one spelling to render.
#[utoipa::path(
    get,
    path = "/v1/models/{id}",
    params(("id" = String, Path, description = "Model ID (may contain slashes, e.g. 'openai/dall-e-3')")),
    responses(
        (status = 200, description = "Model schema", body = crate::capabilities::ModelSchema),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 410, description = "Retired: `model_retired`, with `replaced_by` naming its successor", body = ErrorResponse),
    ),
    tag = "Models"
)]
pub async fn get_model_schema(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.registry.resolve(&id) {
        Some(schema) => (
            StatusCode::OK,
            Json(serde_json::to_value(served_model_schema(&schema)).unwrap()),
        ).into_response(),
        None => match state.registry.retired(&id) {
            Some(m) => (StatusCode::GONE, Json(crate::api::middleware::validator::retired_body(&m))).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(error_response(&format!("model '{}' not found", id), 404)),
            ).into_response(),
        },
    }
}

/// A catalog entry as the API presents it: `output_formats` widened from what
/// the vendor emits to what a caller may request. Everything else verbatim.
pub(crate) fn served_model_schema(
    schema: &crate::capabilities::ModelSchema,
) -> crate::capabilities::ModelSchema {
    let mut served = schema.clone();
    served.status = schema.status();
    if let Some(native) = schema.output_formats_spec() {
        served.params.remove("output_format");
        served.params.insert(
            "output_formats".to_string(),
            crate::capabilities::ParamSpec::StringArray(
                crate::proxy::model3d_convert::served_output_formats_spec(&native),
            ),
        );
    }
    served
}

fn project_model_info(s: &crate::capabilities::ModelSchema) -> ModelInfo {
    let region = match s.params.get("region") {
        Some(crate::capabilities::ParamSpec::Region(r)) => Some(r),
        _ => None,
    };
    let takes_mask = s.ref_inputs.as_ref().is_some_and(|r| r.roles.contains_key("mask"));
    let keyframes = s.ref_inputs.as_ref().and_then(|r| r.roles.get("keyframe"));
    // An image model's containers (PixelGenie L9): its output_format values.
    let image_formats: Vec<String> = match s.params.get("output_format") {
        Some(crate::capabilities::ParamSpec::String(f)) if s.media_type == crate::capabilities::MediaType::Image => f.enum_values.clone(),
        _ => Vec::new(),
    };
    ModelInfo {
        status: s.status(),
        deprecated_at: s.deprecated_at.clone(),
        retires_at: s.retires_at.clone(),
        replaced_by: s.replaced_by.clone(),
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
            task: s.task,
            supports_text_to_image: s.capabilities.text_to_image,
            supports_image_to_image: s.capabilities.image_to_image,
            // "Takes a mask" (3D doc §4), read the way the validator reads it.
            supports_inpainting: takes_mask,
            supports_text_to_video: s.capabilities.text_to_video,
            supports_image_to_video: s.capabilities.image_to_video,
            supports_first_frame: s.ref_inputs.as_ref().is_some_and(|ri| ri.roles.contains_key("first_frame")),
            supports_last_frame: s.ref_inputs.as_ref().is_some_and(|ri| ri.roles.contains_key("last_frame")),
            supported_sizes: extract_sizes(s),
            max_reference_images: s.ref_inputs.as_ref().map(|ri| ri.max_total).unwrap_or(1),
            max_images: s.ref_inputs.as_ref().map(|ri| ri.max_total).unwrap_or(1),
            max_duration_seconds: match s.params.get("duration_seconds") {
                Some(crate::capabilities::ParamSpec::Float(f)) => f.max,
                Some(crate::capabilities::ParamSpec::Int(i)) => i.max.map(|m| m as f64),
                _ => None,
            },
            supports_text_to_3d: s.capabilities.text_to_3d,
            supports_image_to_3d: s.capabilities.image_to_3d,
            supports_multiview_to_3d: s.capabilities.multiview_to_3d,
            // Read the way the validator reads them — a declared Bool — so a
            // capability can never say yes to a param the validator refuses.
            supports_pbr: declares_bool(s, "pbr"),
            supports_rig: declares_bool(s, "rig"),
            supports_animation: matches!(s.params.get("animations"), Some(crate::capabilities::ParamSpec::AnimationList(_))),
            supports_skeleton: s.params.contains_key("skeleton"),
            supports_joint_naming: s.params.contains_key("joint_naming"),
            supports_texture: declares_bool(s, "texture"),
            supports_source_model: matches!(s.params.get("source_model"), Some(crate::capabilities::ParamSpec::SourceModel(_))),
            output_formats: if s.media_type == crate::capabilities::MediaType::Image { image_formats.clone() } else { extract_output_formats(s) },
            max_polycount: extract_max_polycount(s),
            supports_region_edit: region.is_some(),
            region_input: region.map(|r| r.input),
            preserves_unmasked: s.preserves_unmasked,
            mask_convention: takes_mask.then_some(crate::capabilities::MaskConvention::WhiteEdits),
            supports_keyframes: keyframes.is_some(),
            max_keyframes: keyframes.map(|k| k.max_count),
            keyframe_timing: keyframes.and_then(|k| k.timing),
            supports_transparent_background: matches!(
                s.params.get("background"),
                Some(crate::capabilities::ParamSpec::String(b)) if b.enum_values.iter().any(|v| v == "transparent")
            ),
            lossy_only: !image_formats.is_empty() && !image_formats.iter().any(|f| f == "png"),
            supports_fixed_camera: matches!(
                s.params.get("camera"),
                Some(crate::capabilities::ParamSpec::String(c)) if c.enum_values.iter().any(|v| v == "fixed")
            ),
            supports_audio_toggle: matches!(s.params.get("audio"), Some(crate::capabilities::ParamSpec::Bool(_))),
            supports_loop: matches!(s.params.get("loop"), Some(crate::capabilities::ParamSpec::Bool(_))),
            duration_values: match s.params.get("duration_seconds") {
                Some(crate::capabilities::ParamSpec::Float(f)) => f.values.clone(),
                Some(crate::capabilities::ParamSpec::Int(i)) => i.values.iter().map(|v| *v as f64).collect(),
                _ => Vec::new(),
            },
            supports_background_removal: s.capabilities.background_removal,
            texture_resolutions: match s.params.get("texture_resolution") {
                Some(crate::capabilities::ParamSpec::Int(i)) => i.values.iter().map(|v| *v as u32).collect(),
                _ => Vec::new(),
            },
            supports_keep_uvs: declares_bool(s, "keep_uvs"),
            supports_placement: s.media_type == crate::capabilities::MediaType::Model3d,
            supports_expand: matches!(s.params.get("expand"), Some(crate::capabilities::ParamSpec::Expand(_))),
            supports_layout: matches!(s.params.get("layout"), Some(crate::capabilities::ParamSpec::Layout(_))),
            layout_kinds: match s.params.get("layout") {
                Some(crate::capabilities::ParamSpec::Layout(l)) => l.kinds.clone(),
                _ => Vec::new(),
            },
            supports_mask_prompt: matches!(s.params.get("mask_prompt"), Some(crate::capabilities::ParamSpec::String(_))),
            supports_preserve_unmasked: takes_mask || region.is_some(),
            supports_mask_grow: takes_mask || region.is_some(),
            supports_source_video: matches!(s.params.get("source_video"), Some(crate::capabilities::ParamSpec::SourceVideo(_))),
            native_pixel_art: s.capabilities.native_pixel_art,
            pixel_size: match s.params.get("size") {
                Some(crate::capabilities::ParamSpec::Size(crate::capabilities::SizeSpec::Freeform(f))) if s.capabilities.native_pixel_art => {
                    Some(crate::types::PixelSize { min: f.min_width.min(f.min_height), max: f.max_width.max(f.max_height) })
                }
                _ => None,
            },
            supports_palette: matches!(s.params.get("palette"), Some(crate::capabilities::ParamSpec::Palette(_))),
            supports_forced_palette: matches!(s.params.get("palette"), Some(crate::capabilities::ParamSpec::Palette(p)) if p.forced),
            directions: match s.params.get("directions") {
                Some(crate::capabilities::ParamSpec::Int(i)) => i.values.iter().map(|v| *v as u32).collect(),
                _ => Vec::new(),
            },
            extend_output: match s.params.get("source_video") {
                Some(crate::capabilities::ParamSpec::SourceVideo(v)) if s.task == crate::capabilities::ModelTask::Extend => v.extend_output,
                _ => None,
            },
            supports_time_range: matches!(s.params.get("time_range"), Some(crate::capabilities::ParamSpec::TimeRange(_))),
            // PixelGenie L7, read the way the segmentation endpoint reads it.
            supports_segmentation: segmentation_prompts(s).is_some(),
            segmentation_prompts: segmentation_prompts(s).map(|p| p.types.clone()).unwrap_or_default(),
        },
        pricing: Some(ModelPricing {
            base_cost_usd: s.pricing.base_cost_usd,
            variable_pricing: s.pricing.variable_pricing.clone(),
        }),
        tags: s.tags.clone(),
    }
}

/// A segmentation model's prompt spec (PixelGenie L7): `None` for any other
/// model.
fn segmentation_prompts(s: &crate::capabilities::ModelSchema) -> Option<&crate::capabilities::ParamSpecSegmentationPrompts> {
    match s.params.get("prompts") {
        Some(crate::capabilities::ParamSpec::SegmentationPrompts(p)) if s.task == crate::capabilities::ModelTask::Segment => Some(p),
        _ => None,
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

fn declares_bool(s: &crate::capabilities::ModelSchema, param: &str) -> bool {
    matches!(s.params.get(param), Some(crate::capabilities::ParamSpec::Bool(_)))
}

/// Every container a caller may request: the same list, in the same order, as
/// `GET /v1/models/{id}` serves under `params.output_formats.enum_values`.
fn extract_output_formats(s: &crate::capabilities::ModelSchema) -> Vec<String> {
    // Goes through `output_formats_spec` so both the canonical plural and the
    // superseded scalar spelling advertise correctly. Reading one variant
    // directly is how this silently returned `[]` for every 3D model the day
    // the spec kind changed.
    //
    // Advertises what the model can DELIVER, including containers litegen
    // derives locally — the validator accepts exactly this set, and a
    // capability list narrower than what the API accepts would send clients
    // looking for a vendor that emits obj when we can make one from the glb.
    s.output_formats_spec()
        .map(|sp| crate::proxy::model3d_convert::served_output_formats_spec(&sp).enum_values)
        .unwrap_or_default()
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
///
/// Checks each provider configured with the gateway's own keys, side by
/// side, and reports what each vendor answered. It reads the deployment's
/// own state, so it takes the master key, or, in single-tenant mode, the
/// `read` scope in the deployment's org.
#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Health check results: one entry per provider and modality, each checked with the gateway's own keys", body = HealthResponse),
        (status = 403, description = "Provider health covers the gateway's own vendor keys, so it is the deployment's: the master key, or, in single-tenant mode, the `read` scope in the deployment's org", body = ErrorResponse),
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
            enabled: state.router.settings().cache.enabled,
            entries: state.router.cache.entry_count().await,
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
/// org-scoped BYO credentials — so it does NOT gate readiness. No vendor is
/// called: the body lists the providers configured with the gateway's own
/// keys, by name, and `GET /health` checks them.
/// Returns 503 only when the DB is unreachable. No auth required.
/// Live: `curl https://app.litegen.ai/api/health/ready`
#[utoipa::path(
    get,
    path = "/health/ready",
    responses(
        (status = 200, description = "Service is ready: the database answered. No vendor is asked, so a slow vendor never fails readiness", body = ReadinessResponse),
        (status = 503, description = "Service is not ready", body = ReadinessResponse),
    ),
    tag = "System"
)]
pub async fn readiness(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let db_ok = state.db.ping().await.is_ok();

    // Readiness gates on the DB only: provider availability is resolved
    // per-request (global config OR org-scoped BYO credentials), so an empty
    // global-provider list must not mark a BYO deployment "not_ready" — it serves
    // fine. Nor does it ask a vendor (P1, code review 2026-11-11): an
    // anonymous probe would spend the operator's keys, and a slow vendor
    // would fail a probe with a short timeout.
    let body = ReadinessResponse {
        status: if db_ok { "ready".to_string() } else { "not_ready".to_string() },
        checks: ReadinessChecks {
            db: db_ok,
            providers: state.router.registry.configured_provider_names().await,
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
///
/// `metadata.<key>=<value>` (repeatable) narrows them to the requests whose
/// `metadata` holds every pair: spend per project, per asset (L17).
#[utoipa::path(
    get,
    path = "/v1/stats",
    responses(
        (status = 200, description = "Usage statistics", body = ProxyStats),
        (status = 400, description = "A `metadata.<key>` filter names a key metadata cannot hold", body = ErrorResponse),
    ),
    tag = "Dashboard"
)]
pub async fn get_stats(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Query(pairs): Query<Vec<(String, String)>>,
) -> impl IntoResponse {
    // Tenant scope: require an active org; stats are computed over org (+ app when set).
    let org_id = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    let app_id = key_ctx.as_ref().and_then(|c| c.app_id.as_deref());
    let metadata = match metadata_filter(&pairs) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state.db.get_stats_for_tenant(org_id, app_id, &metadata).await {
        Ok(stats) => (StatusCode::OK, Json(serde_json::to_value(stats).unwrap())).into_response(),
        Err(e) => {
            internal_error_response("Failed to get stats", &e)
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
    /// CSV of scopes (default: "generate,read"). `generate`: create generations,
    /// quote costs, and read the model catalog and the key's generations, so a
    /// `generate` key is an AI-only key. `read`: read the catalog, generations,
    /// request logs, stats, spend, limits and end users. `admin`: manage keys,
    /// budgets, end users and settings. A key gets only scopes its creator may
    /// grant.
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
        if let Err(resp) = crate::api::middleware::validate_grant(ctx, &request.scopes) {
            return resp;
        }
    }

    // Mint a pk_live_/sk_live_ id+secret pair. The secret_hash is what auth looks up.
    let kp = crate::auth::secrets::generate_key_pair();

    // The key belongs to the person who makes it, and a key made by a key to
    // that key's maker, so it ends with them (the middleware's
    // `resolve_db_key`). Keys made with the master key, or by a key that has
    // no owner, belong to the org.
    let owner_user_id = match acting_person(&state, key_ctx.as_ref()).await {
        Ok(owner) => owner,
        Err(resp) => return resp,
    };

    match state.db.create_api_key_scoped(
        org_id, app_id, &kp.public_id, &request.name,
        &kp.secret_hash, &kp.prefix,
        request.token_quota, request.rpm_limit,
        &request.scopes, request.webhook_url.as_deref(),
        request.expires_at,
    ).await {
        Ok(key) => {
            if let Some(ref uid) = owner_user_id {
                if let Err(e) = state.db.set_api_key_owner(&key.id, uid).await {
                    // Without its owner the key would belong to the org and
                    // outlive its maker. Its secret never leaves this call, so
                    // revoking it is only tidying up.
                    error!(error = %e, "Failed to record an API key's owner");
                    let _ = state.db.revoke_api_key(&key.id).await;
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response("could not record the key's owner", 500))).into_response();
                }
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
            internal_error_response("Failed to create API key", &e)
        }
    }
}

/// The person a key made or rotated by `ctx` acts for, so that it ends with
/// them: the signed-in person (a dashboard session or an OAuth grant), or the
/// owner of the calling key. `None` for the master key, the dev bypass and a
/// calling key that belongs to the org. A calling key whose row is gone (its
/// org or app was deleted mid-request) is refused, since what it made would
/// belong to nobody and outlive everyone.
async fn acting_person(state: &AppState, ctx: Option<&KeyContext>) -> Result<Option<String>, axum::response::Response> {
    let Some(ctx) = ctx else { return Ok(None) };
    if let Some(user) = &ctx.user {
        return Ok(Some(user.user_id.clone()));
    }
    let Some(key_id) = ctx.key_id else { return Ok(None) };
    match state.db.get_api_key(&key_id).await {
        Ok(Some(key)) => Ok(key.owner_user_id),
        Ok(None) => Err((StatusCode::UNAUTHORIZED, Json(error_response("the calling API key no longer exists", 401))).into_response()),
        Err(e) => {
            error!(error = %e, "Failed to look up the calling key");
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response("could not look up the calling key", 500))).into_response())
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
            internal_error_response("Failed to list API keys", &e)
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
        Err(e) => return internal_error_response("Failed to revoke API key", &e),
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
                        Err(e) => return internal_error_response("Failed to revoke API key", &e),
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
            internal_error_response("Failed to revoke API key", &e)
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
        Err(e) => internal_error_response("Failed to get API key", &e),
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
    let existing = match state.db.get_api_key(&id).await {
        Ok(Some(k)) if k.org_id.as_deref() == Some(ctx_org) => k,
        Ok(_) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => return internal_error_response("Failed to update API key", &e),
    };
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
                        Err(e) => return internal_error_response("Failed to update API key", &e),
                        _ => {}
                    }
                } else {
                    return (StatusCode::FORBIDDEN, Json(error_response("key:write:own permission required", 403))).into_response();
                }
            }
        }
    }
    // The same privilege check as creation: without it a Member could create a
    // `generate,read` key and then edit it to `admin`.
    if let (Some(ctx), Some(scopes)) = (key_ctx.as_ref(), req.scopes.as_deref()) {
        if let Err(resp) = crate::api::middleware::validate_grant(ctx, scopes) {
            return resp;
        }
    }
    // And the key's own owner must be able to hold what it's given: a key acts
    // for its owner, so `admin` on a Member's key would be dropped at every
    // request, an edit that answers 200 and does nothing.
    if let Some(scopes) = req.scopes.as_deref() {
        if let Err(resp) = crate::api::middleware::validate_owner_can_hold(&state, &existing, scopes).await {
            return resp;
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
        Err(e) => internal_error_response("Failed to update API key", &e),
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
                return internal_error_response("Failed to list generations", &e);
            }
        };
        let total_pages = crate::api::pagination::total_pages(total, per_page);
        let mut paged = paged;
        present_rows(&state, Some(&ctx), &mut paged).await;
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
            return internal_error_response("Failed to list generations", &e);
        }
    };

    let total_pages = crate::api::pagination::total_pages(total as u64, per_page);
    let mut gens = gens;
    present_rows(&state, Some(&ctx), &mut gens).await;
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
        (status = 200, description = "Cancelled generation, and whether its vendor took the cancel", body = crate::types::CancelledGeneration),
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
            return internal_error_response("Failed to get generation for cancel", &e);
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
            // Stop the job, don't just relabel the row. The router held the
            // in-flight handle, so the next GET /v1/models3d/{id} (or /videos/)
            // polled the provider anyway, saw `Completed`, re-hosted the assets
            // and persisted `completed` OVER this cancel. Dropping the handle
            // here is what makes cancellation actually stop an in-flight job;
            // `get_3d_status`'s terminal-row read-through covers the poller-owned
            // and racing-poll cases. Both are no-ops when nothing is tracked.
            let tracked = state.router.forget_model3d_job(&id).await;
            state.router.forget_video_job(&id).await;
            // ...and stop it at the VENDOR, which otherwise keeps working on a
            // job nobody will collect — and bills for it (L18). The answer says
            // whether the vendor took it, so it waits briefly; a slow vendor is
            // still asked, in the background, and reads as not cancelled.
            let asked = tokio::spawn({
                let (state, gen) = (state.clone(), gen.clone());
                async move {
                    match gen.media_type.as_str() {
                        "model3d" => cancel_model3d_at_vendor(state, gen, tracked).await,
                        "video" => cancel_video_at_vendor(state, gen).await,
                        _ => None,
                    }
                }
            });
            let vendor_cancelled = matches!(
                tokio::time::timeout(VENDOR_CANCEL_WAIT, asked).await,
                Ok(Ok(Some(crate::providers::CancelOutcome::Requested)))
            );
            // A cancelled job leaves the poller's active window, so this is its
            // only settlement: the `pending` request log (same id) takes the
            // outcome, and what the job was billed goes back.
            crate::proxy::poller::settle_terminal(&state.db, &id, "cancelled", None).await;
            // Every other way a job ends sends the terminal webhook; so does this.
            crate::proxy::poller::dispatch_terminal_webhook(
                &state.db, &crate::proxy::webhook::webhook_client(state.mode), state.mode, updated.clone(),
                Some(crate::access::present::Presenting::of(&state)),
            ).await;
            // The row as settled: a refunded job reads `cost_usd: 0`.
            let updated = match state.db.get_generation(&id).await {
                Ok(Some(row)) => row,
                _ => updated,
            };
            log_audit(
                state.db.clone(),
                Some(&ctx),
                "generation.cancel",
                "generation",
                &id,
                None,
                serde_json::to_value(&updated).ok(),
            );
            let answer = crate::types::CancelledGeneration { generation: updated, vendor_cancelled };
            (StatusCode::OK, Json(serde_json::to_value(&answer).unwrap())).into_response()
        }
        Ok(None) => (StatusCode::CONFLICT, Json(error_response(
            "Generation cannot be cancelled: not in pending or processing state", 409,
        ))).into_response(),
        Err(e) => {
            internal_error_response("Failed to cancel generation", &e)
        }
    }
}

/// How long `PATCH /v1/generations/{id}` waits on the vendor's cancel before
/// answering `vendor_cancelled: false`.
const VENDOR_CANCEL_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Ask a cancelled video generation's vendor to stop working on it, rebuilt
/// from its row as the poller rebuilds it, with the org's own credential
/// where it has one. `None` when the vendor could not be asked.
async fn cancel_video_at_vendor(
    state: Arc<AppState>,
    gen: crate::types::Generation,
) -> Option<crate::providers::CancelOutcome> {
    let provider_job_id = gen.provider_job_id.clone()?;
    let creds = crate::proxy::poller::resolve_gen_credential(&state.db, state.secrets_key, &gen).await;
    let Some(provider) = state.router.registry.video_provider_for_request(&gen.provider, creds).await else {
        tracing::warn!(generation_id = %gen.id, provider = %gen.provider, "video cancel: provider not configured, the vendor job is left running");
        return None;
    };
    let handle = crate::providers::VideoGenerationHandle { provider_job_id, provider: gen.provider.clone(), model: gen.model.clone() };
    match provider.cancel(&handle).await {
        Ok(outcome) => {
            tracing::info!(generation_id = %gen.id, ?outcome, "video cancel: vendor answered");
            Some(outcome)
        }
        Err(e) => {
            tracing::warn!(generation_id = %gen.id, error = %e, "video cancel: vendor call failed; the row stays cancelled");
            None
        }
    }
}

/// Ask a cancelled 3D generation's vendor to stop working on it. Best-effort:
/// the row is already `cancelled` whatever the vendor answers; the answer
/// only decides what `vendor_cancelled` reports.
///
/// A job this process is not tracking — the poller drove it, or it was
/// submitted before a restart — is rebuilt from its row exactly as the poller
/// rebuilds it, with the org's own credential where it has one: the vendor
/// knows the job only under the key that submitted it.
async fn cancel_model3d_at_vendor(
    state: Arc<AppState>,
    gen: crate::types::Generation,
    tracked: Option<(crate::providers::Model3dGenerationHandle, Arc<dyn crate::providers::Model3dProvider>)>,
) -> Option<crate::providers::CancelOutcome> {
    let (handle, provider) = match tracked {
        Some(job) => job,
        None => {
            let provider_job_id = gen.provider_job_id.clone()?;
            let creds = crate::proxy::poller::resolve_gen_credential(&state.db, state.secrets_key, &gen).await;
            let Some(provider) = state.router.registry.model3d_provider_for_request(&gen.provider, creds).await else {
                tracing::warn!(generation_id = %gen.id, provider = %gen.provider, "3d cancel: provider not configured, the vendor job is left running");
                return None;
            };
            let handle = crate::providers::Model3dGenerationHandle {
                provider_job_id,
                provider: gen.provider.clone(),
                model: gen.model.clone(),
                stage_context: gen.metadata.as_ref().and_then(|m| m.get("stage_context").cloned()),
            };
            (handle, provider)
        }
    };
    match provider.cancel(&handle).await {
        Ok(outcome) => {
            tracing::info!(generation_id = %gen.id, ?outcome, "3d cancel: vendor answered");
            Some(outcome)
        }
        Err(e) => {
            tracing::warn!(generation_id = %gen.id, error = %e, "3d cancel: vendor call failed; the row stays cancelled");
            None
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
            return internal_error_response("Failed to fetch key for rotate", &e);
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

    // Whoever receives the new secret holds the only one that works, so the
    // key acts for them from now on and ends when they leave, as a key they
    // made would (`resolve_db_key`). The master key, the dev bypass and a key
    // that belongs to the org act for the deployment: the key keeps its owner.
    let new_owner = match acting_person(&state, key_ctx.as_ref()).await {
        Ok(owner) => owner,
        Err(resp) => return resp,
    };

    // 2. Mint a fresh id/secret pair and update the same row in place.
    let kp = crate::auth::secrets::generate_key_pair();
    let rotated = match state.db.rotate_api_key(&id, &kp.public_id, &kp.secret_hash, &kp.prefix, new_owner.as_deref()).await {
        Ok(Some(k)) => k,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(error_response("Key not found", 404))).into_response(),
        Err(e) => {
            return internal_error_response("Failed to rotate key", &e);
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
            return internal_error_response("Failed to fetch key for test-webhook", &e);
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
        files_deleted_at: None,
        files_deleted_reason: None,
    };

    // Don't follow redirects: a public webhook host must not be able to 3xx the
    // request into an internal target. In hosted mode, connect only to public
    // addresses too, like every webhook dispatch.
    let client = crate::proxy::webhook::webhook_client(state.mode);
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
///
/// `metadata.<key>=<value>` (repeatable) keeps the requests whose `metadata`
/// holds every pair (L17).
#[utoipa::path(
    get,
    path = "/v1/logs",
    params(
        ("page" = Option<u32>, Query, description = "Page number"),
        ("per_page" = Option<u32>, Query, description = "Items per page"),
    ),
    responses(
        (status = 200, description = "Request logs", body = PaginatedResponse<RequestLog>),
        (status = 400, description = "A `metadata.<key>` filter names a key metadata cannot hold", body = ErrorResponse),
    ),
    tag = "Dashboard"
)]
pub async fn get_logs_filtered(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Query(query): Query<LogsQuery>,
    Query(pairs): Query<Vec<(String, String)>>,
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
    let metadata = match metadata_filter(&pairs) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let filter = crate::db::RequestLogFilter {
        model: query.model.as_deref(),
        provider: query.provider.as_deref(),
        status: query.status.as_deref(),
        from: query.from.as_deref().and_then(parse_rfc3339_utc),
        to: query.to.as_deref().and_then(parse_rfc3339_utc),
        metadata: &metadata,
    };
    let (rows, total) = match state.db
        .get_request_logs_for_tenant_filtered(org_id, app_id, &filter, page, per_page)
        .await
    {
        Ok(pair) => pair,
        Err(e) => {
            return internal_error_response("Failed to get tenant logs", &e);
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

/// The `metadata.<key>=<value>` pairs of a `/v1/logs` or `/v1/stats` query
/// (L17), or the 400 for a key no request's `metadata` can hold.
fn metadata_filter(pairs: &[(String, String)]) -> Result<Vec<(String, String)>, axum::response::Response> {
    pairs
        .iter()
        .filter_map(|(k, v)| Some((k.strip_prefix("metadata.")?, v)))
        .map(|(k, v)| {
            if crate::api::middleware::validator::is_metadata_key(k) {
                Ok((k.to_string(), v.clone()))
            } else {
                Err((StatusCode::BAD_REQUEST, Json(error_response(
                    &format!("metadata.{k}: a metadata key is 1-64 letters, digits, '_', '.' or '-'"), 400,
                ))).into_response())
            }
        })
        .collect()
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
        Err(e) => return internal_error_response("Failed to list webhook deliveries", &e),
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
            internal_error_response("Failed to list webhook deliveries", &e)
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
            return internal_error_response("Failed to list audit log", &e);
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
        (status = 503, description = "`cache_clear_incomplete`: this replica's cache was emptied, but the shared cache in Redis could not be cleared, so the other replicas keep their answers; retry once Redis answers (`Retry-After` is set).", body = ErrorResponse),
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
            // The cache is the deployment's: cleared from the default org,
            // by a role holding cache:clear or one of its admin-scope keys.
            crate::config::Mode::SingleTenant => {
                let from_deployment = crate::api::middleware::acts_in_default_org(ctx)
                    || crate::api::middleware::is_platform_admin(ctx);
                if !from_deployment
                    || (ctx.user.is_some()
                        && !ctx.permissions.contains(&crate::auth::permissions::Permission::CacheClear))
                {
                    return forbidden_perm_resp("cache:clear");
                }
            }
        }
    }
    // Clear, then tell the other replicas to empty their local tiers (without
    // Redis the announcement does nothing, and there are no other replicas).
    // A task of its own: a client that goes away must not leave the shared tier
    // part way cleared, with no epoch raised and no one told. A clear that did
    // not reach the shared tier raised no epoch, so telling the others would
    // change nothing: it says so instead.
    let clearing = state.clone();
    let cleared = crate::management::runtime::finish_even_if_dropped_with(async move {
        let cleared = clearing.router.cache.clear().await;
        if cleared.is_ok() {
            crate::management::runtime::announce(&clearing, crate::shared::events::SharedEvent::CacheClear).await;
        }
        cleared
    })
    .await;
    match cleared {
        Some(Ok(())) => Json(CacheClearedResponse { cleared: true }).into_response(),
        // The Redis error is in the log; the answer is fixed.
        Some(Err(_)) | None => cache_clear_incomplete(),
    }
}

/// The 503 for a `DELETE /v1/cache` that emptied this replica's cache but not
/// the shared one.
fn cache_clear_incomplete() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [("retry-after", "1")],
        Json(serde_json::json!({ "error": {
            "message": "Only this replica's cache was emptied. The shared cache in Redis could not be cleared, so the \
                        other replicas keep their answers. Retry once Redis answers.",
            "type": "cache_clear_incomplete",
            "code": "cache_clear_incomplete"
        } })),
    )
        .into_response()
}

// ─── OpenAPI Specification ──────────────────────────────────────────────────

/// GET /openapi.json — Serve the OpenAPI specification.
pub async fn openapi_spec() -> impl IntoResponse {
    use utoipa::OpenApi;
    Json(crate::api::openapi::ApiDoc::openapi())
}

// ─── Router Setup ───────────────────────────────────────────────────────────

pub fn create_router(state: Arc<AppState>) -> axum::Router {
    create_router_with_hooks(state, None)
}

/// The router with generation call hooks (`docs/hooks.md`) around the
/// `Generate` routes: `pre_call` before authentication, on the raw request,
/// and `post_call` on the finished response.
/// With `None` the routes are exactly as [`create_router`] makes them.
pub fn create_router_with_hooks(state: Arc<AppState>, hooks: Option<Arc<crate::hooks::HookSet>>) -> axum::Router {
    use axum::routing::{delete, get, patch, post};
    use axum::middleware;
    use super::metrics::metrics_handler;
    use super::middleware::{auth_middleware, check_any_scope, check_generation_principal, check_scope, csrf_middleware, Scope};

    // The request fields the validator knows (E2) come from the OpenAPI
    // document: build them now, so no request waits for the document and a
    // router that builds has read every request schema.
    for name in crate::api::openapi::REQUEST_SCHEMAS {
        crate::api::openapi::request_fields(name);
    }

    let auth_state = state.clone();
    let csrf_state = state.clone();

    // Generate routes — require Scope::Generate
    // Every billed POST honours an `Idempotency-Key`. Layered per route, so it
    // runs inside authentication and sees whose key it is.
    let idempotent = || middleware::from_fn_with_state(state.clone(), super::middleware::idempotency::idempotency_middleware);
    let generate_routes = axum::Router::new()
        .route("/v1/images/generations", post(generate_image).route_layer(idempotent()))
        .route("/v1/images/segmentations", post(segment_image).route_layer(idempotent()))
        .route("/v1/images/cost", post(estimate_image_cost))
        .route("/v1/videos/generations", post(generate_video).route_layer(idempotent()))
        .route("/v1/videos/cost", post(estimate_video_cost))
        .route("/v1/videos/{id}", get(get_video_status))
        .route("/v1/models3d/generations", post(generate_3d).route_layer(idempotent()))
        .route("/v1/models3d/cost", post(estimate_3d_cost))
        // Registered after the two literal-segment routes above so they win the
        // match; axum/matchit gives static segments precedence over a capture
        // regardless of declaration order, but the ordering is kept anyway for
        // readability. The only theoretical collision is a generation id
        // literally equal to "assets" — unreachable, since ids are minted
        // `litegen-3d-{uuid}`.
        .route("/v1/models3d/{id}", get(get_3d_status))
        // Standalone conversion: no provider, no generation row, no billing.
        // Behind Generate rather than Read because it is CPU work on
        // caller-supplied bytes, not a lookup.
        .route("/v1/utils/models3d/convert", post(convert::convert_model3d))
        // A person needs an org and `generation:create` (runs after the scope check).
        .layer(middleware::from_fn(check_generation_principal))
        .layer(middleware::from_fn(|req: axum::extract::Request, next: middleware::Next| async {
            check_scope(Scope::Generate, req, next).await
        }))
        // CSRF: no-ops for safe methods and Bearer/API-key auth; only enforces
        // on mutating cookie-session requests. Same layer as auth_required_routes.
        .layer(middleware::from_fn_with_state(
            csrf_state.clone(),
            csrf_middleware,
        ));

    // AI read routes — the model catalog and the caller's generations. Open to
    // `generate` as well as `read`, so a `generate` key is a complete AI-only
    // key (LiteLLM's `llm_api` key type) and `read` stays the scope for the
    // management reads below.
    let ai_read_routes = axum::Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/models/{*id}", get(get_model_schema))
        .route("/v1/providers", get(list_providers))
        .route("/v1/utils/models3d/formats", get(convert::list_model3d_convert_formats))
        .route("/v1/generations", get(list_generations))
        .route("/v1/generations/{id}", get(get_generation))
        .route("/v1/generations/{id}", patch(cancel_generation))
        .route("/v1/generations/{id}/assets", get(list_generation_assets))
        .layer(middleware::from_fn(|req: axum::extract::Request, next: middleware::Next| async {
            check_any_scope(&[Scope::Generate, Scope::Read], req, next).await
        }))
        // CSRF: no-ops for safe methods and Bearer/API-key auth; only enforces
        // on mutating cookie-session requests (e.g. PATCH cancel_generation).
        .layer(middleware::from_fn_with_state(
            csrf_state.clone(),
            csrf_middleware,
        ));

    // Changing a generation's files or links (docs/storage-access.md): what a
    // key that can make generations may do to the ones it owns.
    let generation_change_routes = axum::Router::new()
        .route("/v1/generations/{id}/revoke-links", post(revoke_generation_links))
        .route("/v1/generations/{id}", delete(delete_generation))
        .layer(middleware::from_fn(check_generation_principal))
        .layer(middleware::from_fn(|req: axum::extract::Request, next: middleware::Next| async {
            check_scope(Scope::Generate, req, next).await
        }))
        .layer(middleware::from_fn_with_state(
            csrf_state.clone(),
            csrf_middleware,
        ));

    // Read routes — require Scope::Read
    let read_routes = axum::Router::new()
        // The gateway's own vendor keys, checked: the deployment's to read
        // (P1, code review 2026-11-11). `route_layer`, so it wraps this route only.
        .merge(
            axum::Router::new()
                .route("/health", get(health_check))
                .route_layer(middleware::from_fn_with_state(state.mode, super::middleware::require_deployment_reader)),
        )
        .route("/v1/stats", get(get_stats))
        .route("/v1/logs", get(get_logs_filtered))
        .route("/v1/logs/{id}/artifact", get(get_log_artifact))
        .layer(middleware::from_fn(|req: axum::extract::Request, next: middleware::Next| async {
            check_scope(Scope::Read, req, next).await
        }))
        // CSRF: no-ops for safe methods and Bearer/API-key auth; same layer as
        // the other route groups.
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
        .route("/v1/apps/{app_id}/access", get(orgs::get_app_access).put(orgs::put_app_access))
        .route("/v1/apps/{app_id}/access/revoke-links", post(orgs::revoke_app_links))
        .route(
            "/v1/orgs/{id}/allowed-models",
            get(orgs::get_org_allowed_models).put(orgs::put_org_allowed_models),
        )
        .route(
            "/v1/apps/{app_id}/allowed-models",
            get(orgs::get_app_allowed_models).put(orgs::put_app_allowed_models),
        )
        // MCP OAuth consent: the dashboard's consent page reads a sign-in
        // request and records the signed-in person's decision.
        .route("/v1/oauth/requests/{id}", get(crate::mcp::oauth::authorize::get_request))
        .route("/v1/oauth/requests/{id}/approve", post(crate::mcp::oauth::authorize::approve_request))
        .route("/v1/oauth/requests/{id}/deny", post(crate::mcp::oauth::authorize::deny_request))
        // The apps a person has connected through OAuth.
        .route("/v1/oauth/grants", get(crate::mcp::oauth::grants::list_grants))
        .route("/v1/oauth/grants/{id}", delete(crate::mcp::oauth::grants::revoke_grant))
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

    // litegen's file route (`docs/storage-access.md`): open to `generate` and
    // `read` like the other AI reads (a generate-only key fetches its own
    // outputs), and hooked like the generation routes, so custom auth
    // reaches a file and `file_access` can narrow who opens it.
    let file_routes = axum::Router::new()
        .route("/v1/generations/{id}/assets/{index}", get(get_generation_asset))
        .layer(middleware::from_fn(|req: axum::extract::Request, next: middleware::Next| async {
            check_any_scope(&[Scope::Generate, Scope::Read], req, next).await
        }))
        .layer(middleware::from_fn_with_state(
            csrf_state.clone(),
            csrf_middleware,
        ));
    let file_routes = match hooks.clone() {
        Some(hooks) => file_routes
            .layer(Extension(hooks.clone()))
            .layer(middleware::from_fn_with_state(hooks.clone(), crate::hooks::post_call_layer))
            .layer(auth_layer.clone())
            .layer(middleware::from_fn_with_state(hooks, crate::hooks::pre_call_layer)),
        None => file_routes.layer(auth_layer.clone()),
    };

    // The generation routes carry their own auth layer so hooks can sit
    // around it: `pre_call` outside, on the request before authentication;
    // `post_call` inside, where the authenticated caller is known.
    let generate_routes = match hooks {
        Some(hooks) => generate_routes
            .layer(middleware::from_fn_with_state(hooks.clone(), crate::hooks::post_call_layer))
            .layer(auth_layer.clone())
            .layer(middleware::from_fn_with_state(hooks, crate::hooks::pre_call_layer)),
        None => generate_routes.layer(auth_layer.clone()),
    };

    // The groups that authorize people by role carry no scope layer of their
    // own (a browser session holds every scope); an OAuth principal still
    // needs its grant's scope for each call there.
    let grant_scope = || middleware::from_fn(super::middleware::check_grant_scope);

    // The process's metrics (docs/mcp/self-hosting.md): where the config puts
    // them, or nowhere; behind a credential that may read the deployment
    // when `metrics_require_auth` asks for one. `route_layer`, not `layer`:
    // a layer would also wrap this router's fallback, which `api` takes over
    // from the last router merged into it, putting every path litegen
    // doesn't serve behind the same credential.
    let observability = &state.router.config.observability;
    let metrics_routes = if !observability.metrics_enabled {
        axum::Router::new()
    } else if observability.metrics_require_auth {
        axum::Router::new()
            .route(&observability.metrics_path, get(metrics_handler))
            .route_layer(middleware::from_fn_with_state(state.mode, super::middleware::require_deployment_reader))
            .route_layer(auth_layer.clone())
    } else {
        axum::Router::new().route(&observability.metrics_path, get(metrics_handler))
    };

    let authenticated_routes = axum::Router::new()
        .merge(ai_read_routes)
        .merge(read_routes)
        .merge(generation_change_routes)
        .merge(admin_routes)
        .merge(auth_required_routes.layer(grant_scope()))
        .merge(management::routes(csrf_state.clone()).layer(grant_scope()))
        .layer(auth_layer);

    let api = axum::Router::new()
        .merge(generate_routes)
        .merge(file_routes)
        .merge(authenticated_routes)
        // Unauth routes — no auth middleware needed, no CSRF
        .merge(unauth_routes)
        .merge(metrics_routes)
        // Health probes, openapi, and mock video bytes don't require auth
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness))
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
        // A link litegen signed is its own permission, so no auth layer.
        .route("/v1/generations/{id}/assets/{index}/signed", get(get_signed_generation_asset))
        .route("/v1/refs/{token}", get(get_published_ref))
        .with_state(state.clone());

    // MCP (`/mcp`, `/admin/mcp`) replays every tool call through `api`, the
    // router above, so it gets the HTTP API's own checks.
    if state.router.config.mcp.enabled {
        let mcp = crate::mcp::router(state, api.clone());
        api.merge(mcp)
    } else {
        api
    }
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
            let mut artifact = artifact;
            // The read rule of `GET /v1/generations/{id}`: a session holding
            // only `generation:read:own` reads the artifacts of its own keys'
            // generations. A request that made no row (one that failed) is
            // attributed to no generation, so it takes `generation:read:any`.
            let row = state.db.get_generation(&artifact.request_id).await.ok().flatten();
            match row.as_ref() {
                Some(gen) => {
                    if let Err(resp) = authorize_generation_for_session(
                        &state, key_ctx.as_ref(), gen,
                        crate::auth::permissions::Permission::GenerationReadAny,
                        crate::auth::permissions::Permission::GenerationReadOwn,
                        "generation:read:own",
                    ).await {
                        return resp;
                    }
                }
                None => {
                    let read_own_session = key_ctx.as_ref().is_some_and(|c| {
                        c.user.is_some() && !c.permissions.contains(&crate::auth::permissions::Permission::GenerationReadAny)
                    });
                    if read_own_session {
                        return forbidden_perm_resp("generation:read:any");
                    }
                }
            }
            // An output kept by URL goes out as the app's access mode says,
            // like the generation it belongs to (`docs/storage-access.md`).
            if let (true, Some(stored)) = (artifact.output_kind == "url", artifact.output_value.clone()) {
                let (owner, index) = match row.as_ref() {
                    Some(gen) => (FileOwner::of(gen), generation_assets(gen).iter().position(|a| a.url == stored)),
                    None => (
                        FileOwner {
                            generation_id: artifact.request_id.clone(),
                            key_id: None,
                            app_id: artifact.app_id.clone(),
                            org_id: artifact.org_id.clone(),
                        },
                        None,
                    ),
                };
                let presenter = presenter_for(&state, key_ctx.as_ref(), &owner).await;
                artifact.output_value = Some(presenter.url(index, &stored).await.0);
            }
            // An image the log keeps inline (base64) goes out as the file route
            // hands that file out. Where the app keeps files to their owners
            // (`signed`, `private`), a caller outside the owner scope gets the
            // route, which refuses it, never the bytes.
            // With no row (its write failed), the key that made it is unknown, so
            // only a caller that passes the scope without one gets the bytes.
            if artifact.output_kind == "b64" && artifact.output_value.is_some() {
                let owner = match row.as_ref() {
                    Some(gen) => FileOwner::of(gen),
                    None => FileOwner {
                        generation_id: artifact.request_id.clone(),
                        key_id: None,
                        app_id: artifact.app_id.clone(),
                        org_id: artifact.org_id.clone(),
                    },
                };
                let policy = crate::access::policy_for(&state, owner.app_id.as_deref()).await;
                if policy.mode != crate::access::AccessMode::Public
                    && !crate::access::present::in_owner_scope(policy.owner_scope, key_ctx.as_ref(), &owner)
                {
                    // Withheld: always `url`, the file route (which refuses
                    // this caller) or none when nothing was kept, and no
                    // bytes, so nothing is truncated either.
                    let kept = row.as_ref().map(generation_assets).unwrap_or_default();
                    artifact.output_kind = "url".to_string();
                    artifact.output_truncated = false;
                    artifact.output_value = match kept.iter().position(|a| a.index == 0) {
                        Some(i) => {
                            let presenter = presenter_for(&state, key_ctx.as_ref(), &owner).await;
                            Some(presenter.url(Some(i), &kept[i].url).await.0)
                        }
                        None => None,
                    };
                }
            }
            (StatusCode::OK, Json(serde_json::to_value(artifact).unwrap())).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(error_response("Artifact not found", 404))).into_response(),
        Err(e) => {
            internal_error_response("Failed to get request artifact", &e)
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

/// The query a signed link to a stored file carries (`docs/storage-access.md`).
#[derive(Debug, Default, serde::Deserialize)]
pub struct AssetLinkQuery {
    /// Unix seconds after which the link no longer works.
    pub expires: Option<String>,
    /// Unix milliseconds the link was issued at, which a revocation compares
    /// against. Absent on a link minted before links carried it.
    pub issued: Option<String>,
    /// Hex HMAC over the file's key, `issued` and `expires`.
    pub signature: Option<String>,
}

/// GET /v1/models3d/assets/{key} — Serve a result file litegen keeps, from
/// its store (disk, or the bucket of an S3 deployment), as a stream; one
/// `Range` is answered. Who may open it follows the app's access mode
/// (`docs/storage-access.md`).
pub async fn get_model3d_asset_bytes(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Query(link): Query<AssetLinkQuery>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    use crate::access::AccessMode;
    let not_found = || (StatusCode::NOT_FOUND, "not found").into_response();
    // Only litegen's own result files: never anything else under the asset
    // directory or in the bucket, and never a published reference, whose own
    // route checks its expiry.
    let prefix = [
        crate::proxy::storage::MODEL3D_PATH_PREFIX,
        crate::proxy::storage::VIDEO_PATH_PREFIX,
        crate::proxy::storage::IMAGE_RESULT_PATH_PREFIX,
    ]
    .into_iter()
    .find(|p| key.strip_prefix(p).is_some_and(|rest| rest.starts_with('/')));
    let Some(prefix) = prefix else { return not_found() };
    if key.split('/').any(|part| part == ".." || part.is_empty()) {
        return not_found();
    }
    // Every file lives under its generation's id, and the generation names
    // the app whose access mode decides who may open it.
    let Some(generation_id) = key[prefix.len() + 1..].split('/').next().filter(|id| !id.is_empty()) else {
        return not_found();
    };
    // The generation's app and that app's access settings, in one read
    // (review 2026-11-11, P14): an unsigned request needs them whatever the
    // order, since the mode that decides whether a signature is needed is
    // the app's.
    let (app_id, access) = match state.db.generation_file_access(generation_id).await {
        Ok(Some(found)) => found,
        // A generation that is gone (its app or org deleted) takes its files
        // with it, whatever the defaults say; the sweep removes them later.
        Ok(None) => return not_found(),
        Err(e) => {
            error!(generation_id = %generation_id, error = %e, "could not look up a stored file's generation");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response("could not check who may open this file", 500))).into_response();
        }
    };
    let policy = crate::access::effective(&state.router.config.storage_access, &state.router.overrides().gateway, access.as_ref());
    let cache_control = match policy.mode {
        // Meshes are immutable once written and keyed by generation id.
        AccessMode::Public => "public, max-age=31536000, immutable".to_string(),
        // Only the file route, which checks who is asking; not even a link
        // signed while the app was `signed`.
        AccessMode::Private => return not_found(),
        AccessMode::Signed => {
            let (Some(expires), Some(signature)) = (link.expires.as_deref().and_then(|e| e.parse::<i64>().ok()), link.signature.as_deref()) else {
                return not_found();
            };
            let issued = match link.issued.as_deref().map(str::parse::<i64>) {
                Some(Ok(i)) => Some(i),
                Some(Err(_)) => return not_found(),
                None => None,
            };
            if !state.router.link_signer.verify_local(&key, issued, expires, signature) {
                return not_found();
            }
            match revocation_check(&state, app_id.as_deref(), generation_id, issued).await {
                Ok(()) => {}
                Err(resp) => return resp,
            }
            let remaining = expires - chrono::Utc::now().timestamp();
            if remaining < 0 {
                let body = serde_json::json!({ "error": {
                    "type": "forbidden",
                    "code": "link_expired",
                    "message": "this link has expired; read the generation again for a fresh one",
                }});
                return (StatusCode::FORBIDDEN, Json(body)).into_response();
            }
            // Never cached past the link's own life.
            format!("private, max-age={}", remaining.min(3600))
        }
    };
    // Streamed, never read whole (see `stored_file_response`).
    match state.router.model3d_store.open(&key, requested_range(&headers)).await.ok().flatten() {
        Some(file) => {
            let headers = [
                (axum::http::header::CONTENT_TYPE, file.content_type.clone()),
                (axum::http::header::CACHE_CONTROL, cache_control),
                (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
                (axum::http::header::CONTENT_SECURITY_POLICY, "sandbox".to_string()),
            ];
            stored_file_response(file, headers)
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// GET /v1/refs/{token} — A reference litegen published for a vendor that
/// fetches its inputs only by URL (L4). No auth: vendors fetch it without
/// litegen's keys, and the token is unguessable and expires within the hour.
pub async fn get_published_ref(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> impl IntoResponse {
    let found = match state.materializer.publisher() {
        Some(p) => p.get(&token).await.ok().flatten(),
        None => None,
    };
    match found {
        Some((bytes, content_type)) => (
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, content_type),
                (axum::http::header::CACHE_CONTROL, "private, max-age=3600".to_string()),
                // On litegen's own origin: never sniffed into anything else,
                // and sandboxed if a browser renders it.
                (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
                (axum::http::header::CONTENT_SECURITY_POLICY, "sandbox".to_string()),
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
            crate::types::RefImageKind::Generation => format!("generation {}", r.value.chars().take(80).collect::<String>()),
        };
        serde_json::json!({
            "role": r.role,
            "kind": format!("{:?}", r.kind).to_lowercase(),
            "source_summary": source_summary,
        })
    }).collect();
    Some(serde_json::json!(summaries))
}

/// The request as its generation row records it (PixelGenie L2): enough to
/// audit the job and run it again, with each reference image identified by
/// its SHA-256 and size, never by its bytes. A URL reference keeps its URL.
pub(crate) fn request_record(
    base: &crate::types::BaseGenerationRequest,
    params: Option<serde_json::Value>,
    ctx: &crate::proxy::materializer::MaterializeContext,
) -> serde_json::Value {
    use base64::Engine;
    use sha2::Digest;
    // Every inline reference decoded and hashed: off the async workers (P15).
    let ref_bytes: usize = base
        .reference_images
        .iter()
        .map(|r| match r.kind {
            crate::types::RefImageKind::Base64 => r.value.len(),
            crate::types::RefImageKind::Blob => ctx.blob_parts.get(&r.value).map_or(0, |(bytes, _)| bytes.len()),
            _ => 0,
        })
        .sum();
    let refs: Vec<serde_json::Value> = crate::util::blocking::cpu_heavy(ref_bytes, || base.reference_images.iter().map(|r| {
        let digest = |bytes: &[u8]| serde_json::json!({
            "sha256": hex::encode(sha2::Sha256::digest(bytes)),
            "bytes": bytes.len(),
        });
        let mut entry = match r.kind {
            crate::types::RefImageKind::Url => serde_json::json!({ "url": r.value }),
            crate::types::RefImageKind::Generation => serde_json::json!({ "generation_id": r.value }),
            crate::types::RefImageKind::Base64 => {
                let raw = r.value.split_once(";base64,").map(|(_, b)| b).unwrap_or(&r.value);
                match base64::engine::general_purpose::STANDARD.decode(raw.trim()) {
                    Ok(bytes) => digest(&bytes),
                    Err(_) => serde_json::json!({ "undecodable": true }),
                }
            }
            crate::types::RefImageKind::Blob => match ctx.blob_parts.get(&r.value) {
                Some((bytes, _)) => digest(bytes),
                None => serde_json::json!({ "part": r.value }),
            },
        };
        entry["role"] = serde_json::json!(r.role);
        entry["type"] = serde_json::to_value(r.kind).unwrap_or(serde_json::Value::Null);
        entry
    }).collect());
    serde_json::json!({
        "model": base.model,
        "prompt": base.prompt,
        "negative_prompt": base.negative_prompt,
        "seed": base.seed,
        "n": base.n,
        "strict": base.strict,
        "params": params,
        "extra": base.extra,
        "reference_images": refs,
        "metadata": base.metadata,
    })
}

/// A generation that failed before anything was made (E7): what its
/// request-log row says.
struct FailedGeneration<'a> {
    /// `litegen-img`, `litegen-vid` or `litegen-3d`: the row's id prefix.
    id_prefix: &'static str,
    media_type: &'static str,
    schema: &'a crate::capabilities::ModelSchema,
    latency_ms: i64,
    error: &'a str,
    prompt: &'a str,
    metadata: Option<&'a serde_json::Value>,
    attributed: &'a crate::management::types::Attribution,
}

/// Log a failed generation (E7): a `failed` row at no cost naming the
/// model's provider, its attribution, and an error artifact for the logs'
/// drill-down. Awaited, so the row exists when the answer does (it counts in
/// `/v1/stats` at once); a write that fails is logged, never the request's
/// error. Not for a job litegen couldn't record: its own row says that.
async fn log_failed_generation(state: &AppState, key_ctx: Option<&KeyContext>, f: FailedGeneration<'_>) {
    let id = format!("{}-{}", f.id_prefix, Uuid::new_v4());
    let org_id = key_ctx.and_then(|c| c.org_id.clone());
    let app_id = key_ctx.and_then(|c| c.app_id.clone());
    let logged = state
        .db
        .log_request(&id, &f.schema.id, &f.schema.provider, "failed", f.media_type, 0.0, f.latency_ms, Some(f.error), f.metadata, org_id.as_deref(), app_id.as_deref())
        .await;
    if let Err(e) = logged {
        tracing::warn!(error = %e, request_id = %id, "failed to log a failed generation");
    }
    crate::management::enforce::attribute(&state.db, &id, f.attributed).await;
    let artifact = RequestArtifact {
        request_id: id,
        media_type: f.media_type.to_string(),
        prompt: Some(f.prompt.to_string()),
        negative_prompt: None,
        params_json: None,
        refs_meta_json: None,
        output_kind: "error".to_string(),
        output_value: None,
        output_mime: None,
        output_truncated: false,
        error_message: Some(f.error.to_string()),
        created_at: chrono::Utc::now(),
        org_id,
        app_id,
    };
    if let Err(e) = state.db.insert_request_artifact(&artifact).await {
        tracing::warn!(error = %e, "Failed to store error artifact");
    }
}

/// 500 for a submit the vendor accepted but whose row litegen could not write
/// (L2): answering with an id nothing can poll would hand the caller a job it
/// can neither follow nor resume. Marked as having reached the vendor, so a
/// retry with the same `Idempotency-Key` gets this answer back instead of a
/// second billed job.
fn generation_record_failed_response(id: &str, vendor_job_id: Option<&str>) -> axum::response::Response {
    // The cause (a database error) is in the server log, never the body.
    let mut resp = (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "error": {
                "message": format!("the vendor accepted the job, but litegen could not record generation {id}"),
                "type": "internal_error",
                "code": "generation_record_failed",
                "vendor_job_id": vendor_job_id,
            }
        })),
    )
        .into_response();
    resp.extensions_mut().insert(crate::api::middleware::idempotency::VendorAttempted);
    resp
}

/// A 400 `validation_error` whose `code` says which rule refused it.
fn validation_rejection_with_code(code: &str, message: &str, model: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    let body = serde_json::json!({
        "error": {
            "type": "validation_error",
            "message": message,
            "code": code,
            "model": model,
        }
    });
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

fn validation_rejection_response(message: &str, code: u16, model: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    let body = serde_json::json!({
        "error": {
            "type": "validation_error",
            "message": message,
            "code": "validation_error",
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
    // A signed-in person (session or OAuth grant) by email, as the org and
    // user audit entries name them; a key by its id; else the master key.
    let actor_label = key_ctx
        .and_then(|c| c.user.as_ref().map(|u| u.email.clone()).or_else(|| c.key_id.map(|id| id.to_string())))
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
            "error": { "message": "quota exceeded", "type": "quota_exceeded", "code": "quota_exceeded" }
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
) -> Result<QuotaHold, axum::response::Response> {
    let nothing = QuotaHold { key_id, reserved: 0.0, release_to: None };
    let Some(id) = key_id else { return Ok(nothing) };
    if estimate <= 0.0 {
        return Ok(nothing);
    }
    match state.db.reserve_tokens(&id, estimate).await {
        Ok(true) => Ok(QuotaHold { key_id, reserved: estimate, release_to: Some(state.db.clone()) }),
        Ok(false) => Err(quota_exceeded_response()),
        Err(e) => {
            tracing::error!(error = %e, key_id = %id, "quota reservation failed");
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                [("retry-after", "1")],
                Json(error_response("temporarily unable to authorize request", 503)),
            ).into_response())
        }
    }
}

/// A key's quota reservation from [`reserve_quota`]: the key it was taken
/// from, and how much. Settle it once, with [`settle_quota`]; a hold dropped
/// unsettled (its handler panicked) gives the reservation back by itself.
pub(crate) struct QuotaHold {
    key_id: Option<uuid::Uuid>,
    reserved: f64,
    release_to: Option<Arc<dyn crate::db::DatabaseStore>>,
}

impl Drop for QuotaHold {
    /// Gives back what was reserved and never settled, as
    /// `settle_quota(.., 0.0)` would, in a task of its own (a drop cannot
    /// await): nothing else ever lowers a key's `tokens_used`.
    fn drop(&mut self) {
        let (Some(key_id), Some(db)) = (self.key_id, self.release_to.take()) else { return };
        let reserved = std::mem::take(&mut self.reserved);
        if reserved <= 0.0 {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(key_id = %key_id, reserved, "a dropped quota reservation could not be given back outside a runtime");
            return;
        };
        runtime.spawn(async move {
            if let Err(e) = db.atomic_charge_tokens(&key_id, -reserved).await {
                tracing::warn!(error = %e, key_id = %key_id, "releasing a dropped quota reservation failed");
            }
        });
    }
}

/// Settle a prior [`reserve_quota`] hold once the generation resolves: adjust the
/// held amount to the `actual` cost. A negative delta refunds the
/// difference; `actual == 0.0` releases the whole hold (used when generation
/// fails so a failed request is never billed).
async fn settle_quota(state: &AppState, mut hold: QuotaHold, actual: f64) {
    let reserved = std::mem::take(&mut hold.reserved);
    hold.release_to = None;
    let Some(key_id) = hold.key_id else { return };
    let delta = actual - reserved;
    if delta.abs() > f64::EPSILON {
        if let Err(e) = state.db.atomic_charge_tokens(&key_id, delta).await {
            tracing::warn!(error = %e, key_id = %key_id, "quota settlement failed");
        }
    }
}

#[cfg(test)]
mod quota_hold_tests {
    use super::*;
    use crate::db::sqlite::SqliteDatabase;
    use crate::db::DatabaseStore;

    async fn tokens_used(db: &SqliteDatabase, key: &uuid::Uuid) -> f64 {
        db.get_api_key(key).await.unwrap().unwrap().tokens_used
    }

    /// E12: a key's quota reservation dropped unsettled (a billed handler
    /// that panicked) is given back, as `settle_quota(.., 0.0)` would. It
    /// used to stay held for good: nothing ever lowers `tokens_used` again.
    #[tokio::test]
    async fn an_unsettled_quota_hold_is_given_back_when_dropped() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let key = db.create_api_key("e12", "e12-hash", "lg-e12", Some(10.0), None, "generate", None).await.unwrap();
        let state = super::tenant_scoping_tests::build_state(db.clone(), crate::config::Mode::SingleTenant, None).await;
        let hold = reserve_quota(&state, Some(key.id), 0.5).await.ok().unwrap();
        assert_eq!(tokens_used(&db, &key.id).await, 0.5, "the estimate is reserved");
        drop(hold);
        // The release is a task the drop spawns.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut left = tokens_used(&db, &key.id).await;
        while left != 0.0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            left = tokens_used(&db, &key.id).await;
        }
        assert_eq!(left, 0.0, "a quota hold dropped unsettled left {left} USD reserved on the key");
    }

    /// A settled hold's charge stands when it is dropped.
    #[tokio::test]
    async fn a_settled_quota_hold_keeps_its_charge() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let key = db.create_api_key("e12", "e12-hash", "lg-e12", Some(10.0), None, "generate", None).await.unwrap();
        let state = super::tenant_scoping_tests::build_state(db.clone(), crate::config::Mode::SingleTenant, None).await;
        let hold = reserve_quota(&state, Some(key.id), 0.5).await.ok().unwrap();
        settle_quota(&state, hold, 0.3).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!((tokens_used(&db, &key.id).await - 0.3).abs() < 1e-9, "the settled charge stands");
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
                    return Err(internal_error_response("ownership check failed: list_api_keys_for_owner", &e));
                }
            },
        };
        if owns {
            return Ok(());
        }
    }
    Err(forbidden_perm_resp(any_label))
}

/// [`cache_scope`], narrowed to whoever may read a hit. A cache hit hands back
/// the earlier generation's image, bytes included, and its id, with no read
/// check of its own. So:
/// - a key gets its own scope where the app keeps files to the key that made
///   them (`signed` or `private`, `owner_scope: key`): two keys sharing an
///   entry would let one have the other's image, which the file route
///   refuses it;
/// - a person (a session or OAuth grant) without `generation:read:any`
///   (Member, Viewer) gets their own scope: they may read only their own
///   generations.
fn cache_scope_for(ctx: &KeyContext, policy: crate::access::AccessPolicy) -> String {
    let scope = cache_scope(ctx);
    let kept_to_the_key = policy.mode != crate::access::AccessMode::Public && policy.owner_scope == crate::access::OwnerScope::Key;
    match (ctx.key_id, ctx.user.as_ref()) {
        (Some(kid), None) if kept_to_the_key => format!("{scope}/k:{kid}"),
        (_, Some(user)) if !ctx.permissions.contains(&crate::auth::permissions::Permission::GenerationReadAny) => {
            format!("{scope}/u:{}", user.user_id)
        }
        _ => scope,
    }
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
            oauth_grant_id: None,
        }
    }

    /// Security fix 5 (2026-11-05, and its review). A cache hit hands back an
    /// earlier generation's image with no read check, so a caller who couldn't
    /// read that generation must not share its bucket:
    /// - a key, where the app keeps files to the key that made them;
    /// - a session or OAuth grant holding only `generation:read:own`.
    #[test]
    fn cache_scope_for_follows_who_may_read_the_hit() {
        use crate::access::{AccessMode, AccessPolicy, OwnerScope, SignedLinks};
        use crate::auth::permissions::Permission;
        let policy = |mode, owner_scope| AccessPolicy {
            mode,
            owner_scope,
            ttl: std::time::Duration::from_secs(3600),
            keep_outputs: true,
            retention_days: 0,
            signed_links: SignedLinks::default(),
        };
        let kid = uuid::Uuid::new_v4();
        let key = ctx(Some(kid), None, Some("orgA"), Some("app1"));
        let app_scope = cache_scope(&key);
        let with_perms = |perms: Vec<Permission>| KeyContext { permissions: perms, ..ctx(None, Some("u1"), Some("orgA"), Some("app1")) };
        let member = with_perms(vec![Permission::GenerationCreate, Permission::GenerationReadOwn]);
        let admin = with_perms(vec![Permission::GenerationCreate, Permission::GenerationReadAny]);

        for mode in [AccessMode::Signed, AccessMode::Private] {
            assert_eq!(cache_scope_for(&key, policy(mode, OwnerScope::Key)), format!("{app_scope}/k:{kid}"), "{mode:?}");
            assert_eq!(cache_scope_for(&key, policy(mode, OwnerScope::App)), app_scope, "{mode:?}: app scope shares");
        }
        assert_eq!(cache_scope_for(&key, policy(AccessMode::Public, OwnerScope::Key)), app_scope, "public shares");
        for mode in [AccessMode::Public, AccessMode::Signed, AccessMode::Private] {
            let p = policy(mode, OwnerScope::Key);
            assert_eq!(cache_scope_for(&member, p), format!("{}/u:u1", cache_scope(&member)), "{mode:?}: read:own reads only its own");
            assert_eq!(cache_scope_for(&admin, p), cache_scope(&admin), "{mode:?}: read:any may read any hit");
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

/// 400 `option_needs_keeping`: a request that keeps nothing (`keep_outputs:
/// false`) set an output option litegen applies to the file it keeps, so it
/// could not apply. `param` names the option; every opt-in output option
/// refuses this way (the mesh-gaps overview, "Options users choose").
pub(crate) fn option_needs_keeping(param: &str, what: &str, model: &str) -> axum::response::Response {
    let message = format!(
        "{param} needs a kept output: litegen {what} from the file it keeps, and this request keeps nothing (keep_outputs: false)"
    );
    param_rejection("option_needs_keeping", param, &message, model)
}

/// A 400 whose `code` names `param`: the body every request-option refusal
/// of a 3D generation has (`option_needs_keeping`, and `param_invalid` for a
/// value litegen does not take).
fn param_rejection(code: &str, param: &str, message: &str, model: &str) -> axum::response::Response {
    let body = crate::api::middleware::validator::err_body(code, message, Some(param), model);
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

/// 400 `invalid_keep_outputs_header`: an `X-Litegen-Keep-Outputs` other than `false`.
pub(crate) fn keep_header_rejection(message: &str) -> axum::response::Response {
    let body = serde_json::json!({ "error": {
        "message": message,
        "type": "invalid_request_error",
        "code": "invalid_keep_outputs_header",
    }});
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

/// A 500 that tells the caller nothing about its cause (SEC-DBERR): the
/// error goes to the server log at error level, with `context`, and the body
/// says "internal server error" (`internal_error`). A database error's own
/// text names tables, columns and constraints, and can echo the caller's
/// input.
fn internal_error_response(context: &str, e: &dyn std::fmt::Display) -> axum::response::Response {
    error!(error = %e, "{context}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response("internal server error", 500))).into_response()
}

/// An error body; its `code` is the error's type, a string like every `code`
/// litegen sends (PixelGenie L15).
fn error_response(message: &str, code: u16) -> serde_json::Value {
    let kind = match code {
        400 => "invalid_request_error",
        401 => "authentication_error",
        403 => "forbidden",
        404 => "not_found_error",
        424 => "provider_not_configured",
        429 => "rate_limit_error",
        502 => "provider_error",
        503 => "service_unavailable",
        504 => "vendor_timeout",
        _ => "internal_error",
    };
    serde_json::json!({ "error": { "message": message, "type": kind, "code": kind } })
}

/// What a caller's `Prefer` header (RFC 7240) asks of an image call: how
/// long to wait for a vendor's job before answering 202 that it runs on,
/// and the `Preference-Applied` value that says so. `respond-async` waits
/// for nothing (or for its `wait`); `wait=N` alone waits N seconds. Other
/// preferences, and a malformed `wait`, are ignored, as the RFC allows.
fn image_wait_preference(headers: &axum::http::HeaderMap) -> Option<(std::time::Duration, String)> {
    let mut async_asked = false;
    let mut wait: Option<u64> = None;
    for value in headers.get_all("prefer").iter().filter_map(|v| v.to_str().ok()) {
        for pref in value.split(',') {
            let token = pref.split(';').next().unwrap_or("").trim();
            let (name, arg) = match token.split_once('=') {
                Some((n, a)) => (n.trim(), Some(a.trim().trim_matches('"'))),
                None => (token, None),
            };
            if name.eq_ignore_ascii_case("respond-async") {
                async_asked = true;
            } else if name.eq_ignore_ascii_case("wait") {
                if let Some(n) = arg.and_then(|a| a.parse::<u64>().ok()) {
                    wait = Some(n);
                }
            }
        }
    }
    match (async_asked, wait) {
        (true, Some(n)) => Some((std::time::Duration::from_secs(n), format!("respond-async, wait={n}"))),
        (true, None) => Some((std::time::Duration::ZERO, "respond-async".to_string())),
        (false, Some(n)) => Some((std::time::Duration::from_secs(n), format!("wait={n}"))),
        (false, None) => None,
    }
}

/// An image job the vendor took that litegen delivers later.
struct LateImage {
    job: String,
    resume: Option<serde_json::Value>,
    response_format: String,
    price: f64,
    prefer: Option<(std::time::Duration, String)>,
}

/// Give a still-running image job its row, billed like a video submit (the
/// charge taken now, refunded if the vendor fails it later), for the poller
/// to finish (docs/async-images.md). A caller that asked for async hears
/// 202 with the generation; a sync caller hears the 504 it always did, now
/// naming the generation that will hold the image.
#[allow(clippy::too_many_arguments)]
async fn accept_late_image(
    state: &Arc<AppState>,
    key_ctx: Option<&crate::api::middleware::KeyContext>,
    validated: &ValidatedImage,
    extras: &ImageExtras,
    late: LateImage,
    reserved: QuotaHold,
    mut limits: crate::management::enforce::LimitGuard,
    attributed: crate::management::types::Attribution,
    start: std::time::Instant,
) -> axum::response::Response {
    let id = format!("litegen-img-{}", Uuid::new_v4());
    let model = validated.schema.id.clone();
    let provider = validated.schema.provider.clone();
    let org_id = key_ctx.and_then(|c| c.org_id.clone());
    let app_id = key_ctx.and_then(|c| c.app_id.clone());
    let latency = start.elapsed().as_millis() as i64;
    let db = state.db.clone();
    if let Err(e) = db
        .log_request(&id, &model, &provider, "pending", "image", late.price, latency, None, validated.request.base.metadata.as_ref(), org_id.as_deref(), app_id.as_deref())
        .await
    {
        tracing::warn!(generation_id = %id, error = %e, "failed to log the late image request");
    }
    // The request, for the Logs trace, as a video's submit keeps it: the
    // output is the generation's once the poller delivers it.
    let artifact = RequestArtifact {
        request_id: id.clone(),
        media_type: "image".to_string(),
        prompt: Some(validated.request.base.prompt.clone()),
        negative_prompt: validated.request.base.negative_prompt.clone(),
        params_json: serde_json::to_value(extras).ok(),
        refs_meta_json: build_refs_meta(&validated.request.base.reference_images),
        output_kind: "url".to_string(),
        output_value: None,
        output_mime: None,
        output_truncated: false,
        error_message: None,
        created_at: chrono::Utc::now(),
        org_id: org_id.clone(),
        app_id: app_id.clone(),
    };
    if let Err(e) = db.insert_request_artifact(&artifact).await {
        tracing::warn!(generation_id = %id, error = %e, "failed to store the late image's request artifact");
    }
    crate::management::enforce::attribute(&db, &id, &attributed).await;
    let inserted = db
        .insert_generation(&id, key_ctx.and_then(|c| c.key_id).as_ref(), &model, &provider, "image", Some(&late.job), late.price, org_id.as_deref(), app_id.as_deref())
        .await;
    // Charged now, as a video submit is; a job litegen could not record is
    // charged nothing, and its caller hears the timeout it would have.
    let charge = if inserted.is_ok() { late.price } else { 0.0 };
    settle_quota(state, reserved, charge).await;
    let charged_to = limits.charged_to();
    limits.settle(state, charge).await;
    if let Err(e) = inserted {
        tracing::error!(generation_id = %id, error = %e, "a late image's row could not be written");
        let _ = db.update_request_log_status(&id, "failed", Some("generation_record_failed")).await;
        return generation_error_response(&crate::proxy::router::ProxyError::VendorTimeout { model, provider, vendor_job_id: Some(late.job) });
    }
    let request = request_record(&validated.request.base, serde_json::to_value(extras).ok(), &validated.ctx);
    let recorded = serde_json::json!({
        "request": request,
        "billing": { "limits": charged_to },
        "end_user": attributed.end_user,
        "image_job": { "resume": late.resume, "response_format": late.response_format },
    });
    if let Err(e) = db.merge_generation_metadata_if_active(&id, &recorded).await {
        tracing::warn!(generation_id = %id, error = %e, "failed to record the late image request on its row");
    }
    let mut resp = match late.prefer {
        Some((_, applied)) => {
            let body = crate::types::ImageGenerationAccepted {
                id: id.clone(),
                object: "image.generation".to_string(),
                status: "processing".to_string(),
                model: model.clone(),
                created: chrono::Utc::now().timestamp(),
            };
            let mut resp = (StatusCode::ACCEPTED, Json(body)).into_response();
            if let Ok(v) = axum::http::HeaderValue::from_str(&format!("/v1/generations/{id}")) {
                resp.headers_mut().insert(axum::http::header::LOCATION, v);
            }
            if let Ok(v) = axum::http::HeaderValue::from_str(&applied) {
                resp.headers_mut().insert("preference-applied", v);
            }
            // What the request dropped, as its 200 would say (N1).
            with_dropped(resp, &validated.dropped)
        }
        None => {
            let e = crate::proxy::router::ProxyError::VendorTimeout { model: model.clone(), provider: provider.clone(), vendor_job_id: Some(late.job.clone()) };
            let body = serde_json::json!({
                "error": {
                    "message": format!("{e}; litegen delivers it to GET /v1/generations/{id} when it finishes"),
                    "type": "vendor_timeout",
                    "code": "vendor_timeout",
                    "retryable": false,
                    "vendor_job_id": late.job,
                    "generation_id": id,
                    "status": "processing",
                }
            });
            (StatusCode::GATEWAY_TIMEOUT, Json(body)).into_response()
        }
    };
    resp.extensions_mut().insert(crate::api::middleware::idempotency::VendorAttempted);
    resp
}

/// Store an image response's images and record it as a `completed` generation
/// (PixelGenie L3). Inline (`b64_json`) images are written to litegen's asset
/// store, the app's own bucket when it has one; an image already returned by
/// URL (an S3 image store) is recorded by that URL. Best-effort: the caller
/// already has the images. Returns what it recorded, in order (each entry's
/// `index` is its image's in the response); nothing when no row was written.
/// With `keep` false the row is written with no assets and nothing is stored.
async fn keep_image_result(
    state: &Arc<AppState>,
    key_ctx: Option<&crate::api::middleware::KeyContext>,
    response: &ImageGenerationResponse,
    validated: &ValidatedImage,
    cost: f64,
    end_user: Option<&str>,
    keep: bool,
) -> Vec<crate::types::GenerationAsset> {
    use base64::Engine;
    let app_id = key_ctx.and_then(|c| c.app_id.clone());
    let org_id = key_ctx.and_then(|c| c.org_id.clone());
    let target = match app_id.as_deref().filter(|_| keep) {
        Some(app) => match app_store_for_writing(&state.db, state.secrets_key, &state.router.config.aws, tenant_bucket_of(state), app).await {
            Ok(found) => Some(found.unwrap_or_else(|| (state.router.model3d_store.clone(), None))),
            // Its bucket is down and litegen's store may not stand in: the
            // caller has the images, and the row records none.
            Err(reason) => {
                tracing::warn!(generation_id = %response.id, reason = %reason, "an image result was not kept: the app's bucket can't be reached");
                None
            }
        },
        None => Some((state.router.model3d_store.clone(), None)),
    };
    let write = keep && target.is_some();
    let (store, prefix) = target.unwrap_or_else(|| (state.router.model3d_store.clone(), None));
    let mut assets: Vec<crate::types::GenerationAsset> = Vec::new();
    for (i, img) in response.data.iter().enumerate().filter(|_| write) {
        let format = crate::proxy::storage::image_format(&img.content_type);
        let asset = if let Some(b64) = img.b64_json.as_deref() {
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else { continue };
            let key = format!(
                "{}/{}/image_{i}.{format}",
                prefix.as_deref().unwrap_or(crate::proxy::storage::IMAGE_RESULT_PATH_PREFIX).trim_matches('/'),
                response.id
            );
            match store.put(&key, &bytes::Bytes::from(bytes.clone()), &img.content_type).await {
                Ok(url) => crate::types::GenerationAsset {
                    expires_at: None,
                    index: i as u32,
                    kind: "image".into(),
                    format: format.into(),
                    url,
                    size_bytes: Some(bytes.len() as u64),
                    sha256: Some(crate::proxy::router::sha256_hex(&bytes)),
                    width: None,
                    height: None,
                    duration_s: None,
                    fps: None,
                    has_audio: None,
                    kept: true,
                },
                Err(e) => {
                    tracing::warn!(generation_id = %response.id, error = %e, "could not keep an image result");
                    continue;
                }
            }
        } else if let Some(url) = img.url.as_deref() {
            crate::types::GenerationAsset {
                expires_at: None,
                index: i as u32,
                kind: "image".into(),
                format: format.into(),
                url: url.to_string(),
                size_bytes: None,
                sha256: None,
                width: None,
                height: None,
                duration_s: None,
                fps: None,
                has_audio: None,
                kept: true,
            }
        } else {
            continue;
        };
        assets.push(asset);
    }
    let key_id = key_ctx.and_then(|c| c.key_id);
    if let Err(e) = state.db.insert_generation(
        &response.id, key_id.as_ref(), &response.model, &response.provider, "image", None, cost,
        org_id.as_deref(), app_id.as_deref(),
    ).await {
        tracing::warn!(generation_id = %response.id, error = %e, "could not record the image result");
        return Vec::new();
    }
    let request = request_record(&validated.request.base, None, &validated.ctx);
    let mut metadata = serde_json::json!({ "assets": assets, "request": request, "end_user": end_user });
    if !keep {
        metadata["keep_outputs"] = false.into();
    }
    if let Err(e) = state.db.update_generation_if_active(
        &response.id, "completed", 100, assets.first().map(|a| a.url.as_str()), None, Some(chrono::Utc::now()),
        Some(&metadata),
    ).await {
        tracing::warn!(generation_id = %response.id, error = %e, "could not complete the image result's row");
    }
    assets
}

/// Read every `reference_images` entry that names a stored image generation
/// back into bytes (3D doc §2 rule 2), after the same access check
/// `GET /v1/generations/{id}` makes. The entry becomes an inline `base64`
/// reference, so every vendor form (`url`, `base64`, `multipart`) takes it.
async fn resolve_generation_refs(
    state: &Arc<AppState>,
    key_ctx: Option<&crate::api::middleware::KeyContext>,
    refs: &[crate::types::ReferenceImage],
) -> Result<Vec<crate::types::ReferenceImage>, crate::proxy::model3d_source::SourceError> {
    use crate::proxy::model3d_source::SourceError;
    use base64::Engine;
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        if r.kind != crate::types::RefImageKind::Generation {
            out.push(r.clone());
            continue;
        }
        let gen = chainable_generation(state, key_ctx, &r.value)
            .await
            .ok_or_else(|| SourceError::NotFound(format!("generation {} is not one you can read", r.value)))?;
        if gen.media_type != "image" {
            return Err(SourceError::NotFound(format!("generation {} is not an image", gen.id)));
        }
        if !gen.status.is_terminal() {
            return Err(SourceError::NotReady(format!("generation {} has not finished yet", gen.id)));
        }
        let asset = generation_assets(&gen)
            .into_iter()
            .find(|a| a.kind == "image")
            .ok_or_else(|| SourceError::NotFound(format!("generation {} has no stored image", gen.id)))?;
        let mut stores = vec![state.router.model3d_store.clone()];
        if let Some(app) = gen.app_id.as_deref() {
            if let Some((store, _)) = resolve_app_model3d_store(&state.db, state.secrets_key, &state.router.config.aws, tenant_bucket_of(state), app).await {
                stores.insert(0, store);
            }
        }
        let mut bytes = None;
        for store in &stores {
            if let Some(key) = store.key_of(&asset.url) {
                bytes = store.get(&key).await.ok().flatten();
                break;
            }
        }
        let (bytes, content_type) = bytes
            .ok_or_else(|| SourceError::NotFound(format!("generation {}'s image is no longer stored", gen.id)))?;
        out.push(crate::types::ReferenceImage {
            kind: crate::types::RefImageKind::Base64,
            value: format!("data:{content_type};base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes)),
            role: r.role.clone(),
            time_s: r.time_s,
        });
    }
    Ok(out)
}

/// The refusal for a reference image that names a generation litegen could not use.
fn reference_error_response(e: &crate::proxy::model3d_source::SourceError, model: &str) -> axum::response::Response {
    let status = StatusCode::from_u16(e.status()).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY);
    (status, Json(crate::api::middleware::validator::err_body(e.code(), &e.message(), Some("reference_images"), model))).into_response()
}

/// Read an operation's `source_model` and get it ready for the vendor (3D doc
/// §3A): a generation is read back from storage after the same access check
/// `GET /v1/generations/{id}` makes, a URL is fetched, base64 is decoded; then
/// litegen's own reader refuses what the vendor would refuse and converts the
/// rest to a container the model takes. Returns the adapter's input and the
/// lineage its row records.
async fn resolve_source_model(
    state: &Arc<AppState>,
    key_ctx: Option<&crate::api::middleware::KeyContext>,
    spec: &crate::capabilities::ParamSpecSourceModel,
    src: &crate::types::SourceRef,
    provider: &str,
    vendor_account: &str,
) -> Result<(crate::providers::SourceInput, serde_json::Value), crate::proxy::model3d_source::SourceError> {
    use crate::proxy::model3d_source::{decode_base64_source, fetch_url_source, prepare, read_generation_source, SourceError};
    let (bytes, format, origin) = match src.kind {
        crate::types::SourceKind::Generation => {
            let gen = chainable_generation(state, key_ctx, &src.value)
                .await
                .ok_or_else(|| SourceError::NotFound(format!("generation {} is not one you can read", src.value)))?;
            let mut stores = vec![state.router.model3d_store.clone()];
            if let Some(app) = gen.app_id.as_deref() {
                if let Some((store, _)) = resolve_app_model3d_store(&state.db, state.secrets_key, &state.router.config.aws, tenant_bucket_of(state), app).await {
                    stores.insert(0, store);
                }
            }
            let (bytes, format) = read_generation_source(&stores, &gen, spec).await?;
            (bytes, format, Some(gen))
        }
        crate::types::SourceKind::Url => {
            (fetch_url_source(&src.value, spec).await?, src.format.clone().unwrap_or_default(), None)
        }
        crate::types::SourceKind::Base64 => {
            (decode_base64_source(&src.value)?, src.format.clone().unwrap_or_default(), None)
        }
    };
    let format = format.to_ascii_lowercase();
    let spec_owned = spec.clone();
    let prepared = tokio::task::spawn_blocking(move || prepare(bytes, &format, &spec_owned))
        .await
        .map_err(|e| SourceError::FormatUnsupported(format!("reading the source failed: {e}")))??;
    // The vendor's own task id, when it made the source on this same account.
    // An animation's own task is no rig: one chained from an animation (or a
    // chain of them) gets the rig it came from, its recorded source, which is
    // what Tripo and Meshy animate.
    let mut chain_from = origin.clone();
    for _ in 0..8 {
        let Some(g) = chain_from.as_ref() else { break };
        let animation = state
            .registry
            .resolve(&g.model)
            .or_else(|| state.registry.retired(&g.model))
            .is_some_and(|m| m.task == crate::capabilities::ModelTask::Animate);
        if !animation {
            break;
        }
        let up = g.metadata.as_ref().and_then(|m| m["source"]["generation_id"].as_str()).map(str::to_string);
        chain_from = match up {
            Some(id) => state.db.get_generation(&id).await.ok().flatten(),
            None => None,
        };
    }
    let vendor_task_id = chain_from
        .as_ref()
        .filter(|g| g.provider == provider)
        .filter(|g| g.metadata.as_ref().and_then(|m| m.get("vendor_account")).and_then(|v| v.as_str()) == Some(vendor_account))
        .and_then(|g| g.provider_job_id.clone());
    // And the joint naming that rig was delivered in (3D doc §3B), from the
    // plan its row recorded, whichever provider made it: the vendor
    // animates the rig as it stored it, in its own names, so an animation
    // of it is delivered in this naming (`DeliveryPlan::for_request`). A
    // row whose plan is missing or does not read names none.
    let joint_naming = chain_from
        .as_ref()
        .and_then(|g| g.metadata.as_ref()?["delivery"]["joint_naming"].as_str().map(str::to_string));
    // Where the source's delivered mesh faces, by its own manifest: an
    // operation that keeps its source's axes faces there too.
    let front = origin.as_ref().and_then(|g| {
        g.metadata.as_ref()?["assets"]
            .as_array()?
            .iter()
            .find(|a| a["kind"] == "mesh")?["scene"]["front"]
            .as_str()
            .map(str::to_string)
    });
    let mut record = serde_json::json!({
        "generation_id": origin.as_ref().map(|g| g.id.clone()),
        "sha256": prepared.sha256,
        "format": prepared.format,
        "received_format": prepared.received_format,
        "facts": prepared.facts,
        "front": front,
    });
    if let Some(naming) = &joint_naming {
        record["joint_naming"] = serde_json::json!(naming);
    }
    let input = crate::providers::SourceInput {
        bytes: prepared.bytes,
        format: prepared.format,
        sha256: prepared.sha256,
        facts: prepared.facts,
        vendor_task_id,
        front,
        joint_naming,
        delivered: Default::default(),
    };
    Ok((input, record))
}

/// The largest init or mask litegen downloads to check a partial edit.
const MAX_EDIT_IMAGE_BYTES: usize = 50 * 1024 * 1024;

/// One reference's bytes, whatever form the materializer gave it.
async fn materialized_bytes(form: &crate::proxy::materializer::MaterializedRefForm) -> Result<Vec<u8>, String> {
    use crate::proxy::materializer::MaterializedRefForm;
    use base64::Engine;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.decode(s.trim()).map_err(|e| format!("not base64: {e}"));
    match form {
        MaterializedRefForm::Base64(s) => b64(s.rsplit_once(',').map_or(s.as_str(), |(_, d)| d)),
        MaterializedRefForm::MultipartField { bytes, .. } => Ok(bytes.to_vec()),
        MaterializedRefForm::Url(u) if u.starts_with("data:") => b64(u.split_once(',').map_or("", |(_, d)| d)),
        MaterializedRefForm::Url(u) => {
            use crate::util::ssrf::CappedDownloadError;
            crate::util::ssrf::check_public_url(u).await.map_err(|e| format!("the image URL was refused: {e}"))?;
            // A public host must not 3xx this fetch into a private one after
            // the check above, nor may its name resolve to one at connect.
            let client = crate::util::ssrf::public_only_download_client(std::time::Duration::from_secs(60), std::time::Duration::from_secs(30));
            match crate::util::ssrf::download_capped(&client, u, MAX_EDIT_IMAGE_BYTES as u64).await {
                Ok((bytes, _)) => Ok(bytes),
                Err(CappedDownloadError::Transport(why)) => Err(format!("the image could not be fetched: {why}")),
                Err(CappedDownloadError::Status(status)) => Err(format!("the image URL answered HTTP {status}")),
                Err(CappedDownloadError::TooLarge { .. }) => Err(format!("the image is over {MAX_EDIT_IMAGE_BYTES} bytes")),
            }
        }
    }
}

/// A partial edit (3D doc §4): the init image and mask read from the
/// materialized references, checked (`proxy::image_edit::prepare`), and the
/// mask replaced — or, for a region drawn as one, added — in the model's own
/// convention and form. The box and the vendor's growth travel in the
/// returned `ImageEdit`. An error is the caller's answer, before billing.
async fn apply_image_edit(
    schema: &crate::capabilities::ModelSchema,
    req: &ImageGenerationRequest,
    materialized: &mut crate::proxy::materializer::MaterializedRequest,
    publisher: Option<&Arc<crate::proxy::ref_publish::RefPublisher>>,
) -> Result<Option<crate::providers::ImageEdit>, axum::response::Response> {
    use crate::capabilities::{MaskConvention, ParamSpec, RefProviderFormat, RegionInput};
    use crate::proxy::image_edit::{prepare, EditError, EditRequest, EditTarget, RegionTarget};
    use crate::proxy::materializer::{MaterializedRef, MaterializedRefForm};
    let has_mask = materialized.refs.iter().any(|r| r.role == "mask");
    if req.region.is_none() && !has_mask {
        return Ok(None);
    }
    let refuse = |e: EditError| {
        let status = StatusCode::from_u16(e.status()).unwrap_or(StatusCode::BAD_REQUEST);
        (status, Json(crate::api::middleware::validator::err_body(e.code(), &e.message(), Some(e.param()), &schema.id))).into_response()
    };
    let refs = schema.ref_inputs.as_ref();
    let mask_role = refs.and_then(|r| r.roles.get("mask"));
    let target = EditTarget {
        convention: mask_role.and_then(|r| r.convention).unwrap_or(MaskConvention::WhiteEdits),
        native_grow: mask_role.is_some_and(|r| r.native_grow),
        region: match schema.params.get("region") {
            Some(ParamSpec::Region(r)) if r.input == RegionInput::Box => Some(RegionTarget {
                min_window_px: r.min_window_px.unwrap_or(1),
                max_aspect: r.max_aspect.unwrap_or(f64::INFINITY),
                takes_mask: mask_role.is_some(),
            }),
            _ => None,
        },
    };
    let default_role = refs.and_then(|r| r.default_role.as_deref()).unwrap_or("init");
    let Some(init_ref) = materialized.refs.iter().find(|r| r.role == "init" || r.role == default_role) else {
        return Err(refuse(EditError::ImageUnreadable("a mask or region edits an init image; send one with role init".into())));
    };
    let init = materialized_bytes(&init_ref.form).await.map_err(|m| refuse(EditError::ImageUnreadable(format!("the init image could not be read: {m}"))))?;
    let mask = match materialized.refs.iter().find(|r| r.role == "mask") {
        Some(m) => Some(materialized_bytes(&m.form).await.map_err(|e| refuse(EditError::ImageUnreadable(format!("the mask could not be read: {e}"))))?),
        None => None,
    };
    let (region, grow_px) = (req.region, req.mask_grow_px.unwrap_or(0));
    let preserve_init = (req.preserve_unmasked == Some(true)).then(|| init.clone());
    let prepared = tokio::task::spawn_blocking(move || {
        prepare(&EditRequest { init: &init, mask: mask.as_deref(), region, grow_px }, &target)
    })
    .await
    .map_err(|e| refuse(EditError::ImageUnreadable(format!("the edit could not be prepared: {e}"))))?
    .map_err(refuse)?;
    if let Some(png) = prepared.mask_png {
        use base64::Engine;
        let form = match refs.map(|r| &r.provider_format) {
            Some(RefProviderFormat::Multipart(m)) => MaterializedRefForm::MultipartField {
                field_name: m.field_map.get("mask").cloned().unwrap_or_else(|| "mask".into()),
                bytes: bytes::Bytes::from(png),
                content_type: "image/png".into(),
            },
            // Inline where the vendor takes a `data:` URI, else published at
            // litegen's own URL, as an uploaded reference is (L4).
            Some(RefProviderFormat::Url) if refs.is_none_or(|r| r.data_uri) => {
                MaterializedRefForm::Url(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png)))
            }
            Some(RefProviderFormat::Url) => {
                let unpublishable = |m: String| validation_rejection_response(&m, 400, &schema.id);
                let publisher = publisher.ok_or_else(|| {
                    unpublishable("this model fetches its mask by URL and litegen cannot publish one here; send a public mask URL".into())
                })?;
                MaterializedRefForm::Url(publisher.publish(&png, "image/png").await.map_err(unpublishable)?)
            }
            _ => MaterializedRefForm::Base64(base64::engine::general_purpose::STANDARD.encode(&png)),
        };
        match materialized.refs.iter_mut().find(|r| r.role == "mask") {
            Some(r) => r.form = form,
            None => materialized.refs.push(MaterializedRef { role: "mask".into(), form, time_s: None }),
        }
    }
    // 3D doc §4: preservation pastes the init back, so the output must be the
    // init's size — a `size` asking for another is refused before billing.
    let preserve = match (preserve_init, prepared.area) {
        (Some(init), Some((width, height, area))) => {
            if let Some((w, h)) = req.size.as_deref().and_then(|s| s.split_once('x')).and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))) {
                if (w, h) != (width, height) {
                    let msg = format!("preserve_unmasked keeps the init's {width}x{height}; size {w}x{h} would not");
                    return Err((StatusCode::BAD_REQUEST, Json(crate::api::middleware::validator::err_body("param_conflict", &msg, Some("size"), &schema.id))).into_response());
                }
            }
            Some(crate::proxy::preserve::Preserve {
                init,
                width,
                height,
                area,
                feather_px: req.feather_px.unwrap_or(0),
                webp: req.output_format.as_deref() == Some("webp"),
            })
        }
        _ => None,
    };
    Ok(Some(crate::providers::ImageEdit { window: prepared.window, vendor_grow_px: prepared.vendor_grow_px, preserve }))
}

/// What a request will be charged: its price with the markup in effect (3D
/// doc §3G) — what its quote says, its reservation and limits hold, and the
/// router's charge takes.
fn charged_price(state: &AppState, price: f64) -> f64 {
    crate::providers::apply_markup(price, state.router.cost_markup_percent()).1
}

/// The error answer for a router call that failed (a generation, a quote, a
/// status poll), marked as having reached a vendor when it did — which is
/// what an `Idempotency-Key` records and replays. A vendor timeout names the vendor's job, when it had
/// accepted one, and is never re-sent.
fn generation_error_response(e: &crate::proxy::router::ProxyError) -> axum::response::Response {
    let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    use crate::proxy::router::ProxyError;
    // A vendor's own answer reaches the caller (PixelGenie L15): its status,
    // code and message, and whether the same request may work later.
    let vendor_body = |kind: &str, vendor: &crate::proxy::router::VendorError, vendor_job_id: &Option<String>| {
        serde_json::json!({
            "error": {
                "message": e.to_string(),
                "type": kind,
                "code": kind,
                "retryable": e.retryable(),
                "provider_error": vendor,
                "vendor_job_id": vendor_job_id,
            }
        })
    };
    let body = match e {
        ProxyError::VendorTimeout { vendor_job_id, .. } => serde_json::json!({
            "error": {
                "message": e.to_string(),
                "type": "vendor_timeout",
                "code": "vendor_timeout",
                "retryable": e.retryable(),
                "vendor_job_id": vendor_job_id,
            }
        }),
        ProxyError::VendorRejected { vendor, vendor_job_id, .. } => vendor_body("vendor_rejected", vendor, vendor_job_id),
        ProxyError::VendorFailed { vendor, vendor_job_id, .. } => vendor_body("vendor_error", vendor, vendor_job_id),
        // One condition, one answer, wherever it is noticed (E8), with the
        // adapter's advice when it gave one (E9).
        ProxyError::ProviderNotConfigured { provider, detail } => return provider_not_configured_with(provider, detail.as_deref()),
        // The adapter refused the request before calling its vendor (E9): a
        // validation error the caller can fix, and no vendor attempt.
        ProxyError::Refused { model, message, .. } => {
            let mut body = crate::api::middleware::validator::err_body("request_unsupported", message, None, model);
            body["error"]["retryable"] = false.into();
            body
        }
        // A panic's text stays in the log (SEC-DBERR).
        ProxyError::Internal(_) => return internal_error_response("the router failed", e),
        _ => error_response(&e.to_string(), status.as_u16()),
    };
    let mut resp = (status, Json(body)).into_response();
    if e.reached_vendor() {
        resp.extensions_mut().insert(crate::api::middleware::idempotency::VendorAttempted);
    }
    resp
}

/// 400 body for a model whose provider has neither a per-app BYO credential nor a
/// registered platform default. Distinct from `error_response(400, ..)` because the
/// `type` is `provider_not_configured` (not `invalid_request_error`), matching the
/// documented BYO contract the dashboard/SDK keys off of.
fn provider_not_configured_response(provider: &str) -> axum::response::Response {
    provider_not_configured_with(provider, None)
}

/// [`provider_not_configured_response`], with the adapter's own advice
/// (`detail`) when the provider's adapter judged the credential it has
/// unusable, e.g. Kling's "store a Kling API key instead" (E9).
fn provider_not_configured_with(provider: &str, detail: Option<&str>) -> axum::response::Response {
    let message = match detail {
        Some(detail) => format!("Provider '{provider}' is not configured for this application: {detail}"),
        None => format!(
            "Provider '{provider}' is not configured for this application. \
             Add a provider credential in the dashboard."
        ),
    };
    let body = serde_json::json!({
        "error": {
            "type": "provider_not_configured",
            "code": "provider_not_configured",
            "message": message,
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

/// The calling org's own credentials for every provider a request may reach:
/// the model's provider's (`app_creds`, which the caller resolved with
/// [`resolve_org_provider_credential`]) and, when a model route serves the
/// model, each deployment's provider's, with the same lookup. The router
/// hands each deployment only its own provider's, so an org's key never
/// reaches another vendor (BYO-ROUTE, code review 2026-11-11).
async fn org_credentials_for(
    state: &AppState,
    key_ctx: &Option<KeyContext>,
    schema: &crate::capabilities::ModelSchema,
    app_creds: Option<crate::providers::ProviderCredentials>,
) -> Result<crate::proxy::router::OrgCredentials, axum::response::Response> {
    let mut creds = crate::proxy::router::OrgCredentials::of(&schema.provider, app_creds);
    for provider in state.router.route_providers(&schema.id) {
        if provider == schema.provider {
            continue;
        }
        if let Some(c) = resolve_org_provider_credential(state, key_ctx, &provider).await? {
            creds.insert(&provider, c);
        }
    }
    Ok(creds)
}

/// Resolve the calling app's BYO image store, if configured & usable. Returns
/// `None` (→ caller uses the global store) on any miss or, for a configured-but-
/// broken row, after logging a warning (fail-open — never breaks a generation).
async fn resolve_app_image_store(
    state: &AppState,
    key_ctx: &Option<KeyContext>,
) -> Option<std::sync::Arc<dyn crate::proxy::storage::ImageStore>> {
    let app_id = key_ctx.as_ref().and_then(|c| c.app_id.as_deref())?;
    let inline = || Some(std::sync::Arc::new(crate::proxy::storage::LocalStore) as std::sync::Arc<dyn crate::proxy::storage::ImageStore>);
    let tenant = tenant_bucket_of(state);
    match app_bucket(&state.db, state.secrets_key, &state.router.config.aws, tenant, app_id).await {
        AppBucket::None => None,
        AppBucket::Ready { config, fallback, .. } => match bucket_image_store(&config, tenant) {
            Ok(store) => Some(std::sync::Arc::new(store) as std::sync::Arc<dyn crate::proxy::storage::ImageStore>),
            Err(e) => {
                tracing::warn!(app_id, error = %e, "byo storage: build failed");
                if fallback { None } else { inline() }
            }
        },
        // litegen's own S3 image store may stand in...
        AppBucket::Unavailable { reason, fallback: true } => {
            tracing::warn!(app_id, reason = %reason, "byo storage unavailable, using the global store");
            None
        }
        // ...or not: the image goes back inline and nothing is uploaded.
        AppBucket::Unavailable { reason, fallback: false } => {
            tracing::warn!(app_id, reason = %reason, "byo storage unavailable, and the app keeps nothing in litegen's store");
            inline()
        }
    }
}

/// What an app's own bucket is right now.
// Built once per request and matched at once, so the large `Ready` variant
// costs a move, not memory; boxing it would add an allocation.
#[allow(clippy::large_enum_variant)]
pub(crate) enum AppBucket {
    /// It has none: litegen's store holds its files.
    None,
    /// Built from its storage settings (an assumed role's session included).
    Ready { config: crate::config::ImageStorageConfig, prefix: Option<String>, fallback: bool },
    /// Configured but unusable now: a decrypt, build or role failure.
    /// `fallback`: may litegen's own store stand in (`fallback_to_litegen_store`)?
    Unavailable { reason: String, fallback: bool },
}

/// Whose an app's own bucket is, and so how litegen reaches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TenantBucket {
    /// Single-tenant: the operator's own settings, used as written; an
    /// internal store is fine, as with webhooks.
    Operator,
    /// Hosted: a tenant names it (`PUT /v1/apps/{app_id}/storage`) and litegen
    /// sends it signed requests from inside the deployment (SSRF). Its host
    /// must be public (checked at every use), and its requests go out
    /// presigned through the guarded client (`S3Storage::guarded`), so a name
    /// that rebinds after the check still reaches nothing inside.
    Hosted,
    /// Tests only (`storage_access.unchecked_app_storage_hosts`): `Hosted`
    /// without the address check, so wiremock on 127.0.0.1 can stand in. The
    /// guarded client still refuses a *name* that resolves inside.
    HostedUncheckedHosts,
}

impl TenantBucket {
    /// Whether the bucket's host is checked before each use.
    pub(crate) fn checks_hosts(self) -> bool {
        self == Self::Hosted
    }

    /// Whether its requests go through the guarded client.
    pub(crate) fn guarded(self) -> bool {
        self != Self::Operator
    }
}

/// The deployment's [`TenantBucket`]: by its mode, with the tests' switch
/// from `config` when there is one.
pub(crate) fn tenant_bucket(mode: crate::config::Mode, config: Option<&crate::config::AppConfig>) -> TenantBucket {
    match mode {
        crate::config::Mode::SingleTenant => TenantBucket::Operator,
        crate::config::Mode::Hosted if config.is_some_and(|c| c.storage_access.unchecked_app_storage_hosts) => {
            TenantBucket::HostedUncheckedHosts
        }
        crate::config::Mode::Hosted => TenantBucket::Hosted,
    }
}

/// [`tenant_bucket`] for a request's state.
fn tenant_bucket_of(state: &AppState) -> TenantBucket {
    tenant_bucket(state.mode, Some(state.router.config.as_ref()))
}

/// The app's bucket as a file store: guarded when it's a tenant's.
fn bucket_storage(config: &crate::config::ImageStorageConfig, tenant: TenantBucket) -> Result<crate::proxy::storage::S3Storage, crate::proxy::storage::ImageStoreError> {
    crate::proxy::storage::S3Storage::from_config(config).map(|s| if tenant.guarded() { s.guarded() } else { s })
}

/// The app's bucket as an image store: guarded when it's a tenant's.
fn bucket_image_store(config: &crate::config::ImageStorageConfig, tenant: TenantBucket) -> Result<crate::proxy::storage::S3Store, crate::proxy::storage::ImageStoreError> {
    crate::proxy::storage::S3Store::from_config(config).map(|s| if tenant.guarded() { s.guarded() } else { s })
}

/// The app's own bucket, as its storage row says: its key pair decrypted, or
/// its role assumed (`credential_mode: assume_role`, docs/storage-access.md).
/// For a tenant's bucket ([`TenantBucket::checks_hosts`]), one whose requests
/// would go inside the deployment is unavailable, whenever its row was saved.
pub(crate) async fn app_bucket(
    db: &std::sync::Arc<dyn crate::db::DatabaseStore>,
    secrets_key: Option<[u8; 32]>,
    aws: &crate::config::AwsConfig,
    tenant: TenantBucket,
    app_id: &str,
) -> AppBucket {
    let row = match db.get_app_storage(app_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return AppBucket::None,
        // The row says whether litegen's store may stand in, and it can't be
        // read: nothing goes anywhere until it can. The reason can end up on
        // a reaped generation, so the database's text stays in the log.
        Err(e) => {
            tracing::error!(app_id, error = %e, "the app's storage settings could not be read");
            return AppBucket::Unavailable { reason: "the app's storage settings could not be read".into(), fallback: false };
        }
    };
    let fallback = row.fallback_to_litegen_store.unwrap_or(true);
    let unavailable = |reason: String| AppBucket::Unavailable { reason, fallback };
    // Checked at every use, before a secret is decrypted or a role assumed,
    // so a row saved before the save-time check existed, or a host whose
    // answer has since moved inside the deployment, gets no request.
    if tenant.checks_hosts() {
        match crate::proxy::storage::check_tenant_bucket(&row.bucket_name, &row.region, row.endpoint_url.as_deref()).await {
            // A name that didn't resolve just now isn't a verdict: the guarded
            // client judges every address it connects to anyway, and the
            // request fails on its own if the name stays bad.
            Ok(()) | Err(crate::proxy::storage::TenantBucketError::Lookup(_)) => {}
            Err(why) => return unavailable(format!("the app's bucket may not be used: {why}")),
        }
    }
    let Some(secrets_key) = secrets_key else {
        return unavailable("litegen has no secrets key to read the app's storage settings with".into());
    };
    let (access_key_id, secret_access_key, session_token, credentials_expire_at) = if row.credential_mode.as_deref() == Some("assume_role") {
        let Some(role) = row.role_arn.as_deref() else { return unavailable("the app's storage names no role".into()) };
        let external = crate::proxy::aws_sts::external_id(&secrets_key, app_id);
        match crate::proxy::aws_sts::assumed_credentials(aws, role, &external, &crate::proxy::aws_sts::session_name(app_id)).await {
            Ok(c) => (c.access_key_id, c.secret_access_key, Some(c.session_token), Some(c.expiration)),
            Err(e) => return unavailable(format!("could not assume {role}: {e}")),
        }
    } else {
        #[derive(serde::Deserialize)]
        struct StorageSecret {
            access_key_id: String,
            secret_access_key: String,
        }
        let plaintext = match crate::auth::secrets::decrypt(&secrets_key, &row.secret_ciphertext, &row.secret_nonce) {
            Ok(p) => p,
            Err(e) => return unavailable(format!("the app's storage secret could not be decrypted: {e}")),
        };
        match serde_json::from_slice::<StorageSecret>(&plaintext) {
            Ok(s) => (s.access_key_id, s.secret_access_key, None, None),
            Err(e) => return unavailable(format!("the app's storage secret is corrupt: {e}")),
        }
    };
    AppBucket::Ready {
        config: crate::config::ImageStorageConfig {
            backend: row.backend.clone(),
            path_prefix: row.path_prefix.clone(),
            s3: Some(crate::config::S3StorageConfig {
                bucket_name: row.bucket_name.clone(),
                region: row.region.clone(),
                access_key_id: Some(access_key_id),
                secret_access_key: Some(secret_access_key),
                endpoint_url: row.endpoint_url.clone(),
                custom_public_url: row.custom_public_url.clone(),
                session_token,
                credentials_expire_at,
            }),
            ..Default::default()
        },
        prefix: row.path_prefix.clone(),
        fallback,
    }
}

/// The app's own bucket for reading its files and presenting their links,
/// with its path prefix: `None` when it has none or it can't be reached now
/// (litegen's store is then read). Never fails a request.
pub(crate) async fn resolve_app_model3d_store(
    db: &std::sync::Arc<dyn crate::db::DatabaseStore>,
    secrets_key: Option<[u8; 32]>,
    aws: &crate::config::AwsConfig,
    tenant: TenantBucket,
    app_id: &str,
) -> Option<(std::sync::Arc<dyn crate::proxy::storage::ImageStorage>, Option<String>)> {
    match app_bucket(db, secrets_key, aws, tenant, app_id).await {
        AppBucket::None => None,
        AppBucket::Ready { config, prefix, .. } => match bucket_storage(&config, tenant) {
            Ok(store) => Some((std::sync::Arc::new(store), prefix)),
            Err(e) => {
                tracing::warn!(app_id, error = %e, "byo 3d storage: build failed, using global store");
                None
            }
        },
        AppBucket::Unavailable { reason, .. } => {
            tracing::warn!(app_id, reason = %reason, "byo 3d storage unavailable, reading the global store");
            None
        }
    }
}

/// The app's own bucket for reading or deleting its files: `Ok(None)` when it
/// has none, `Err` when it has one that can't be used now, whatever
/// `fallback_to_litegen_store` says. A delete that passed over the bucket
/// would mark files deleted that are still in it, and a read would send the
/// caller to the bucket's own URL.
pub(crate) async fn app_store_for_files(
    db: &std::sync::Arc<dyn crate::db::DatabaseStore>,
    secrets_key: Option<[u8; 32]>,
    aws: &crate::config::AwsConfig,
    tenant: TenantBucket,
    app_id: &str,
) -> Result<Option<std::sync::Arc<dyn crate::proxy::storage::ImageStorage>>, String> {
    match app_bucket(db, secrets_key, aws, tenant, app_id).await {
        AppBucket::None => Ok(None),
        AppBucket::Ready { config, .. } => bucket_storage(&config, tenant)
            .map(|store| Some(std::sync::Arc::new(store) as std::sync::Arc<dyn crate::proxy::storage::ImageStorage>))
            .map_err(|e| format!("the app's bucket could not be used: {e}")),
        AppBucket::Unavailable { reason, .. } => Err(reason),
    }
}

/// Where to keep a file of the app: its own bucket; `Ok(None)` for litegen's
/// store (it has none, or its bucket is down and it falls back); `Err` when
/// its bucket is down and it keeps nothing in litegen's store, so the caller
/// waits for the bucket (`fallback_to_litegen_store: false`).
pub(crate) async fn app_store_for_writing(
    db: &std::sync::Arc<dyn crate::db::DatabaseStore>,
    secrets_key: Option<[u8; 32]>,
    aws: &crate::config::AwsConfig,
    tenant: TenantBucket,
    app_id: &str,
) -> Result<Option<(std::sync::Arc<dyn crate::proxy::storage::ImageStorage>, Option<String>)>, String> {
    match app_bucket(db, secrets_key, aws, tenant, app_id).await {
        AppBucket::None => Ok(None),
        AppBucket::Ready { config, prefix, fallback } => match bucket_storage(&config, tenant) {
            Ok(store) => Ok(Some((std::sync::Arc::new(store), prefix))),
            Err(e) if fallback => {
                tracing::warn!(app_id, error = %e, "byo storage: build failed, keeping in the global store");
                Ok(None)
            }
            Err(e) => Err(format!("the app's bucket could not be used: {e}")),
        },
        AppBucket::Unavailable { reason, fallback: true } => {
            tracing::warn!(app_id, reason = %reason, "byo storage unavailable, keeping in the global store");
            Ok(None)
        }
        AppBucket::Unavailable { reason, fallback: false } => Err(reason),
    }
}

#[cfg(test)]
mod request_record_tests {
    use super::*;

    /// P15 (timing; run alone): decoding and hashing a request's inline
    /// references holds no async worker. Today a task queued behind it
    /// waited as long as the digest took.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn digesting_a_large_reference_leaves_the_runtime_free() {
        use base64::Engine;
        use sha2::Digest;
        let raw = vec![7u8; 15 * 1024 * 1024];
        let reference = crate::types::ReferenceImage {
            kind: crate::types::RefImageKind::Base64,
            value: base64::engine::general_purpose::STANDARD.encode(&raw),
            role: Some("reference".into()),
            time_s: None,
        };
        let base = crate::types::BaseGenerationRequest {
            prompt: "p".into(),
            model: "m".into(),
            n: 1,
            negative_prompt: None,
            seed: None,
            reference_images: vec![reference; 4],
            strict: true,
            extra: None,
            metadata: None,
        };
        let ctx = crate::proxy::materializer::MaterializeContext::default();
        let started = std::time::Instant::now();
        let inline = request_record(&base, None, &ctx);
        let alone = started.elapsed();
        assert!(alone >= std::time::Duration::from_millis(200), "too light to show a stall: {alone:?}");
        let (record, stall) = crate::util::blocking::longest_stall_while(async move { request_record(&base, None, &ctx) }).await;
        assert_eq!(record, inline, "the same record, wherever it was made");
        let first = &record["reference_images"][0];
        assert_eq!(first["sha256"], hex::encode(sha2::Sha256::digest(&raw)), "{first}");
        assert_eq!(first["bytes"], raw.len(), "{first}");
        assert!(stall < alone / 4, "the runtime stalled {stall:?} while digesting (the digest alone takes {alone:?})");
    }
}

#[cfg(test)]
mod error_response_tests {
    use super::*;
    use crate::proxy::router::ProxyError;

    async fn status_and_body(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// E8: one condition, one answer. A provider the router finds unusable
    /// (removed mid-request, or a BYO credential no adapter can be built
    /// from) answers as the handler's own check does: the documented 400
    /// `provider_not_configured`, not a 424 with another message.
    #[tokio::test]
    async fn a_provider_the_router_cannot_find_answers_as_the_handler_does() {
        let (status, routed) = status_and_body(generation_error_response(&ProxyError::not_configured("vidu"))).await;
        let (_, handled) = status_and_body(provider_not_configured_response("vidu")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{routed}");
        assert_eq!(routed, handled);
    }

    /// E9: an adapter's advice on a credential it can't use reaches the
    /// caller, under the provider's name.
    #[tokio::test]
    async fn an_adapters_credential_advice_reaches_the_caller() {
        let advice = "kling: this credential is an access/secret key pair, which works only on Kling's legacy API; store a Kling API key instead";
        let e = ProxyError::ProviderNotConfigured { provider: "kling".into(), detail: Some(advice.into()) };
        let (status, body) = status_and_body(generation_error_response(&e)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"]["code"], "provider_not_configured", "{body}");
        let message = body["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("'kling'") && message.contains("store a Kling API key instead"), "{message}");
    }

    /// SEC-DBERR: the router's internal errors carry a panic's text; it
    /// stays in the log.
    #[tokio::test]
    async fn an_internal_router_error_names_no_detail() {
        let e = ProxyError::Internal("mask normalisation stopped: task 7 panicked at src/proxy/x.rs:12".into());
        let (status, body) = status_and_body(generation_error_response(&e)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert_eq!(body["error"]["message"], "internal server error", "{body}");
    }
}

#[cfg(test)]
mod app_bucket_error_tests {
    use super::*;
    use crate::db::sqlite::SqliteDatabase;

    /// SEC-DBERR: an app's storage row that can't be read makes the bucket
    /// unavailable with a reason that names no database text. The poller
    /// stores that reason as the generation's `error_message` when it reaps
    /// an over-age job, and clients read it back.
    #[tokio::test]
    async fn an_unreadable_storage_row_gives_a_reason_without_the_database_text() {
        let raw = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        sqlx::query("ALTER TABLE app_storage_credentials RENAME TO app_storage_away").execute(raw.pool()).await.unwrap();
        let db: Arc<dyn crate::db::DatabaseStore> = raw;
        match app_bucket(&db, None, &crate::config::AwsConfig::default(), TenantBucket::Operator, "app-1").await {
            AppBucket::Unavailable { reason, fallback } => {
                assert!(!fallback, "an unreadable row lets nothing stand in");
                assert!(!reason.contains("no such table") && !reason.contains("database"), "{reason}");
            }
            _ => panic!("an unreadable storage row must make the bucket unavailable"),
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

    #[tokio::test]
    async fn cancel_generation_marks_the_request_log_cancelled() {
        // A cancelled job leaves the poller's active window, so the cancel is
        // the only terminal transition its `pending` log row will ever see.
        let state = build_state().await;
        state.db.log_request("lg-cancel-log-1", "mock/v", "mock", "pending", "video", 0.0, 5, None, None, None, None).await.unwrap();
        state.db.insert_generation("lg-cancel-log-1", None, "mock/v", "mock", "video", None, 0.0, None, None).await.unwrap();
        let db = state.db.clone();

        let app = build_test_router(state);
        let body = serde_json::to_vec(&serde_json::json!({"status": "cancelled"})).unwrap();
        let req = Request::builder()
            .method("PATCH").uri("/v1/generations/lg-cancel-log-1")
            .header("content-type", "application/json")
            .body(Body::from(body)).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let (logs, _) = db.get_request_logs(1, 50).await.unwrap();
        let log = logs.iter().find(|l| l.id == "lg-cancel-log-1").unwrap();
        assert_eq!(log.status, GenerationStatus::Cancelled);
    }

    /// L18: a cancel gives back what the job was billed, and sends the
    /// terminal webhook every other ending sends — the row as settled, with the
    /// caller's metadata (L17).
    #[tokio::test]
    async fn a_cancel_refunds_the_job_and_sends_its_webhook() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/wh"))
            .and(body_partial_json(serde_json::json!({
                "status": "cancelled",
                "cost_usd": 0.0,
                "metadata": { "request": { "metadata": { "asset": "knight" } } },
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let state = build_state().await;
        let key = state.db.create_api_key(
            "wh", "wh-hash-cancel", "lg-wh", Some(5.0), None, "generate,read", Some(&format!("{}/wh", server.uri())),
        ).await.unwrap();
        state.db.atomic_charge_tokens(&key.id, 0.5).await.unwrap();
        state.db.log_request("lg-cancel-refund-1", "mock/priced-video", "mock", "pending", "video", 0.5, 5, None, None, None, None).await.unwrap();
        state.db.insert_generation(
            "lg-cancel-refund-1", Some(&key.id), "mock/priced-video", "mock", "video", Some("job-1"), 0.5, None, None,
        ).await.unwrap();
        state.db.merge_generation_metadata_if_active(
            "lg-cancel-refund-1", &serde_json::json!({ "request": { "metadata": { "asset": "knight" } } }),
        ).await.unwrap();
        let db = state.db.clone();

        let app = build_test_router(state);
        let req = Request::builder()
            .method("PATCH").uri("/v1/generations/lg-cancel-refund-1")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"status":"cancelled"}"#)).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        // No mock video provider is registered here: nothing took the cancel.
        assert_eq!((body["status"].as_str(), body["cost_usd"].as_f64(), body["vendor_cancelled"].as_bool()), (Some("cancelled"), Some(0.0), Some(false)), "{body}");
        assert_eq!(db.get_api_key(&key.id).await.unwrap().unwrap().tokens_used, 0.0, "the quota is back");

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        server.verify().await;
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
        // Every account joins the default org at signup or invitation; the role
        // it holds there is the one its permissions come from.
        db.add_org_member(crate::api::middleware::DEFAULT_ORG_ID, &user.id, role).await.expect("join the default org");
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

    /// A key made by a key takes that key's owner. When the calling key's row
    /// is gone by the time it's read (deleted with its org or app mid-request),
    /// the new key would have no owner and outlive everyone: refuse it instead.
    #[tokio::test]
    async fn a_key_whose_making_key_is_gone_is_refused() {
        let db = build_db().await;
        let state = build_state_with_key(db.clone()).await;
        let gone = KeyContext {
            key_id: Some(uuid::Uuid::new_v4()),
            scopes: vec![crate::api::middleware::Scope::Generate, crate::api::middleware::Scope::Read],
            rpm_limit: None,
            quota_remaining: None,
            webhook_url: None,
            user: None,
            permissions: vec![],
            session_id: None,
            org_id: Some(crate::api::middleware::DEFAULT_ORG_ID.to_string()),
            app_id: Some(crate::api::middleware::DEFAULT_APP_ID.to_string()),
            oauth_grant_id: None,
        };
        let request = CreateApiKeyRequest {
            name: "child".into(),
            token_quota: None,
            rpm_limit: None,
            scopes: "generate,read".into(),
            webhook_url: None,
            expires_at: None,
        };
        let resp = create_api_key(State(state), OptionalKeyContext(Some(gone)), Json(request)).await.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(db.list_api_keys().await.unwrap().is_empty(), "no ownerless key was left behind");
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

    /// P1 (code review 2026-11-11): readiness answers from the database alone.
    /// An anonymous probe asked every networked vendor, one after another,
    /// with the operator's keys: it spent quota, took the sum of their
    /// latencies, and a short probe timeout failed a replica whose database
    /// was fine whenever a vendor was slow.
    #[tokio::test]
    async fn readiness_asks_no_vendor() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let vendor = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "object": "list", "data": [] })))
            .mount(&vendor)
            .await;
        let state = build_state_with_real_db().await;
        let cfg = crate::providers::ProviderInstanceConfig {
            api_key: "operator-key".into(),
            credentials: crate::providers::ProviderCredentials { api_key: Some("operator-key".into()), ..Default::default() },
            api_base: Some(format!("{}/v1", vendor.uri())),
            ..Default::default()
        };
        assert!(state.router.registry.configure_provider("openai", cfg).await);
        let req = Request::builder().method("GET").uri("/health/ready").body(Body::empty()).unwrap();
        let resp = build_health_router(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["checks"]["db"], true);
        let seen = vendor.received_requests().await.unwrap_or_default();
        let paths: Vec<String> = seen.iter().map(|r| r.url.path().to_string()).collect();
        assert!(seen.is_empty(), "an anonymous readiness probe asked a vendor {} time(s) with the operator's key: {paths:?}", seen.len());
        assert_eq!(body["checks"]["providers"], serde_json::json!(["openai"]), "each provider configured with the gateway's keys, once");
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

    #[test]
    fn the_listing_carries_each_models_task() {
        let reg = shipped_registry();
        let trellis = reg.get("fal/trellis").expect("fal/trellis present");
        let info = serde_json::to_value(project_model_info(trellis)).unwrap();
        assert_eq!(info["capabilities"]["task"], "generate", "{info}");
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

    pub(super) async fn build_state(db: Arc<SqliteDatabase>, mode: Mode, master_key: Option<String>) -> Arc<AppState> {
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
                stage_context: None,
                next_stage: None,
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

    #[tokio::test]
    async fn completed_without_mesh_marks_the_request_log_failed() {
        let state = build_state_with_no_mesh_provider().await;
        let db = state.db.clone();
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

        // Both rows exist the moment submit returns — `generate_3d` writes them
        // before responding. They used to be written by a spawned task, and a
        // client polling first found no row for the guarded terminal write to
        // update, so the generation and its log both sat `pending`.
        let (logs, _) = db.get_request_logs(1, 100).await.unwrap();
        assert!(logs.iter().any(|l| l.id == id), "the pending log row is written before submit returns");
        assert!(db.get_generation(&id).await.unwrap().is_some(), "and so is the generation row");

        let resp = app.oneshot(
            Request::get(format!("/v1/models3d/{id}")).body(Body::empty()).unwrap(),
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let (logs, _) = db.get_request_logs(1, 100).await.unwrap();
        let log = logs.iter().find(|l| l.id == id).unwrap();
        assert_eq!(log.status, GenerationStatus::Failed);
        assert_eq!(log.error.as_deref(), Some("provider reported success without a mesh asset"));
    }
}

// ─── `usage` derived from the generation row ────────────────────────────────
//
// The HTTP tests in `litegen-core/tests/model3d_api.rs` can only assert that
// `usage` is PRESENT on a poll response: the mock 3D models are priced $0, so
// every cost they produce is 0.0. The value derivation is pinned here instead,
// against a row with a real cost.
#[cfg(test)]
mod model3d_usage_derivation_tests {
    use super::*;
    use crate::types::{Generation, GenerationStatus};

    fn row(cost_usd: f64, status: GenerationStatus) -> Generation {
        let now = chrono::Utc::now();
        Generation {
            id: "litegen-3d-derivation".into(),
            key_id: None,
            model: "tripo/v2".into(),
            provider: "tripo".into(),
            media_type: "model3d".into(),
            status,
            progress: 100,
            provider_job_id: Some("provider-job-1".into()),
            result_url: None,
            error_message: None,
            cost_usd,
            created_at: now,
            completed_at: Some(now),
            metadata: None,
            org_id: None,
            app_id: None,
            files_deleted_at: None,
            files_deleted_reason: None,
        }
    }

    #[test]
    fn usage_is_the_row_cost_converted_at_a_thousand_tokens_per_dollar() {
        let usage = usage_from_row_cost(0.042);
        assert_eq!(usage.cost_usd, 0.042);
        assert_eq!(usage.tokens, 42, "1 token = $0.001, so $0.042 = 42 tokens");
        assert_eq!(usage.cost_source, CostSource::Estimated);
    }

    #[test]
    fn a_partial_token_rounds_up_so_a_charged_generation_is_never_free() {
        // `usd_to_tokens` ceils: $0.0005 is half a token, and billing zero for
        // a dispatched generation would be the wrong direction to round.
        assert_eq!(usage_from_row_cost(0.0005).tokens, 1);
        assert_eq!(usage_from_row_cost(0.0011).tokens, 2);
    }

    #[test]
    fn a_zero_cost_row_still_yields_a_present_usage_triple() {
        // The mock models (and any promo-priced model) cost $0. `usage` must
        // still be an object — the consumer distinguishes "free" from "unknown".
        let resp = model3d_response_from_row(&row(0.0, GenerationStatus::Completed));
        let usage = resp.usage.expect("usage is present even at $0");
        assert_eq!(usage.cost_usd, 0.0);
        assert_eq!(usage.tokens, 0);
        assert_eq!(usage.cost_source, CostSource::Estimated);
    }

    #[test]
    fn model3d_response_from_row_carries_the_rows_cost() {
        let resp = model3d_response_from_row(&row(0.25, GenerationStatus::Completed));
        let usage = resp.usage.expect("a row-answered poll carries usage");
        assert_eq!(usage.cost_usd, 0.25);
        assert_eq!(usage.tokens, 250);
    }

    #[test]
    fn a_cancelled_row_carries_usage_too() {
        // A cancelled generation was still dispatched and still charged, so the
        // consumer needs its cost to reconcile against.
        let resp = model3d_response_from_row(&row(0.25, GenerationStatus::Cancelled));
        assert_eq!(resp.status, GenerationStatus::Cancelled);
        assert_eq!(resp.usage.expect("usage on a cancelled row").cost_usd, 0.25);
    }

    #[test]
    fn only_pending_and_processing_are_non_terminal() {
        // `get_3d_status`'s read-through hangs off this: a status that wrongly
        // reported non-terminal would let a poll overwrite a decided outcome.
        for s in [GenerationStatus::Completed, GenerationStatus::Failed, GenerationStatus::Cancelled] {
            assert!(s.is_terminal(), "{s} must be terminal");
        }
        for s in [GenerationStatus::Pending, GenerationStatus::Processing] {
            assert!(!s.is_terminal(), "{s} must not be terminal");
        }
    }
}

#[cfg(test)]
mod served_schema_tests {
    use super::served_model_schema;
    use crate::capabilities::{CapabilityRegistry, ParamSpec};

    fn one(yaml_params: &str) -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[(
            "x.yaml",
            format!(
                "models:\n  - id: x/m\n    provider: x\n    media_type: model3d\n    display_name: M\n    \
                 pricing: {{ base_cost_usd: 0.01 }}\n    capabilities: {{ text_to_3d: true }}\n    \
                 prompt: {{ required: true }}\n    params:\n{yaml_params}"
            )
            .as_str(),
        )])
        .unwrap()
    }

    #[test]
    fn a_legacy_scalar_format_spec_is_served_as_the_canonical_plural() {
        // A deployment's own models dir may still say `output_format`; a client
        // must only ever have one spelling to render.
        let r = one("      output_format:\n        kind: string\n        enum_values: [glb, obj]\n        default: glb\n");
        let served = served_model_schema(r.get("x/m").unwrap());
        assert!(!served.params.contains_key("output_format"));
        match served.params.get("output_formats") {
            Some(ParamSpec::StringArray(s)) => {
                let native = ["glb".to_string(), "obj".to_string()];
                assert_eq!(s.enum_values, crate::proxy::model3d_convert::deliverable_formats(&native));
                assert_eq!(s.derivable_values, crate::proxy::model3d_convert::derivable_formats());
                assert_eq!(s.max_items, Some(1), "a scalar vendor field is one job");
                assert_eq!(s.default, ["glb"]);
            }
            other => panic!("expected a string_array output_formats, got {other:?}"),
        }
    }

    #[test]
    fn a_model_without_formats_is_served_verbatim() {
        let r = one("      texture: { kind: bool }\n");
        let m = r.get("x/m").unwrap();
        let served = served_model_schema(m);
        assert!(!served.params.contains_key("output_formats"));
        assert_eq!(served.params.len(), m.params.len());
    }
}

#[cfg(test)]
mod model3d_vendor_cancel_tests {
    //! A cancelled 3D generation is stopped at the vendor, not only relabelled
    //! here: a queued fal job nobody cancels still runs, and bills.
    use super::*;
    use crate::providers::{
        CancelOutcome, HealthCheckResult, Model3dExtras, Model3dGenerationHandle,
        Model3dGenerationPollResult, Model3dProvider, ProviderError, ProviderInstanceConfig,
    };
    use std::sync::Mutex;

    /// Records every handle it is asked to cancel.
    #[derive(Default)]
    struct RecordingProvider {
        cancelled: Mutex<Vec<Model3dGenerationHandle>>,
    }

    #[async_trait::async_trait]
    impl Model3dProvider for RecordingProvider {
        fn name(&self) -> &str { "mock" }
        fn configure(&mut self, _config: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }
        async fn generate(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _b: &BaseGenerationRequest,
            _e: &Model3dExtras,
            _r: &crate::proxy::materializer::MaterializedRequest,
        ) -> Result<Model3dGenerationHandle, ProviderError> {
            unreachable!("not submitted in these tests")
        }
        async fn poll_status(&self, _h: &Model3dGenerationHandle) -> Result<Model3dGenerationPollResult, ProviderError> {
            unreachable!("a cancelled job is never polled")
        }
        async fn estimate_cost(
            &self,
            m: &crate::capabilities::ModelSchema,
            _r: &Model3dGenerationRequest,
        ) -> Result<CostEstimate, ProviderError> {
            Ok(crate::providers::build_cost_estimate(m.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
        }
        async fn health_check(&self) -> HealthCheckResult {
            HealthCheckResult { healthy: true, message: "ok".into(), latency_ms: None }
        }
        async fn cancel(&self, handle: &Model3dGenerationHandle) -> Result<CancelOutcome, ProviderError> {
            self.cancelled.lock().unwrap().push(handle.clone());
            Ok(CancelOutcome::Requested)
        }
    }

    async fn state_with(provider: Arc<RecordingProvider>) -> Arc<AppState> {
        let db = Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        registry.register_mock_model3d(provider).await;
        let config = Arc::new(crate::config::AppConfig::default());
        let router = Arc::new(crate::proxy::router::ProxyRouter::new(
            registry,
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            config,
            Arc::new(crate::proxy::storage::LocalStore),
        ));
        struct NoTemp;
        #[async_trait::async_trait]
        impl crate::proxy::materializer::TempStorage for NoTemp {
            async fn put(&self, k: &str, _b: bytes::Bytes, _c: &str) -> Result<String, crate::proxy::materializer::MaterializeError> {
                Ok(format!("local://{k}"))
            }
            async fn delete(&self, _k: &str) -> Result<(), crate::proxy::materializer::MaterializeError> { Ok(()) }
        }
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        Arc::new(AppState {
            router,
            db,
            master_key: None,
            registry: Arc::new(crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap()),
            materializer: Arc::new(crate::proxy::materializer::Materializer::new(Arc::new(NoTemp), reqwest::Client::new())),
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(8)),
            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    fn row(stage_context: Option<serde_json::Value>) -> crate::types::Generation {
        let now = chrono::Utc::now();
        serde_json::from_value(serde_json::json!({
            "id": "litegen-3d-cancel-1", "key_id": null, "model": "mock/mesh-3d", "provider": "mock",
            "media_type": "model3d", "status": "cancelled", "provider_job_id": "vendor-job-9",
            "result_url": null, "error_message": null, "progress": 0, "cost_usd": 0.0,
            "metadata": stage_context.map(|c| serde_json::json!({ "stage_context": c })),
            "created_at": now, "updated_at": now, "completed_at": now,
            "org_id": null, "app_id": null,
        }))
        .expect("a generation row")
    }

    #[tokio::test]
    async fn a_tracked_job_is_cancelled_through_the_instance_that_submitted_it() {
        let submitted_by = Arc::new(RecordingProvider::default());
        let registered = Arc::new(RecordingProvider::default());
        let state = state_with(registered.clone()).await;
        let handle = Model3dGenerationHandle {
            provider_job_id: "vendor-job-9".into(),
            provider: "mock".into(),
            model: "mock/mesh-3d".into(),
            stage_context: None,
        };
        cancel_model3d_at_vendor(state, row(None), Some((handle, submitted_by.clone()))).await;
        assert_eq!(submitted_by.cancelled.lock().unwrap().len(), 1, "the submitting instance cancels");
        assert!(registered.cancelled.lock().unwrap().is_empty(), "not a platform-default instance");
    }

    #[tokio::test]
    async fn an_untracked_job_is_rebuilt_from_its_row_and_cancelled() {
        // Poller-owned, or submitted before a restart: the router holds nothing,
        // so the handle comes off the row — stage_context included, which is
        // where fal's cancel URL lives.
        let registered = Arc::new(RecordingProvider::default());
        let state = state_with(registered.clone()).await;
        let ctx = serde_json::json!({ "endpoint": "fal-ai/trellis", "cancel_url": "https://queue.fal.run/x/cancel" });
        cancel_model3d_at_vendor(state, row(Some(ctx.clone())), None).await;
        let cancelled = registered.cancelled.lock().unwrap();
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].provider_job_id, "vendor-job-9");
        assert_eq!(cancelled[0].model, "mock/mesh-3d");
        assert_eq!(cancelled[0].stage_context.as_ref(), Some(&ctx));
    }
}

#[cfg(test)]
mod model3d_webhook_observer_tests {
    //! A key's webhook fires when the CLIENT's poll terminalises a 3D job — and
    //! only once. The poller used to be the only dispatch site, but a row the
    //! client's GET terminalised first never re-entered the poller's window, so
    //! a caller watching webhooks instead of polling never heard about it.
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::{middleware, routing::{get, post}};
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn state() -> Arc<AppState> {
        let db = Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let mut config = crate::config::AppConfig::default();
        config.providers.insert("mock".into(), serde_json::from_value(serde_json::json!({ "enabled": true })).unwrap());
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        registry.init_from_config(&config).await;
        let router = Arc::new(crate::proxy::router::ProxyRouter::new(
            registry,
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(config),
            Arc::new(crate::proxy::storage::LocalStore),
        ));
        struct NoTemp;
        #[async_trait::async_trait]
        impl crate::proxy::materializer::TempStorage for NoTemp {
            async fn put(&self, k: &str, _b: bytes::Bytes, _c: &str) -> Result<String, crate::proxy::materializer::MaterializeError> {
                Ok(format!("local://{k}"))
            }
            async fn delete(&self, _k: &str) -> Result<(), crate::proxy::materializer::MaterializeError> { Ok(()) }
        }
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        Arc::new(AppState {
            router,
            db,
            // Without a master key, single-tenant mode treats every bearer
            // token as anonymous (a dev bypass) and no job has a key to look a
            // webhook up on. A configured master key makes keys real.
            master_key: Some(MASTER.into()),
            registry: Arc::new(crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap()),
            materializer: Arc::new(crate::proxy::materializer::Materializer::new(Arc::new(NoTemp), reqwest::Client::new())),
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(8)),
            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    const MASTER: &str = "test-master-key";

    fn app(state: Arc<AppState>) -> axum::Router {
        let auth_state = state.clone();
        axum::Router::new()
            .route("/v1/keys", post(create_api_key))
            .route("/v1/models3d/generations", post(generate_3d))
            .route("/v1/models3d/{id}", get(get_3d_status))
            .layer(middleware::from_fn(move |req: axum::extract::Request, next: middleware::Next| {
                let s = auth_state.clone();
                async move {
                    let headers = req.headers().clone();
                    crate::api::middleware::auth_middleware(headers, s, req, next).await
                }
            }))
            .with_state(state)
    }

    async fn call(app: &axum::Router, req: Request<Body>) -> serde_json::Value {
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        assert!(status.is_success(), "{status}: {body}");
        body
    }

    #[tokio::test]
    async fn the_webhook_fires_once_when_the_clients_poll_terminalises_the_job() {
        let receiver = MockServer::start().await;
        Mock::given(method("POST")).and(path("/wh")).respond_with(ResponseTemplate::new(200)).mount(&receiver).await;

        let state = state().await;
        let app = app(state.clone());

        let key = call(&app, Request::post("/v1/keys")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {MASTER}"))
            .body(Body::from(serde_json::json!({
                "name": "wh", "scopes": "generate,read", "webhook_url": format!("{}/wh", receiver.uri()),
            }).to_string()))
            .unwrap()).await;
        let bearer = format!("Bearer {}", key["key"].as_str().expect("secret"));

        let submitted = call(&app, Request::post("/v1/models3d/generations")
            .header("content-type", "application/json")
            .header("authorization", &bearer)
            .body(Body::from(r#"{"model":"mock/mesh-3d","prompt":"a fox"}"#))
            .unwrap()).await;
        let id = submitted["id"].as_str().unwrap().to_string();

        let mut last = serde_json::Value::Null;
        for _ in 0..10 {
            last = call(&app, Request::get(format!("/v1/models3d/{id}")).header("authorization", &bearer).body(Body::empty()).unwrap()).await;
            if last["status"] == "completed" { break; }
        }
        assert_eq!(last["status"], "completed", "{last}");
        // The poller's next tick must not deliver it a second time.
        crate::proxy::poller::poll_once(
            &state.db, &state.router.registry, None, crate::config::Mode::SingleTenant, &state.router.model3d_store,
        ).await;

        // Deliveries are spawned off the response path; give the first one
        // time to land, then a little more for a duplicate to show up.
        for _ in 0..50 {
            if !receiver.received_requests().await.unwrap().is_empty() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let deliveries = receiver.received_requests().await.unwrap();
        assert_eq!(deliveries.len(), 1, "exactly one webhook per terminal generation");
        let payload: serde_json::Value = serde_json::from_slice(&deliveries[0].body).unwrap();
        assert_eq!(payload["id"], id);
        assert_eq!(payload["status"], "completed");
        assert!(
            payload["metadata"]["assets"].as_array().is_some_and(|a| a.iter().any(|x| x["kind"] == "mesh")),
            "the webhook carries the assets the row has: {payload}"
        );
    }
}

#[cfg(test)]
mod edit_mask_tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef, MaterializedRefForm, MaterializedRequest};

    fn schema(data_uri: bool) -> crate::capabilities::ModelSchema {
        let yaml = format!(
            r#"
models:
  - id: t/url-mask
    provider: t
    media_type: image
    display_name: T
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ image_to_image: true }}
    prompt: {{ required: true }}
    ref_inputs:
      max_total: 2
      default_role: init
      provider_format: {{ form: url }}
      data_uri: {data_uri}
      roles:
        init: {{ required: true, min_count: 1, max_count: 1 }}
        mask: {{ required: false, min_count: 0, max_count: 1, convention: white_edits }}
"#
        );
        crate::capabilities::CapabilityRegistry::from_yaml_strs(&[("t.yaml", &yaml)]).unwrap().get("t/url-mask").unwrap().clone()
    }

    fn png_uri(rgba: [u8; 4]) -> String {
        use base64::Engine;
        let img = image::RgbaImage::from_pixel(64, 64, image::Rgba(rgba));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(out.into_inner()))
    }

    fn refs() -> MaterializedRequest {
        let r = |role: &str, uri: String| MaterializedRef { role: role.into(), form: MaterializedRefForm::Url(uri), time_s: None };
        MaterializedRequest { refs: vec![r("init", png_uri([10, 20, 30, 255])), r("mask", png_uri([255, 255, 255, 255]))], cleanup: Cleanup::empty() }
    }

    fn mask_url(m: &MaterializedRequest) -> String {
        match &m.refs.iter().find(|r| r.role == "mask").unwrap().form {
            MaterializedRefForm::Url(u) => u.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// A vendor that takes no `data:` URI gets the prepared mask at
    /// litegen's own URL, as an uploaded reference is (L4).
    #[tokio::test]
    async fn a_prepared_mask_for_a_vendor_without_data_uris_is_published() {
        let base = "https://litegen.example.com";
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn crate::proxy::storage::ImageStorage> =
            Arc::new(crate::proxy::storage::LocalModel3dStorage::new(dir.path().to_path_buf(), base.into()));
        let publisher = Arc::new(crate::proxy::ref_publish::RefPublisher::new(store, base.into()));
        let req: ImageGenerationRequest = serde_json::from_value(serde_json::json!({ "model": "t/url-mask", "prompt": "p" })).unwrap();

        let mut published = refs();
        apply_image_edit(&schema(false), &req, &mut published, Some(&publisher)).await.unwrap();
        assert!(mask_url(&published).starts_with(base), "{}", mask_url(&published));

        let mut inline = refs();
        apply_image_edit(&schema(true), &req, &mut inline, Some(&publisher)).await.unwrap();
        assert!(mask_url(&inline).starts_with("data:image/png;base64,"));
    }
}
