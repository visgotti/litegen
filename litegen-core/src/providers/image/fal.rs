use crate::providers::auth::Authorize;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRequest, MaterializedRefForm};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, SegmentationInput, VendorMask, build_cost_estimate,
};
use crate::providers::model3d::download::{fetch_asset, AssetError};
use crate::providers::model3d::fal::{needs_retry, platform_key_check, transient, QueueReply, DEFAULT_QUEUE_BASE};
use crate::types::*;

/// fal's synchronous host, where images run.
const DEFAULT_SYNC_BASE: &str = "https://fal.run";

/// The most a result image may weigh.
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

/// What litegen itself sends in a generation body; `extra` cannot replace it.
/// `num_images` and `sync_mode` are here although litegen sends neither: fal
/// would bill every image it made while litegen keeps one, and a `data:` URI
/// result is not one the adapter fetches.
const OWNED_FIELDS: &[&str] = &[
    "prompt", "num_images", "image_size", "seed", "negative_prompt", "guidance_scale",
    "num_inference_steps", "strength", "style", "image_url", "sync_mode", "output_format", "format",
    "image_urls", "aspect_ratio", "resolution", "expand_left", "expand_top", "expand_right", "expand_bottom",
    "mask_url", "colors",
];

/// Fal.ai image generation provider.
///
/// Routes to model-specific Fal endpoints for Flux, SDXL, SD3.5, Recraft, etc.
/// Sync API: `POST https://fal.run/{endpoint}` → `{images: [{url, ...}]}`
///
/// @see <https://fal.ai/docs/documentation/model-apis/inference/synchronous> — Verbatim: "`run`
///   sends a direct HTTP request to `fal.run`. There is no queue and no status polling."
/// @see <https://fal.ai/explore> — the model index (fal.ai/models 308-redirects here;
///   2026-11-09): per-model API pages documenting each endpoint's input schema
pub struct FalProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
    /// How every request to the vendor's API is authenticated.
    auth: crate::providers::auth::AuthSpec,
    /// Result downloads: no redirects, so a URL that passed the public-address
    /// check cannot bounce into a private one.
    asset_client: Client,
    /// Whether a result URL must resolve to a public address. Only the tests,
    /// whose mock vendor runs on loopback, turn it off.
    pub(crate) public_assets_only: bool,
}

impl FalProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(300), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            client: Self::api_client(&Default::default()),
            auth: crate::providers::model3d::fal::fal_auth(),
            asset_client: crate::util::ssrf::public_only_download_client(
                std::time::Duration::from_secs(120),
                std::time::Duration::from_secs(30),
            ),
            public_assets_only: true,
        }
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("fal".into()))
    }

    /// The host images RUN on. `providers.fal.api_base` is shared with the
    /// video and 3D adapters, for which it is fal's QUEUE host, so fal's own
    /// queue host means its sync host here, and any other base (a proxy) is
    /// used for both; `options.sync_api_base` names the sync host outright.
    ///
    /// @see <https://fal.ai/docs/documentation/model-apis/inference/queue> — Verbatim: "Submit
    ///   requests to queue.fal.run"
    pub(crate) fn sync_base(&self) -> String {
        let config = self.config.as_ref();
        if let Some(base) = config.and_then(|c| c.options.as_ref()).and_then(|o| o["sync_api_base"].as_str()) {
            return base.trim_end_matches('/').to_string();
        }
        match config.and_then(|c| c.api_base.as_deref()).map(|b| b.trim_end_matches('/')) {
            None => DEFAULT_SYNC_BASE.to_string(),
            Some(b) if b == DEFAULT_QUEUE_BASE => DEFAULT_SYNC_BASE.to_string(),
            Some(b) => b.to_string(),
        }
    }


    /// Resolve litegen model ID to the Fal endpoint path (e.g. "fal-ai/flux/dev").
    /// `None` for an id with no arm: refused, not billed as FLUX.1 [dev] (L13).
    fn resolve_endpoint(&self, model_id: &str) -> Option<&'static str> {
        let native = if let Some(rest) = model_id.strip_prefix("fal/") {
            rest
        } else {
            model_id
        };

        Some(match native {
            "flux-schnell" => "fal-ai/flux/schnell",
            "flux-dev" => "fal-ai/flux/dev",
            // FLUX.2 [dev] text-to-image. Without this arm the id fell back to
            // FLUX.1 [dev] below and billed as a different model.
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2>
            "flux-2" => "fal-ai/flux-2",
            // FLUX.2 [dev]'s turbo and flash distillations, [pro], [max],
            // [flex], and [klein] 4B / 9B with their base models. Each has an
            // /edit twin (`edit_twin`).
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2/turbo> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2/flash> (2026-11-04)
            "flux-2-turbo" => "fal-ai/flux-2/turbo",
            "flux-2-flash" => "fal-ai/flux-2/flash",
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2-pro> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2-max> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2-flex> (2026-11-04)
            "flux-2-pro" => "fal-ai/flux-2-pro",
            "flux-2-max" => "fal-ai/flux-2-max",
            "flux-2-flex" => "fal-ai/flux-2-flex",
            // FLUX.2 [pro] outpaint: `outpaint_body`, no prompt.
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2-pro/outpaint> (2026-11-04)
            "flux-2-pro-outpaint" => "fal-ai/flux-2-pro/outpaint",
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2/klein/4b> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2/klein/9b/base> (2026-11-04)
            "flux-2-klein-4b" => "fal-ai/flux-2/klein/4b",
            "flux-2-klein-9b" => "fal-ai/flux-2/klein/9b",
            "flux-2-klein-4b-base" => "fal-ai/flux-2/klein/4b/base",
            "flux-2-klein-9b-base" => "fal-ai/flux-2/klein/9b/base",
            "flux-pro" => "fal-ai/flux-pro/v1.1",
            // FLUX.1 [pro] variants (2026-11-04). Kontext starts at its
            // text-to-image path; `kontext_paths` moves images on.
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/kontext/text-to-image> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/kontext/max/text-to-image> (2026-11-04)
            "flux-kontext-pro" => "fal-ai/flux-pro/kontext/text-to-image",
            "flux-kontext-max" => "fal-ai/flux-pro/kontext/max/text-to-image",
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/v1.1-ultra> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/v1.1-ultra/redux> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/v1.1/redux> (2026-11-04)
            "flux-pro-1.1-ultra" => "fal-ai/flux-pro/v1.1-ultra",
            "flux-pro-1.1-ultra-redux" => "fal-ai/flux-pro/v1.1-ultra/redux",
            "flux-pro-1.1-redux" => "fal-ai/flux-pro/v1.1/redux",
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/v1/fill> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/v1/erase> (2026-11-04)
            "flux-pro-fill" => "fal-ai/flux-pro/v1/fill",
            "flux-pro-erase" => "fal-ai/flux-pro/v1/erase",
            "sdxl" | "sdxl-lightning" => "fal-ai/fast-sdxl",
            "sd35-medium" | "sd3.5-medium" => "fal-ai/stable-diffusion-v35-medium",
            // fal renamed `fal-ai/recraft-v3`: its model page 308-redirects to
            // this id and the model registry no longer lists the old one. The
            // input schema is identical, so only the path moves.
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/v3/text-to-image>
            "recraft-v3" => "fal-ai/recraft/v3/text-to-image",
            // Recraft V4 and V4.1 (2026-11-04): prompt, image_size, colors.
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/v4/text-to-image> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/v4.1/text-to-image> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=recraft/v4.1/flash/text-to-image> (2026-11-04)
            // Recraft V2 at the registry's listed id (its page redirects to
            // fal-ai/recraft/v2/text-to-image, the same input).
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft-20b> (2026-11-04)
            "recraft-v2" => "fal-ai/recraft-20b",
            "recraft-v4" => "fal-ai/recraft/v4/text-to-image",
            "recraft-v4-pro" => "fal-ai/recraft/v4/pro/text-to-image",
            "recraft-v4-vector" => "fal-ai/recraft/v4/text-to-vector",
            "recraft-v4-pro-vector" => "fal-ai/recraft/v4/pro/text-to-vector",
            "recraft-v4.1" => "fal-ai/recraft/v4.1/text-to-image",
            "recraft-v4.1-pro" => "fal-ai/recraft/v4.1/pro/text-to-image",
            "recraft-v4.1-vector" => "fal-ai/recraft/v4.1/text-to-vector",
            "recraft-v4.1-pro-vector" => "fal-ai/recraft/v4.1/pro/text-to-vector",
            "recraft-v4.1-utility" => "fal-ai/recraft/v4.1/utility/text-to-image",
            "recraft-v4.1-utility-pro" => "fal-ai/recraft/v4.1/utility/pro/text-to-image",
            "recraft-v4.1-flash" => "recraft/v4.1/flash/text-to-image",
            // Recraft V4 style references, vectorize and upscale (2026-11-04).
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=recraft/v4/style/text-to-image> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/upscale/creative> (2026-11-04)
            "recraft-v4-style" => "recraft/v4/style/text-to-image",
            "recraft-v4-style-pro" => "recraft/v4/style/pro/text-to-image",
            "recraft-v4-style-vector" => "recraft/v4/style/text-to-vector",
            "recraft-v4-style-pro-vector" => "recraft/v4/style/pro/text-to-vector",
            "recraft-vectorize" => "fal-ai/recraft/vectorize",
            "recraft-upscale-crisp" => "fal-ai/recraft/upscale/crisp",
            "recraft-upscale-creative" => "fal-ai/recraft/upscale/creative",
            "auraflow" | "aura-flow" => "fal-ai/aura-flow",
            // Background removal (PixelGenie L8).
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/bria/background/remove>
            "bria-rmbg-2" => "fal-ai/bria/background/remove",
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/birefnet/v2>
            "birefnet-v2" => "fal-ai/birefnet/v2",
            // FLUX 3 (each with an /edit-image twin, `edit_twin`).
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=blackforestlabs/flux-3/text-to-image> (2026-11-04)
            "flux-3-image" => "blackforestlabs/flux-3/text-to-image",
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=blackforestlabs/flux-3/klein/text-to-image> (2026-11-04)
            "flux-3-image-klein" => "blackforestlabs/flux-3/klein/text-to-image",
            // Klein base (undistilled), which fal serves only at /lora
            // endpoints. Their `loras` is optional ("Default: []"), and
            // litegen sends none.
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=blackforestlabs/flux-3/klein/base/text-to-image/lora> (2026-11-04)
            "flux-3-image-klein-base" => "blackforestlabs/flux-3/klein/base/text-to-image/lora",
            _ => return None,
        })
    }

    /// The edit endpoint a text-to-image endpoint has, which a request with
    /// images goes to ("image_urls … maxItems 10").
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=blackforestlabs/flux-3/edit-image> (2026-11-04)
    fn edit_twin(endpoint: &str) -> Option<&'static str> {
        match endpoint {
            "blackforestlabs/flux-3/text-to-image" => Some("blackforestlabs/flux-3/edit-image"),
            "blackforestlabs/flux-3/klein/text-to-image" => Some("blackforestlabs/flux-3/klein/edit-image"),
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=blackforestlabs/flux-3/klein/base/edit-image/lora> (2026-11-04)
            "blackforestlabs/flux-3/klein/base/text-to-image/lora" => Some("blackforestlabs/flux-3/klein/base/edit-image/lora"),
            // FLUX.2: `{endpoint}/edit`, "image_urls … A maximum of 4 images"
            // on dev, turbo, flash and klein; [pro], [max] and [flex] state no
            // maximum (the rows keep BFL's 8).
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2/edit> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2-pro/edit> (2026-11-04)
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2/klein/9b/base/edit> (2026-11-04)
            "fal-ai/flux-2" => Some("fal-ai/flux-2/edit"),
            "fal-ai/flux-2/turbo" => Some("fal-ai/flux-2/turbo/edit"),
            "fal-ai/flux-2/flash" => Some("fal-ai/flux-2/flash/edit"),
            "fal-ai/flux-2-pro" => Some("fal-ai/flux-2-pro/edit"),
            "fal-ai/flux-2-max" => Some("fal-ai/flux-2-max/edit"),
            "fal-ai/flux-2-flex" => Some("fal-ai/flux-2-flex/edit"),
            "fal-ai/flux-2/klein/4b" => Some("fal-ai/flux-2/klein/4b/edit"),
            "fal-ai/flux-2/klein/9b" => Some("fal-ai/flux-2/klein/9b/edit"),
            "fal-ai/flux-2/klein/4b/base" => Some("fal-ai/flux-2/klein/4b/base/edit"),
            "fal-ai/flux-2/klein/9b/base" => Some("fal-ai/flux-2/klein/9b/base/edit"),
            _ => None,
        }
    }

    /// Where one image goes (`image_url`), and where several go
    /// (`image_urls`), for models whose images have endpoints of their own:
    /// Kontext (two to four at /multi; BFL's input_image … input_image_4) and
    /// Recraft V3 (one, at /image-to-image).
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/kontext/multi> (2026-11-04)
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/v3/image-to-image> (2026-11-04)
    fn image_paths(endpoint: &str) -> Option<(&'static str, Option<&'static str>)> {
        match endpoint {
            "fal-ai/flux-pro/kontext/text-to-image" => Some(("fal-ai/flux-pro/kontext", Some("fal-ai/flux-pro/kontext/multi"))),
            "fal-ai/flux-pro/kontext/max/text-to-image" => Some(("fal-ai/flux-pro/kontext/max", Some("fal-ai/flux-pro/kontext/max/multi"))),
            "fal-ai/recraft/v3/text-to-image" => Some(("fal-ai/recraft/v3/image-to-image", None)),
            _ => None,
        }
    }

    /// Endpoints that take their references as one `image_urls` list without
    /// an edit twin: Recraft V4's style endpoints ("image_urls … maxItems 10").
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=recraft/v4/style/text-to-image> (2026-11-04)
    fn lists_images(endpoint: &str) -> bool {
        matches!(
            endpoint,
            "recraft/v4/style/text-to-image" | "recraft/v4/style/pro/text-to-image" | "recraft/v4/style/text-to-vector" | "recraft/v4/style/pro/text-to-vector"
        )
    }

    /// litegen's tier as FLUX 3 Image's on fal: "768sq", "1k", "1.5k", "2k", "4k".
    fn flux_3_tier(tier: &str) -> Option<&'static str> {
        match tier {
            "0.75K" => Some("768sq"),
            "1K" => Some("1k"),
            "1.5K" => Some("1.5k"),
            "2K" => Some("2k"),
            "4K" => Some("4k"),
            _ => None,
        }
    }

    /// Parse a size string "WxH" into {width, height}.
    fn parse_size(size: &str) -> Option<(u64, u64)> {
        let parts: Vec<&str> = size.split('x').collect();
        if parts.len() == 2 {
            let w = parts[0].parse::<u64>().ok()?;
            let h = parts[1].parse::<u64>().ok()?;
            Some((w, h))
        } else {
            None
        }
    }

    /// Download a result from the URL fal answered with: without the key,
    /// only from a public address, and never past the size limit. fal has run
    /// and billed the job by now, so no failure here is worth sending the
    /// request again — every error is final.
    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        match fetch_asset(&self.asset_client, url, MAX_IMAGE_BYTES, self.public_assets_only, "fal image").await {
            Ok((bytes, _)) => Ok(bytes),
            Err(AssetError::Transient(ProviderError::RequestFailed { message, status_code, provider_error, .. })) => {
                Err(ProviderError::RequestFailed { message, status_code, provider_error, retryable: false })
            }
            Err(AssetError::Transient(e)) => Err(e),
            Err(AssetError::Gone(why)) => Err(ProviderError::RequestFailed {
                message: format!("fal image result could not be fetched: {why}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            }),
        }
    }

    /// A result file's bytes: decoded from a `data:` URI (`sync_mode`), or
    /// fetched without the key.
    async fn result_bytes(&self, at: &str) -> Result<Vec<u8>, ProviderError> {
        use base64::Engine;
        match at.strip_prefix("data:").and_then(|d| d.split_once(";base64,")) {
            Some((_, b64)) => base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| ProviderError::RequestFailed {
                message: format!("Fal result is not base64: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            }),
            None => self.fetch_image_bytes(at).await,
        }
    }

    /// POST `body` to `{sync}/{endpoint}` and read the JSON answer. The reply
    /// is read whole before its status is judged, so a gateway's HTML page is
    /// a retryable 502, not a parse error; a vendor error names every message
    /// in any of fal's error shapes; and fal's `X-Fal-Needs-Retry` decides
    /// whether to ask again when it sent one. A connection that never opened
    /// is the only send failure worth repeating: fal runs, and bills, a
    /// request it received.
    ///
    /// @see <https://fal.ai/docs/api-reference/inference-api/sync/run.md> — Verbatim: "fal does not
    ///   retry synchronous requests. If a request fails because of a temporary error, the response
    ///   has `X-Fal-Needs-Retry: 1`."
    /// @see <https://fal.ai/docs/documentation/model-apis/errors> — Verbatim: "Contains a `detail`
    ///   field which is an array of `Error` objects."
    async fn run(&self, endpoint: &str, body: &Value) -> Result<Value, ProviderError> {
        let api_key = self.api_key()?;
        let url = format!("{}/{}", self.sync_base(), endpoint);
        let resp = crate::providers::inject_trace_headers(self.client.post(&url).authorized(&self.auth, &api_key)?.json(body))
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Fal request failed: {e}"),
                status_code: None,
                provider_error: None,
                retryable: e.is_connect(),
            })?;
        let retry = needs_retry(resp.headers());
        let reply = QueueReply::read(resp).await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Fal response could not be read: {e}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;
        if !reply.status.is_success() {
            let message = format!("Fal API error: {}", reply.describe());
            if let Some(e) = reply.rate_limited(&message) {
                return Err(e);
            }
            return Err(ProviderError::RequestFailed {
                message,
                status_code: Some(reply.status.as_u16()),
                retryable: retry.unwrap_or_else(|| transient(reply.status)),
                provider_error: reply.json,
            });
        }
        reply.json.ok_or_else(|| ProviderError::RequestFailed {
            message: format!("Fal answered HTTP {} with a body that is not JSON", reply.status.as_u16()),
            status_code: Some(reply.status.as_u16()),
            provider_error: None,
            retryable: false,
        })
    }

    /// One image in, one `image` out: background removal (PixelGenie L8,
    /// `sync_mode` on) and Recraft's vectorize and upscalers. Only the image,
    /// the endpoint's own knobs from `extra`, then the asked container, so
    /// `extra` cannot change it. Bria's optional `prompt` is billed $0.03
    /// instead of $0.018 and is never sent.
    async fn image_op(&self, endpoint: &str, extras: &ImageExtras, materialized: &MaterializedRequest, sync_mode: bool) -> Result<GenerationOutput, ProviderError> {
        let image_url = materialized.refs.iter().find(|r| r.role == "init").and_then(|r| match &r.form {
            MaterializedRefForm::Url(u) => Some(u.clone()),
            MaterializedRefForm::Base64(b64) => Some(format!("data:image/png;base64,{b64}")),
            MaterializedRefForm::MultipartField { .. } => None,
        });
        let image_url = image_url.ok_or_else(|| ProviderError::InvalidRequest(format!("{endpoint} needs an init image")))?;
        let mut body = json!({ "image_url": image_url });
        if sync_mode {
            body["sync_mode"] = Value::Bool(true);
        }
        if let Some(obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
                obj.insert(k.clone(), v.clone());
            }
        }
        if let Some(f) = extras.output_format.as_deref() {
            body["output_format"] = Value::String(f.to_string());
        }
        let answer = self.run(endpoint, &body).await?;
        let at = answer["image"]["url"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Fal response has no image".into(),
            status_code: None,
            provider_error: Some(answer.clone()),
            retryable: false,
        })?;
        Ok(GenerationOutput {
            data: self.result_bytes(at).await?,
            content_type: answer["image"]["content_type"].as_str().unwrap_or("image/png").to_string(),
            metadata: HashMap::new(),
            more: Vec::new(),
        })
    }

    /// FLUX.2 [pro] outpaint's body: the init image, and the pixels to add to
    /// each side as `expand_left`, `expand_top`, `expand_right` and
    /// `expand_bottom` ("Pixels to expand on the left of the image", 0-2048).
    /// The endpoint takes no prompt. `extra` adds `mode` and `auto_crop` but
    /// can't change a side; then the asked container.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2-pro/outpaint> (2026-11-04)
    fn outpaint_body(model: &str, extras: &ImageExtras, materialized: &MaterializedRequest) -> Result<Value, ProviderError> {
        let image_url = materialized
            .refs
            .iter()
            .find(|r| r.role == "init")
            .and_then(|r| match &r.form {
                MaterializedRefForm::Url(u) => Some(u.clone()),
                MaterializedRefForm::Base64(b64) => Some(format!("data:image/png;base64,{b64}")),
                MaterializedRefForm::MultipartField { .. } => None,
            })
            .ok_or_else(|| ProviderError::InvalidRequest(format!("{model}: outpainting needs an init image")))?;
        let e = extras
            .expand
            .filter(|e| e.left + e.top + e.right + e.bottom > 0)
            .ok_or_else(|| ProviderError::InvalidRequest(format!("{model}: say how many pixels to add to at least one side (`expand`)")))?;
        let mut body = json!({
            "image_url": image_url,
            "expand_left": e.left,
            "expand_top": e.top,
            "expand_right": e.right,
            "expand_bottom": e.bottom,
        });
        if let Some(obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
                obj.insert(k.clone(), v.clone());
            }
        }
        if let Some(f) = extras.output_format.as_deref() {
            body["output_format"] = Value::String(f.to_string());
        }
        Ok(body)
    }

    /// The first image of fal's `{images: [{url, content_type}]}` answer,
    /// downloaded.
    async fn first_image(&self, model: &ModelSchema, resp_json: Value) -> Result<GenerationOutput, ProviderError> {
        let images = resp_json["images"].as_array().ok_or_else(|| ProviderError::RequestFailed {
            message: "Fal response missing images array".to_string(),
            status_code: None,
            provider_error: Some(resp_json.clone()),
            retryable: false,
        })?;
        let first_image = images.first().ok_or_else(|| ProviderError::RequestFailed {
            message: "Fal response has empty images array".to_string(),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;
        let image_url = first_image["url"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Fal image missing url".to_string(),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;
        let content_type = first_image["content_type"].as_str().unwrap_or("image/jpeg").to_string();
        let image_bytes = self.result_bytes(image_url).await?;
        let mut metadata = HashMap::new();
        metadata.insert("url".to_string(), Value::String(image_url.to_string()));
        metadata.insert("model".to_string(), Value::String(model.id.clone()));
        Ok(GenerationOutput {
            data: image_bytes,
            content_type,
            metadata,
            more: Vec::new(),
        })
    }
}

/// fal's segmentation endpoints (PixelGenie L7), by catalog id. SAM 3 and
/// SAM 3.1 publish the same input.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/sam-3/image>
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/sam-3-1/image>
pub(crate) const SEGMENT_MODELS: &[(&str, &str)] = &[("fal/sam-3", "fal-ai/sam-3/image"), ("fal/sam-3.1", "fal-ai/sam-3-1/image")];

fn segment_endpoint(model_id: &str) -> Option<&'static str> {
    SEGMENT_MODELS.iter().find(|(id, _)| *id == model_id).map(|(_, e)| *e)
}

impl Default for FalProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for FalProvider {
    fn name(&self) -> &str {
        "fal"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
        }
        self.client = Self::api_client(&config.extra_headers);
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| !c.api_key.is_empty() || self.key_pool.is_some())
    }

    /// Run a model synchronously via `POST {fal.run}/{endpoint}`.
    ///
    /// @see <https://fal.ai/docs/documentation/model-apis/inference/synchronous> — synchronous run;
    ///   `Authorization: Key <token>`, JSON body, and the `{images: [{url, content_type}]}` response
    ///   shape this method parses.
    /// @see <https://fal.ai/explore> — the model index (fal.ai/models 308-redirects here;
    ///   2026-11-09): per-model pages proving the `image_size`, `seed`, `guidance_scale`,
    ///   `num_inference_steps`, `strength`, and `image_url` input fields.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let endpoint = self.resolve_endpoint(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "fal".into() })?;
        // fal names the model by its endpoint: a mapping replaces the path.
        let endpoint = crate::providers::wire_model(self.config.as_ref(), &model.id, endpoint);
        if model.capabilities.background_removal {
            return self.image_op(endpoint, extras, materialized, true).await;
        }
        // Recraft's vectorize and upscalers: one image in, one `image` out.
        // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/vectorize> (2026-11-04)
        if matches!(endpoint, "fal-ai/recraft/vectorize" | "fal-ai/recraft/upscale/crisp" | "fal-ai/recraft/upscale/creative") {
            return self.image_op(endpoint, extras, materialized, false).await;
        }
        // Outpainting takes no prompt: its own body, then the same `images`
        // answer.
        if model.params.contains_key("expand") {
            let body = Self::outpaint_body(&model.id, extras, materialized)?;
            let answer = self.run(endpoint, &body).await?;
            return self.first_image(model, answer).await;
        }
        // A request with images goes to the model's edit twin, its images as one
        // `image_urls` list.
        let image_list = Self::edit_twin(endpoint).is_some();
        let endpoint = match Self::edit_twin(endpoint) {
            Some(twin) if !materialized.refs.is_empty() => twin,
            _ => endpoint,
        };
        // Kontext and Recraft V3: one image goes to its own path as
        // `image_url`, two or more (Kontext) to `/multi` as `image_urls`.
        let images = materialized.refs.iter().filter(|r| r.role != "mask").count();
        let (endpoint, image_list) = match Self::image_paths(endpoint) {
            Some((one, _)) if images == 1 => (one, false),
            Some((_, Some(many))) if images > 1 => (many, true),
            _ => (endpoint, image_list || Self::lists_images(endpoint)),
        };

        // Build request body. A row that takes no prompt (erase) sends none:
        // its endpoint has no such field.
        let mut body = if model.prompt.max_length == Some(0) { json!({}) } else { json!({ "prompt": base.prompt }) };

        // Fal models use image_size: {width, height}
        if let Some(size_str) = extras.size.as_deref() {
            if let Some((w, h)) = Self::parse_size(size_str) {
                body["image_size"] = json!({
                    "width": w,
                    "height": h
                });
            }
        }

        // No `num_images`: every carried endpoint defaults to 1, and litegen
        // returns (and bills) one image — fal would bill every one it made.

        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }

        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }

        if let Some(gc) = extras.guidance_scale {
            body["guidance_scale"] = json!(gc);
        }

        if let Some(steps) = extras.steps {
            body["num_inference_steps"] = Value::Number(steps.into());
        }

        if let Some(strength) = extras.strength {
            body["strength"] = json!(strength);
        }

        // Style (e.g. recraft-v3: realistic_image, digital_illustration, vector_illustration)
        if let Some(style) = extras.style.as_deref() {
            body["style"] = Value::String(style.to_string());
        }
        // Recraft V3's text-to-image takes neither `strength` nor
        // `negative_prompt`; only its /image-to-image twin does.
        if endpoint == "fal-ai/recraft/v3/text-to-image" {
            if let Some(o) = body.as_object_mut() {
                o.remove("strength");
                o.remove("negative_prompt");
            }
        }

        // PixelGenie L16: a palette as Recraft's preferred `colors`
        // ([{r, g, b}], a hint) on the rows that declare one.
        // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/v4.1/text-to-image> (2026-11-04)
        if let Some(palette) = extras.palette.as_ref() {
            let colors: Vec<Value> = palette
                .iter()
                .filter_map(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok())
                .map(|v| json!({ "r": (v >> 16) & 0xff, "g": (v >> 8) & 0xff, "b": v & 0xff }))
                .collect();
            body["colors"] = Value::Array(colors);
        }

        // `aspect_ratio` reaches only rows that declare it (FLUX 3 Image,
        // Kontext, ultra), and each of their endpoints takes it.
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        // FLUX 3 Image's own tier spelling (Klein's rows declare none: it
        // takes `image_size`).
        if image_list {
            if let Some(tier) = extras.resolution.as_deref() {
                let t = Self::flux_3_tier(tier).ok_or_else(|| ProviderError::InvalidRequest(format!("{}: fal has no FLUX 3 tier {tier}", model.id)))?;
                body["resolution"] = Value::String(t.to_string());
            }
        }

        // Reference images (URL format for Fal): one `image_url`, or the edit
        // twin's `image_urls` list in order. A mask (already in the row's
        // convention) goes alone as `mask_url`.
        let mut urls = Vec::new();
        let mut mask_url = None;
        for r in &materialized.refs {
            let url = match &r.form {
                MaterializedRefForm::Url(img_url) => img_url.clone(),
                MaterializedRefForm::Base64(b64) => format!("data:image/png;base64,{b64}"),
                _ => continue,
            };
            if r.role == "mask" {
                mask_url = Some(url);
            } else {
                urls.push(url);
            }
        }
        if image_list {
            if !urls.is_empty() {
                body["image_urls"] = json!(urls);
            }
        } else if let Some(last) = urls.pop() {
            body["image_url"] = Value::String(last);
        }
        if let Some(mask) = mask_url {
            body["mask_url"] = Value::String(mask);
        } else if model.capabilities.inpainting {
            // Fill and erase require `mask_url`: a mask, or a region the core
            // drew as one.
            return Err(ProviderError::InvalidRequest(format!("{}: send a mask, or a region to edit", model.id)));
        }

        // `extra` adds the endpoint's own knobs; it cannot replace a field
        // litegen sets (rule 11), which lax mode would otherwise let it.
        if let Some(body_obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
                body_obj.insert(k.clone(), v.clone());
            }
        }
        // PixelGenie L9: the asked container — fast-sdxl names it `format`
        // ("enum": ["jpeg", "png"]), every other endpoint `output_format`.
        // @see https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/fast-sdxl (2026-11-03)
        if let Some(f) = extras.output_format.as_deref() {
            let field = if endpoint == "fal-ai/fast-sdxl" { "format" } else { "output_format" };
            body[field] = Value::String(f.to_string());
        }

        let resp_json = self.run(endpoint, &body).await?;
        self.first_image(model, resp_json).await
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(
            model.pricing.base_cost_usd,
            0.0,
            CostSource::Estimated,
            Some(json!({ "model": model.id })),
        ))
    }

    /// Promptable segmentation on SAM 3 and SAM 3.1 (PixelGenie L7), run
    /// synchronously like `generate`. Points and boxes go as one object
    /// (`object_id` 1), boxes as corners; a text prompt asks for every
    /// instance. `sync_mode` returns masks as data URIs; one that comes as a
    /// URL is downloaded without the key.
    ///
    /// @see <https://fal.ai/models/fal-ai/sam-3/image/api> — the endpoint and its examples
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/sam-3/image> — `prompt`
    ///   (default "wheel"), `point_prompts`, `box_prompts` (`x_min`…`y_max`, "use object_id to group
    ///   boxes for the same object"), `return_multiple_masks`, `max_masks` 1–32, `apply_mask`,
    ///   `include_scores`, `sync_mode`; output `masks`, `scores`, `metadata[{index, score}]`
    async fn segment(&self, model: &ModelSchema, input: &SegmentationInput) -> Result<Vec<VendorMask>, ProviderError> {
        use base64::Engine;
        let endpoint = segment_endpoint(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "fal".into() })?;
        let (mut text, mut points, mut boxes) = (None, Vec::new(), Vec::new());
        for p in &input.prompts {
            match p {
                SegmentationPrompt::Point { x, y, label } => points.push(json!({ "x": x, "y": y, "label": label, "object_id": 1 })),
                SegmentationPrompt::Box { x, y, w, h } => {
                    boxes.push(json!({ "x_min": x, "y_min": y, "x_max": x + w, "y_max": y + h, "object_id": 1 }))
                }
                SegmentationPrompt::Text { text: t } => text = Some(t.clone()),
            }
        }
        let mut body = json!({
            "image_url": format!("data:{};base64,{}", input.content_type, base64::engine::general_purpose::STANDARD.encode(&input.image)),
            // The schema's default prompt is "wheel": a request without text sends none.
            "prompt": text.clone().unwrap_or_default(),
            "point_prompts": points,
            "box_prompts": boxes,
            "return_multiple_masks": text.is_some(),
            "apply_mask": false,
            "include_scores": true,
            "sync_mode": true,
            "output_format": "png",
        });
        if text.is_some() {
            body["max_masks"] = json!(input.max_masks);
        }
        let answer = self.run(endpoint, &body).await?;
        let masks = answer["masks"].as_array().ok_or_else(|| ProviderError::RequestFailed {
            message: "Fal response missing masks".into(),
            status_code: None,
            provider_error: Some(answer.clone()),
            retryable: false,
        })?;
        let mut out = Vec::with_capacity(masks.len());
        for (i, m) in masks.iter().enumerate() {
            let Some(at) = m["url"].as_str() else {
                return Err(ProviderError::RequestFailed {
                    message: format!("Fal mask {i} has no url"),
                    status_code: None,
                    provider_error: None,
                    retryable: false,
                });
            };
            let bytes = self.result_bytes(at).await?;
            let score = answer["scores"][i].as_f64().or_else(|| {
                answer["metadata"].as_array()?.iter().find(|d| d["index"].as_u64() == Some(i as u64))?["score"].as_f64()
            });
            out.push(VendorMask { bytes, score });
        }
        Ok(out)
    }

    /// Check the key against fal's Platform API (`platform_key_check`).
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Fal provider not configured".into(),
                latency_ms: None,
            };
        }
        let Ok(api_key) = self.api_key() else {
            return HealthCheckResult { healthy: false, message: "No API key".into(), latency_ms: None };
        };
        platform_key_check(&self.client, crate::providers::model3d::fal::platform_base_for(self.config.as_ref()).as_deref(), &self.sync_base(), &api_key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

    /// L13: every catalog row has its own arm, and an id with none is refused
    /// rather than billed as another model.
    #[test]
    fn every_catalog_row_resolves_and_an_unknown_id_does_not() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        let segments = |m: &crate::capabilities::ModelSchema| m.task == crate::capabilities::ModelTask::Segment;
        for m in catalog.for_provider("fal").filter(|m| m.media_type == crate::capabilities::MediaType::Image) {
            if segments(m) {
                assert!(segment_endpoint(&m.id).is_some(), "{} has no segmentation arm", m.id);
            } else {
                assert!(FalProvider::new().resolve_endpoint(&m.id).is_some(), "{} has no arm", m.id);
            }
        }
        for (id, _) in SEGMENT_MODELS {
            assert!(catalog.get(id).is_some_and(segments), "{id} is not a catalog segmentation row");
        }
        assert!(FalProvider::new().resolve_endpoint("fal/not-a-model").is_none());
        assert!(segment_endpoint("fal/flux-dev").is_none());
    }

    // ─── Background removal (PixelGenie L8) ────────────────────────────────────

    fn init_ref(form: crate::proxy::materializer::MaterializedRefForm) -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef { role: "init".into(), form, time_s: None }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    fn matting_extras() -> ImageExtras {
        ImageExtras { size: None, steps: None, guidance_scale: None, ..make_extras() }
    }

    /// Bria's `prompt` "Change[s] what the cutout keeps … Billed at $0.03
    /// instead of $0.018", so it is never sent; nor is any generation field.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/bria/background/remove>
    #[tokio::test]
    async fn bria_rmbg_is_sent_only_the_image_and_its_cutout_comes_back() {
        use crate::proxy::materializer::MaterializedRefForm;
        let server = MockServer::start().await;
        let cutout = png_of(4, 4, |_, _| true);
        Mock::given(method("POST"))
            .and(path("/fal-ai/bria/background/remove"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "image": { "url": data_uri(&cutout), "content_type": "image/png" } })))
            .mount(&server)
            .await;
        let out = make_provider(&server.uri())
            .generate(
                &ref_schema("fal/bria-rmbg-2"),
                &make_base("", "fal/bria-rmbg-2"),
                &matting_extras(),
                &init_ref(MaterializedRefForm::Url("https://cdn.example/sprite.png".into())),
            )
            .await
            .expect("generate");
        assert_eq!(out.data, cutout);
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body, json!({ "image_url": "https://cdn.example/sprite.png", "sync_mode": true }));
    }

    /// BiRefNet takes the asked container and its own knobs from `extra`;
    /// its result can come back as a URL, fetched without the key.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/birefnet/v2>
    #[tokio::test]
    async fn birefnet_gets_the_asked_container_and_its_own_knobs() {
        use crate::proxy::materializer::MaterializedRefForm;
        let (server, cdn) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("POST"))
            .and(path("/fal-ai/birefnet/v2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "image": { "url": format!("{}/cut.webp", cdn.uri()) }, "mask_image": null })))
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/cut.webp")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"WEBPCUT".to_vec())).mount(&cdn).await;
        let extras = ImageExtras {
            output_format: Some("webp".into()),
            extra: Some(json!({ "operating_resolution": "2048x2048" })),
            ..matting_extras()
        };
        let out = make_provider(&server.uri())
            .generate(&ref_schema("fal/birefnet-v2"), &make_base("", "fal/birefnet-v2"), &extras, &init_ref(MaterializedRefForm::Base64("aGk=".into())))
            .await
            .expect("generate");
        assert_eq!(out.data, b"WEBPCUT");
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(
            body,
            json!({ "image_url": "data:image/png;base64,aGk=", "sync_mode": true, "output_format": "webp", "operating_resolution": "2048x2048" })
        );
        assert!(cdn.received_requests().await.unwrap()[0].headers.get("authorization").is_none(), "no key to the CDN");
    }

    // ─── Segmentation (PixelGenie L7) ─────────────────────────────────────────

    fn png_of(w: u32, h: u32, white: impl Fn(u32, u32) -> bool) -> Vec<u8> {
        let img = image::GrayImage::from_fn(w, h, |x, y| image::Luma([if white(x, y) { 255 } else { 0 }]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn data_uri(bytes: &[u8]) -> String {
        use base64::Engine;
        format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    fn seg_input(prompts: Value) -> SegmentationInput {
        SegmentationInput {
            image: png_of(32, 32, |_, _| false).into(),
            content_type: "image/png".into(),
            width: 32,
            height: 32,
            prompts: serde_json::from_value(prompts).unwrap(),
            max_masks: 32,
        }
    }

    /// Points and boxes select one object: SAM 3 groups prompts by
    /// `object_id`, and takes boxes as `x_min, y_min, x_max, y_max`. Its
    /// `prompt` defaults to "wheel", so a request without text sends "".
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/sam-3/image>
    #[tokio::test]
    async fn sam_3_gets_points_and_boxes_as_one_object_and_returns_its_masks() {
        let server = MockServer::start().await;
        let mask = png_of(32, 32, |x, _| x < 4);
        Mock::given(method("POST"))
            .and(path("/fal-ai/sam-3/image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "masks": [{ "url": data_uri(&mask), "content_type": "image/png" }],
                "scores": [0.93],
            })))
            .mount(&server)
            .await;
        let input = seg_input(json!([
            { "type": "point", "x": 5, "y": 6 },
            { "type": "point", "x": 7, "y": 8, "label": 0 },
            { "type": "box", "x": 1, "y": 2, "w": 10, "h": 20 },
        ]));
        let masks = make_provider(&server.uri()).segment(&ref_schema("fal/sam-3"), &input).await.expect("segment");
        assert_eq!(masks.len(), 1);
        assert_eq!((masks[0].bytes.clone(), masks[0].score), (mask, Some(0.93)));

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Key test-key");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert!(body["image_url"].as_str().unwrap().starts_with("data:image/png;base64,"), "{body}");
        assert_eq!(body["prompt"], "");
        assert_eq!(
            body["point_prompts"],
            json!([{ "x": 5, "y": 6, "label": 1, "object_id": 1 }, { "x": 7, "y": 8, "label": 0, "object_id": 1 }])
        );
        assert_eq!(body["box_prompts"], json!([{ "x_min": 1, "y_min": 2, "x_max": 11, "y_max": 22, "object_id": 1 }]));
        for (field, want) in [
            ("return_multiple_masks", json!(false)),
            ("apply_mask", json!(false)),
            ("sync_mode", json!(true)),
            ("include_scores", json!(true)),
            ("output_format", json!("png")),
        ] {
            assert_eq!(body[field], want, "{field}: {body}");
        }
    }

    /// A text prompt asks for every instance, at most `max_masks` of them
    /// (SAM 3 takes 1–32); masks may come back as URLs, and scores in
    /// `metadata` when `scores` is absent.
    #[tokio::test]
    async fn a_text_prompt_asks_sam_3_for_every_instance() {
        let server = MockServer::start().await;
        let cdn = MockServer::start().await;
        let (first, second) = (png_of(32, 32, |x, _| x < 4), png_of(32, 32, |x, _| x > 20));
        Mock::given(method("POST"))
            .and(path("/fal-ai/sam-3/image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "masks": [{ "url": format!("{}/m0.png", cdn.uri()) }, { "url": data_uri(&second) }],
                "metadata": [{ "index": 0, "score": 0.8 }, { "index": 1, "score": 0.6 }],
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/m0.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(first.clone())).mount(&cdn).await;
        let mut input = seg_input(json!([{ "type": "text", "text": "visor" }]));
        input.max_masks = 5;
        let masks = make_provider(&server.uri()).segment(&ref_schema("fal/sam-3"), &input).await.expect("segment");
        assert_eq!(masks.iter().map(|m| (m.bytes.clone(), m.score)).collect::<Vec<_>>(), vec![(first, Some(0.8)), (second, Some(0.6))]);
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!((body["prompt"].clone(), body["return_multiple_masks"].clone(), body["max_masks"].clone()), (json!("visor"), json!(true), json!(5)));
        assert_eq!((body["point_prompts"].clone(), body["box_prompts"].clone()), (json!([]), json!([])));
        assert!(cdn.received_requests().await.unwrap()[0].headers.get("authorization").is_none(), "no key to the CDN");
    }

    #[tokio::test]
    async fn sam_3_refusing_the_request_is_the_vendors_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fal-ai/sam-3-1/image"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": [{ "msg": "image_url could not be read" }] })))
            .mount(&server)
            .await;
        let err = make_provider(&server.uri())
            .segment(&ref_schema("fal/sam-3.1"), &seg_input(json!([{ "type": "box", "x": 0, "y": 0, "w": 4, "h": 4 }])))
            .await
            .unwrap_err();
        match err {
            ProviderError::RequestFailed { status_code: Some(422), retryable: false, provider_error: Some(body), .. } => {
                assert!(body.to_string().contains("could not be read"), "{body}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[cfg(test)]
    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    #[cfg(test)]
    fn empty_materialized() -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    /// A provider against a mock fal. Its result URLs are on loopback, so the
    /// public-address check is off — `a_result_is_fetched_only_from_a_public_address`
    /// covers it.
    fn make_provider(api_base: &str) -> FalProvider {
        let mut p = FalProvider::new();
        p.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: "test-key".to_string(),
            api_keys: vec![],
            api_base: Some(api_base.to_string()),
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });
        p.public_assets_only = false;
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(),
            model: model.to_string(),
            n: 1,
            negative_prompt: None,
            seed: Some(42),
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: Some("1024x1024".to_string()),
            resolution: None, aspect_ratio: None,
            quality: None,
            style: None,
            steps: Some(28),
            guidance_scale: Some(3.5),
            strength: None,
            response_format: "url".to_string(),
            edit: None, views: Vec::new(),
            extra: None,
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        }
    }

    /// FLUX 3 on fal: text-to-image at `blackforestlabs/flux-3/text-to-image`
    /// (`resolution` "1k", `aspect_ratio`); with images, its edit twin
    /// `…/edit-image` and an `image_urls` list ("maxItems 10"). Klein takes
    /// `image_size` {width, height} on both, no tier or ratio field.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=blackforestlabs/flux-3/edit-image> (2026-11-04)
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=blackforestlabs/flux-3/klein/edit-image> (2026-11-04)
    #[tokio::test]
    async fn flux_3_on_fal_routes_by_images() {
        let server = MockServer::start().await;
        let uri = server.uri();
        for at in ["blackforestlabs/flux-3/text-to-image", "blackforestlabs/flux-3/edit-image", "blackforestlabs/flux-3/klein/text-to-image", "blackforestlabs/flux-3/klein/edit-image"] {
            Mock::given(method("POST")).and(path(format!("/{at}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": format!("{uri}/out.png"), "content_type": "image/png" }] })))
                .mount(&server).await;
        }
        Mock::given(method("GET")).and(path("/out.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"\x89PNG\r\n\x1a\nX".to_vec())).mount(&server).await;
        let p = make_provider(&server.uri());
        let two = || MaterializedRequest {
            refs: vec![
                crate::proxy::materializer::MaterializedRef { role: "init".into(), form: MaterializedRefForm::Url("https://cdn.example/a.png".into()), time_s: None },
                crate::proxy::materializer::MaterializedRef { role: "init".into(), form: MaterializedRefForm::Url("https://cdn.example/b.png".into()), time_s: None },
            ],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        let none = || MaterializedRequest { refs: vec![], cleanup: crate::proxy::materializer::Cleanup::empty() };
        let body_at = |received: &[wiremock::Request], at: &str| -> Value {
            serde_json::from_slice(&received.iter().find(|r| r.url.path() == at).unwrap_or_else(|| panic!("no call to {at}")).body).unwrap()
        };
        // FLUX 3 Image, text then edit.
        let id = "fal/flux-3-image";
        let mut e = make_extras_no_size();
        e.resolution = Some("1K".into());
        e.aspect_ratio = Some("21:9".into());
        p.generate(&ref_schema(id), &make_base("a canyon", id), &e, &none()).await.expect("generated");
        p.generate(&ref_schema(id), &make_base("merge them", id), &make_extras_no_size(), &two()).await.expect("generated");
        let received = server.received_requests().await.unwrap();
        let t2i = body_at(&received, "/blackforestlabs/flux-3/text-to-image");
        assert_eq!((t2i["resolution"].as_str(), t2i["aspect_ratio"].as_str()), (Some("1k"), Some("21:9")), "{t2i}");
        assert!(t2i.get("image_urls").is_none() && t2i.get("image_size").is_none(), "{t2i}");
        let edit = body_at(&received, "/blackforestlabs/flux-3/edit-image");
        assert_eq!(edit["image_urls"], json!(["https://cdn.example/a.png", "https://cdn.example/b.png"]));
        assert!(edit.get("image_url").is_none(), "{edit}");
        // Klein: image_size, no tier or ratio.
        let id = "fal/flux-3-image-klein";
        let mut e = make_extras();
        e.size = Some("1536x1024".into());
        p.generate(&ref_schema(id), &make_base("a canyon", id), &e, &none()).await.expect("generated");
        p.generate(&ref_schema(id), &make_base("merge them", id), &make_extras_no_size(), &two()).await.expect("generated");
        let received = server.received_requests().await.unwrap();
        let t2i = body_at(&received, "/blackforestlabs/flux-3/klein/text-to-image");
        assert_eq!(t2i["image_size"], json!({ "width": 1536, "height": 1024 }));
        assert!(t2i.get("resolution").is_none() && t2i.get("aspect_ratio").is_none(), "{t2i}");
        assert_eq!(body_at(&received, "/blackforestlabs/flux-3/klein/edit-image")["image_urls"].as_array().map(Vec::len), Some(2));
    }

    /// FLUX 3 Klein base on fal: fal serves it only at its /lora endpoints
    /// (`…/klein/base/text-to-image/lora`, `…/klein/base/edit-image/lora`).
    /// `loras` is optional ("Default: []"), so litegen sends none. The rest is
    /// Klein's body: `image_size` {width, height}, and `image_urls` on the edit
    /// twin.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=blackforestlabs/flux-3/klein/base/text-to-image/lora> (2026-11-04)
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=blackforestlabs/flux-3/klein/base/edit-image/lora> (2026-11-04)
    #[tokio::test]
    async fn flux_3_klein_base_on_fal_takes_its_lora_endpoints_with_no_lora() {
        let server = MockServer::start().await;
        let uri = server.uri();
        for at in ["blackforestlabs/flux-3/klein/base/text-to-image/lora", "blackforestlabs/flux-3/klein/base/edit-image/lora"] {
            Mock::given(method("POST")).and(path(format!("/{at}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": format!("{uri}/out.png"), "content_type": "image/png" }] })))
                .mount(&server).await;
        }
        Mock::given(method("GET")).and(path("/out.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"\x89PNG\r\n\x1a\nX".to_vec())).mount(&server).await;
        let p = make_provider(&server.uri());
        let id = "fal/flux-3-image-klein-base";
        let mut e = make_extras();
        e.size = Some("2048x2048".into());
        e.extra = Some(json!({ "enable_prompt_expansion": false }));
        let none = MaterializedRequest { refs: vec![], cleanup: crate::proxy::materializer::Cleanup::empty() };
        p.generate(&ref_schema(id), &make_base("a canyon", id), &e, &none).await.expect("generated");
        let one = MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef { role: "init".into(), form: MaterializedRefForm::Url("https://cdn.example/a.png".into()), time_s: None }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        p.generate(&ref_schema(id), &make_base("recolour it", id), &make_extras_no_size(), &one).await.expect("generated");
        let received = server.received_requests().await.unwrap();
        let body_at = |at: &str| -> Value {
            serde_json::from_slice(&received.iter().find(|r| r.url.path() == at).unwrap_or_else(|| panic!("no call to {at}")).body).unwrap()
        };
        let t2i = body_at("/blackforestlabs/flux-3/klein/base/text-to-image/lora");
        assert_eq!(t2i["image_size"], json!({ "width": 2048, "height": 2048 }), "{t2i}");
        assert_eq!(t2i["enable_prompt_expansion"], json!(false), "{t2i}");
        let edit = body_at("/blackforestlabs/flux-3/klein/base/edit-image/lora");
        assert_eq!(edit["image_urls"], json!(["https://cdn.example/a.png"]), "{edit}");
        for b in [&t2i, &edit] {
            assert!(b.get("loras").is_none() && b.get("image_url").is_none() && b.get("resolution").is_none(), "{b}");
        }
    }

    /// FLUX.2 on fal: every row is a text-to-image endpoint with an `/edit`
    /// twin, which a request with images goes to, its images as one
    /// `image_urls` list. [flex] and the klein base models take
    /// `guidance_scale` and `num_inference_steps`, and the base models a
    /// `negative_prompt`. No row sends `num_images`.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2/edit> (2026-11-04)
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2-pro/edit> (2026-11-04)
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2-flex/edit> (2026-11-04)
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2/klein/9b/base/edit> (2026-11-04)
    #[tokio::test]
    async fn flux_2_on_fal_routes_by_images() {
        let rows = [
            ("fal/flux-2", "fal-ai/flux-2"),
            ("fal/flux-2-turbo", "fal-ai/flux-2/turbo"),
            ("fal/flux-2-flash", "fal-ai/flux-2/flash"),
            ("fal/flux-2-pro", "fal-ai/flux-2-pro"),
            ("fal/flux-2-max", "fal-ai/flux-2-max"),
            ("fal/flux-2-flex", "fal-ai/flux-2-flex"),
            ("fal/flux-2-klein-4b", "fal-ai/flux-2/klein/4b"),
            ("fal/flux-2-klein-9b", "fal-ai/flux-2/klein/9b"),
            ("fal/flux-2-klein-4b-base", "fal-ai/flux-2/klein/4b/base"),
            ("fal/flux-2-klein-9b-base", "fal-ai/flux-2/klein/9b/base"),
        ];
        let server = MockServer::start().await;
        let uri = server.uri();
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": format!("{uri}/out.png"), "content_type": "image/png" }] })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/out.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"\x89PNG\r\n\x1a\nX".to_vec())).mount(&server).await;
        let p = make_provider(&server.uri());
        let two = || MaterializedRequest {
            refs: vec![
                crate::proxy::materializer::MaterializedRef { role: "init".into(), form: MaterializedRefForm::Url("https://cdn.example/a.png".into()), time_s: None },
                crate::proxy::materializer::MaterializedRef { role: "init".into(), form: MaterializedRefForm::Url("https://cdn.example/b.png".into()), time_s: None },
            ],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        let none = || MaterializedRequest { refs: vec![], cleanup: crate::proxy::materializer::Cleanup::empty() };
        for (id, _) in rows {
            let mut e = make_extras();
            e.size = Some("1536x1024".into());
            p.generate(&ref_schema(id), &make_base("a canyon", id), &e, &none()).await.unwrap_or_else(|err| panic!("{id} text: {err:?}"));
            p.generate(&ref_schema(id), &make_base("merge them", id), &make_extras_no_size(), &two()).await.unwrap_or_else(|err| panic!("{id} edit: {err:?}"));
        }
        let received = server.received_requests().await.unwrap();
        let body_at = |at: &str| -> Value {
            serde_json::from_slice(&received.iter().find(|r| r.url.path() == at).unwrap_or_else(|| panic!("no call to {at}")).body).unwrap()
        };
        for (id, endpoint) in rows {
            let t2i = body_at(&format!("/{endpoint}"));
            assert_eq!(t2i["image_size"], json!({ "width": 1536, "height": 1024 }), "{id}: {t2i}");
            assert!(t2i.get("image_urls").is_none() && t2i.get("num_images").is_none(), "{id}: {t2i}");
            let edit = body_at(&format!("/{endpoint}/edit"));
            assert_eq!(edit["image_urls"], json!(["https://cdn.example/a.png", "https://cdn.example/b.png"]), "{id}: {edit}");
            assert!(edit.get("image_url").is_none() && edit.get("num_images").is_none(), "{id}: {edit}");
        }
        // [flex]'s knobs and a base model's negative prompt reach the body.
        let id = "fal/flux-2-flex";
        let mut e = make_extras();
        e.guidance_scale = Some(4.5);
        e.steps = Some(40);
        p.generate(&ref_schema(id), &make_base("a lighthouse", id), &e, &none()).await.expect("flex");
        let id = "fal/flux-2-klein-9b-base";
        let mut base = make_base("a lighthouse", id);
        base.negative_prompt = Some("fog".into());
        p.generate(&ref_schema(id), &base, &make_extras(), &none()).await.expect("klein base");
        let received = server.received_requests().await.unwrap();
        let last = |at: &str| -> Value {
            serde_json::from_slice(&received.iter().rev().find(|r| r.url.path() == at).unwrap_or_else(|| panic!("no call to {at}")).body).unwrap()
        };
        let flex = last("/fal-ai/flux-2-flex");
        assert_eq!((&flex["guidance_scale"], &flex["num_inference_steps"]), (&json!(4.5), &json!(40)), "{flex}");
        assert_eq!(last("/fal-ai/flux-2/klein/9b/base")["negative_prompt"], "fog");
    }

    /// FLUX.2 [pro] outpaint on fal sends the init image, each side's pixels
    /// as `expand_left` … `expand_bottom` ("Pixels to expand on the left of
    /// the image"), `extra`'s `mode`, and no prompt; `extra` cannot change a
    /// side. A request with no side to grow is refused before any call.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2-pro/outpaint> (2026-11-04)
    #[tokio::test]
    async fn flux_2_pro_outpaint_on_fal_sends_the_sides_and_no_prompt() {
        let server = MockServer::start().await;
        let uri = server.uri();
        Mock::given(method("POST")).and(path("/fal-ai/flux-2-pro/outpaint"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": format!("{uri}/out.png"), "content_type": "image/png" }] })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/out.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"\x89PNG\r\n\x1a\nX".to_vec())).mount(&server).await;
        let p = make_provider(&server.uri());
        let id = "fal/flux-2-pro-outpaint";
        let init = || MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef { role: "init".into(), form: MaterializedRefForm::Url("https://cdn.example/a.png".into()), time_s: None }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        let mut e = make_extras_no_size();
        e.expand = Some(crate::types::Expand { left: 256, top: 0, right: 256, bottom: 128 });
        e.output_format = Some("png".into());
        e.extra = Some(json!({ "mode": "fast", "expand_left": 9 }));
        let out = p.generate(&ref_schema(id), &make_base("", id), &e, &init()).await.expect("outpainted");
        assert_eq!(out.content_type, "image/png");
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 2, "the submit, then the download");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(
            body,
            json!({ "image_url": "https://cdn.example/a.png", "expand_left": 256, "expand_top": 0, "expand_right": 256, "expand_bottom": 128, "mode": "fast", "output_format": "png" })
        );
        let mut nothing = make_extras_no_size();
        nothing.expand = None;
        let err = p.generate(&ref_schema(id), &make_base("", id), &nothing, &init()).await.expect_err("no side to grow");
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        assert_eq!(server.received_requests().await.unwrap().len(), 2, "nothing more was sent");
    }

    /// FLUX.1 [pro] on fal:
    /// - Kontext picks its endpoint by image count: none goes to
    ///   `…/text-to-image`, one to the base path as `image_url`, and two or
    ///   more to `…/multi` as `image_urls`, each with `aspect_ratio`.
    /// - Ultra sends its image prompt as `image_url`.
    /// - Fill and erase send the mask as `mask_url`, and erase sends no
    ///   `prompt`.
    /// - Redux sends its one image as `image_url`.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/kontext/multi> (2026-11-04)
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/v1/fill> (2026-11-04)
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/v1/erase> (2026-11-04)
    #[tokio::test]
    async fn flux_1_pro_on_fal_routes_by_images_and_masks() {
        let server = MockServer::start().await;
        let uri = server.uri();
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": format!("{uri}/out.png"), "content_type": "image/png" }] })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/out.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"\x89PNG\r\n\x1a\nX".to_vec())).mount(&server).await;
        let p = make_provider(&server.uri());
        let image = |role: &str, url: &str| crate::proxy::materializer::MaterializedRef { role: role.into(), form: MaterializedRefForm::Url(url.into()), time_s: None };
        let with = |refs: Vec<crate::proxy::materializer::MaterializedRef>| MaterializedRequest { refs, cleanup: crate::proxy::materializer::Cleanup::empty() };
        let mut ratio = make_extras_no_size();
        ratio.aspect_ratio = Some("21:9".into());
        for (id, base) in [("fal/flux-kontext-pro", "fal-ai/flux-pro/kontext"), ("fal/flux-kontext-max", "fal-ai/flux-pro/kontext/max")] {
            p.generate(&ref_schema(id), &make_base("a canyon", id), &ratio, &with(vec![])).await.unwrap_or_else(|e| panic!("{id} text: {e:?}"));
            p.generate(&ref_schema(id), &make_base("make it night", id), &ratio, &with(vec![image("init", "https://cdn.example/a.png")])).await.unwrap_or_else(|e| panic!("{id} one: {e:?}"));
            let two = vec![image("init", "https://cdn.example/a.png"), image("init", "https://cdn.example/b.png")];
            p.generate(&ref_schema(id), &make_base("merge them", id), &ratio, &with(two)).await.unwrap_or_else(|e| panic!("{id} two: {e:?}"));
            let received = server.received_requests().await.unwrap();
            let body_at = |at: String| -> Value { serde_json::from_slice(&received.iter().rev().find(|r| r.url.path() == at).unwrap_or_else(|| panic!("no call to {at}")).body).unwrap() };
            let t2i = body_at(format!("/{base}/text-to-image"));
            assert_eq!(t2i["aspect_ratio"], "21:9", "{id}: {t2i}");
            assert!(t2i.get("image_url").is_none() && t2i.get("image_urls").is_none(), "{id}: {t2i}");
            let one = body_at(format!("/{base}"));
            assert_eq!(one["image_url"], "https://cdn.example/a.png", "{id}: {one}");
            assert!(one.get("image_urls").is_none(), "{id}: {one}");
            let multi = body_at(format!("/{base}/multi"));
            assert_eq!(multi["image_urls"], json!(["https://cdn.example/a.png", "https://cdn.example/b.png"]), "{id}: {multi}");
            assert!(multi.get("image_url").is_none(), "{id}: {multi}");
        }
        let id = "fal/flux-pro-1.1-ultra";
        p.generate(&ref_schema(id), &make_base("a canyon", id), &ratio, &with(vec![image("redux", "https://cdn.example/style.png")])).await.expect("ultra");
        let id = "fal/flux-pro-fill";
        p.generate(&ref_schema(id), &make_base("a red door", id), &make_extras_no_size(), &with(vec![image("init", "https://cdn.example/a.png"), image("mask", "https://cdn.example/m.png")])).await.expect("fill");
        let id = "fal/flux-pro-erase";
        p.generate(&ref_schema(id), &make_base("", id), &make_extras_no_size(), &with(vec![image("mask", "https://cdn.example/m.png"), image("init", "https://cdn.example/a.png")])).await.expect("erase");
        let id = "fal/flux-pro-1.1-redux";
        p.generate(&ref_schema(id), &make_base("", id), &make_extras_no_size(), &with(vec![image("init", "https://cdn.example/a.png")])).await.expect("redux");
        let received = server.received_requests().await.unwrap();
        let body_at = |at: &str| -> Value { serde_json::from_slice(&received.iter().rev().find(|r| r.url.path() == at).unwrap_or_else(|| panic!("no call to {at}")).body).unwrap() };
        let ultra = body_at("/fal-ai/flux-pro/v1.1-ultra");
        assert_eq!((ultra["image_url"].as_str(), ultra["aspect_ratio"].as_str()), (Some("https://cdn.example/style.png"), Some("21:9")), "{ultra}");
        for at in ["/fal-ai/flux-pro/v1/fill", "/fal-ai/flux-pro/v1/erase"] {
            let b = body_at(at);
            assert_eq!((b["image_url"].as_str(), b["mask_url"].as_str()), (Some("https://cdn.example/a.png"), Some("https://cdn.example/m.png")), "{at}: {b}");
            assert!(b.get("image_urls").is_none(), "{at}: {b}");
        }
        assert_eq!(body_at("/fal-ai/flux-pro/v1/fill")["prompt"], "a red door");
        assert!(body_at("/fal-ai/flux-pro/v1/erase").get("prompt").is_none(), "erase takes no prompt");
        assert_eq!(body_at("/fal-ai/flux-pro/v1.1/redux")["image_url"], "https://cdn.example/a.png");
        // Fill and erase need their mask (or a region drawn as one): without
        // it nothing is sent ("mask_url" is required on both).
        let sent = received.len();
        for id in ["fal/flux-pro-fill", "fal/flux-pro-erase"] {
            let err = p.generate(&ref_schema(id), &make_base("", id), &make_extras_no_size(), &with(vec![image("init", "https://cdn.example/a.png")])).await.expect_err(id);
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{id}: {err:?}");
        }
        assert_eq!(server.received_requests().await.unwrap().len(), sent, "nothing more was sent");
    }

    /// Recraft V4 and V4.1 on fal: each row's own endpoint, the size as
    /// `image_size` {width, height}, and a palette as `colors` [{r, g, b}].
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/v4.1/text-to-image> (2026-11-04)
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=recraft/v4.1/flash/text-to-image> (2026-11-04)
    #[tokio::test]
    async fn recraft_v4_on_fal_sends_its_size_and_palette() {
        let rows = [
            ("fal/recraft-v4", "fal-ai/recraft/v4/text-to-image"),
            ("fal/recraft-v4-pro", "fal-ai/recraft/v4/pro/text-to-image"),
            ("fal/recraft-v4-vector", "fal-ai/recraft/v4/text-to-vector"),
            ("fal/recraft-v4-pro-vector", "fal-ai/recraft/v4/pro/text-to-vector"),
            ("fal/recraft-v4.1", "fal-ai/recraft/v4.1/text-to-image"),
            ("fal/recraft-v4.1-pro", "fal-ai/recraft/v4.1/pro/text-to-image"),
            ("fal/recraft-v4.1-vector", "fal-ai/recraft/v4.1/text-to-vector"),
            ("fal/recraft-v4.1-pro-vector", "fal-ai/recraft/v4.1/pro/text-to-vector"),
            ("fal/recraft-v4.1-utility", "fal-ai/recraft/v4.1/utility/text-to-image"),
            ("fal/recraft-v4.1-utility-pro", "fal-ai/recraft/v4.1/utility/pro/text-to-image"),
            ("fal/recraft-v4.1-flash", "recraft/v4.1/flash/text-to-image"),
            // V2 at the registry's listed id; its page redirects to fal-ai/recraft/v2/text-to-image.
            ("fal/recraft-v2", "fal-ai/recraft-20b"),
        ];
        let server = MockServer::start().await;
        let uri = server.uri();
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": format!("{uri}/out.svg"), "content_type": "image/svg+xml" }] })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/out.svg")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec())).mount(&server).await;
        let p = make_provider(&server.uri());
        for (id, _) in rows {
            let mut e = make_extras_no_size();
            e.size = Some(if id.contains("pro") { "2048x2048" } else { "1536x768" }.into());
            e.palette = Some(vec!["#ff6432".into(), "#081820".into()]);
            let out = p.generate(&ref_schema(id), &make_base("a fox logo", id), &e, &MaterializedRequest { refs: vec![], cleanup: crate::proxy::materializer::Cleanup::empty() })
                .await
                .unwrap_or_else(|err| panic!("{id}: {err:?}"));
            assert_eq!(out.content_type, "image/svg+xml", "{id}: fal's own content_type");
        }
        let received = server.received_requests().await.unwrap();
        for (id, endpoint) in rows {
            let body: Value = serde_json::from_slice(&received.iter().find(|r| r.url.path() == format!("/{endpoint}")).unwrap_or_else(|| panic!("no call to {endpoint}")).body).unwrap();
            let (w, h) = if id.contains("pro") { (2048, 2048) } else { (1536, 768) };
            assert_eq!(body["image_size"], json!({ "width": w, "height": h }), "{id}: {body}");
            assert_eq!(body["colors"], json!([{ "r": 255, "g": 100, "b": 50 }, { "r": 8, "g": 24, "b": 32 }]), "{id}: {body}");
            assert_eq!(body["prompt"], "a fox logo", "{id}");
        }
    }

    /// The rest of Recraft on fal:
    /// - V3 with an image goes to `/image-to-image` (`image_url`, `strength`,
    ///   `negative_prompt`), and without one to text-to-image with neither of
    ///   those fields.
    /// - A style row sends its `style` references as `image_urls`.
    /// - Vectorize and the upscalers send only `image_url` and read fal's one
    ///   `image`.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/v3/image-to-image> (2026-11-04)
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=recraft/v4/style/text-to-image> (2026-11-04)
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/vectorize> (2026-11-04)
    #[tokio::test]
    async fn recraft_operations_on_fal_send_their_own_fields() {
        let server = MockServer::start().await;
        let uri = server.uri();
        for at in ["/fal-ai/recraft/vectorize", "/fal-ai/recraft/upscale/crisp", "/fal-ai/recraft/upscale/creative"] {
            Mock::given(method("POST")).and(path(at))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "image": { "url": format!("{uri}/one.svg"), "content_type": "image/svg+xml" } })))
                .mount(&server).await;
        }
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": format!("{uri}/out.png"), "content_type": "image/png" }] })))
            .mount(&server).await;
        for at in ["/out.png", "/one.svg"] {
            Mock::given(method("GET")).and(path(at)).respond_with(ResponseTemplate::new(200).set_body_bytes(b"\x89PNG\r\n\x1a\nX".to_vec())).mount(&server).await;
        }
        let p = make_provider(&server.uri());
        let image = |role: &str, url: &str| crate::proxy::materializer::MaterializedRef { role: role.into(), form: MaterializedRefForm::Url(url.into()), time_s: None };
        let with = |refs: Vec<crate::proxy::materializer::MaterializedRef>| MaterializedRequest { refs, cleanup: crate::proxy::materializer::Cleanup::empty() };
        let mut e = make_extras_no_size();
        e.strength = Some(0.3);
        e.palette = Some(vec!["#ff6432".into()]);
        let mut base = make_base("a fox", "fal/recraft-v3");
        base.negative_prompt = Some("blur".into());
        p.generate(&ref_schema("fal/recraft-v3"), &base, &e, &with(vec![image("init", "https://cdn.example/a.png")])).await.expect("v3 image");
        p.generate(&ref_schema("fal/recraft-v3"), &base, &e, &with(vec![])).await.expect("v3 text");
        let mut styled = make_extras_no_size();
        styled.extra = Some(json!({ "style_match": "precise" }));
        let refs = vec![image("style", "https://cdn.example/s1.png"), image("style", "https://cdn.example/s2.png")];
        p.generate(&ref_schema("fal/recraft-v4-style"), &make_base("a fox", "fal/recraft-v4-style"), &styled, &with(refs)).await.expect("style");
        for id in ["fal/recraft-vectorize", "fal/recraft-upscale-crisp", "fal/recraft-upscale-creative"] {
            let out = p.generate(&ref_schema(id), &make_base("", id), &make_extras_no_size(), &with(vec![image("init", "https://cdn.example/a.png")])).await.unwrap_or_else(|err| panic!("{id}: {err:?}"));
            assert_eq!(out.content_type, "image/svg+xml", "{id}: fal's own content_type");
        }
        let received = server.received_requests().await.unwrap();
        let bodies = |at: &str| -> Vec<Value> { received.iter().filter(|r| r.url.path() == at).map(|r| serde_json::from_slice(&r.body).unwrap()).collect() };
        let i2i = &bodies("/fal-ai/recraft/v3/image-to-image")[0];
        assert_eq!((i2i["image_url"].as_str(), &i2i["strength"], i2i["negative_prompt"].as_str()), (Some("https://cdn.example/a.png"), &json!(0.3), Some("blur")), "{i2i}");
        assert_eq!(i2i["colors"], json!([{ "r": 255, "g": 100, "b": 50 }]), "{i2i}");
        let t2i = &bodies("/fal-ai/recraft/v3/text-to-image")[0];
        assert!(t2i.get("strength").is_none() && t2i.get("negative_prompt").is_none() && t2i.get("image_url").is_none(), "text-to-image takes neither: {t2i}");
        let style = &bodies("/recraft/v4/style/text-to-image")[0];
        assert_eq!(style["image_urls"], json!(["https://cdn.example/s1.png", "https://cdn.example/s2.png"]), "{style}");
        assert_eq!(style["style_match"], "precise", "{style}");
        for at in ["/fal-ai/recraft/vectorize", "/fal-ai/recraft/upscale/crisp", "/fal-ai/recraft/upscale/creative"] {
            assert_eq!(bodies(at), vec![json!({ "image_url": "https://cdn.example/a.png" })], "{at}");
        }
    }

    fn make_extras_no_size() -> ImageExtras {
        let mut e = make_extras();
        e.size = None;
        e
    }

    #[tokio::test]
    async fn generates_flux_dev_image() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;

        let image_url = format!("{}/output.png", image_server.uri());

        // Mock POST /fal-ai/flux/dev
        Mock::given(method("POST"))
            .and(path("/fal-ai/flux/dev"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "images": [{
                    "url": image_url,
                    "content_type": "image/jpeg",
                    "width": 1024,
                    "height": 1024
                }],
                "timings": {},
                "seed": 42
            })))
            .mount(&server)
            .await;

        // Mock GET to image URL
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"FAKEJPEGDATA".to_vec())
                    .insert_header("content-type", "image/jpeg"),
            )
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("fal/flux-dev");
        let base = make_base("a neon city at night", "fal/flux-dev");
        let mut extras = make_extras();
        extras.output_format = Some("png".into());
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let output = result.unwrap();
        assert_eq!(output.data, b"FAKEJPEGDATA");

        // Verify outbound request body
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();

        assert_eq!(body["prompt"], "a neon city at night");
        assert_eq!(body["image_size"]["width"], 1024);
        assert_eq!(body["image_size"]["height"], 1024);
        assert_eq!(body["seed"], 42);
        assert_eq!(body["guidance_scale"], 3.5);
        assert_eq!(body["num_inference_steps"], 28);
        assert_eq!(body["output_format"], "png", "PixelGenie L9: the asked container");
    }

    /// fal renamed `fal-ai/recraft-v3` to `fal-ai/recraft/v3/text-to-image`:
    /// <https://fal.ai/models/fal-ai/recraft-v3> 308-redirects there, and the
    /// model registry (`api.fal.ai/v1/models`) returns the old id without
    /// metadata. Both ids publish the same input schema (`prompt` of at most
    /// 1000 chars, `style`, `image_size`, `colors`, `style_id`), so the catalog
    /// id `fal/recraft-v3` keeps working by moving only the endpoint path.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/v3/text-to-image>
    #[tokio::test]
    async fn recraft_v3_posts_to_the_renamed_endpoint() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/output.webp", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/fal-ai/recraft/v3/text-to-image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "images": [{ "url": image_url, "content_type": "image/webp" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"FAKEWEBP".to_vec()))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("fal/recraft-v3");
        let mut base = make_base("a minimalist fox logo", "fal/recraft-v3");
        base.seed = None;
        // Only what the catalog lets through for this model: prompt + style.
        let extras = ImageExtras {
            vendor_job: Default::default(),
            size: None,
            steps: None,
            guidance_scale: None,
            style: Some("vector_illustration".to_string()),
            ..make_extras()
        };

        let output = provider
            .generate(&schema, &base, &extras, &empty_materialized())
            .await
            .expect("generate");
        assert_eq!(output.data, b"FAKEWEBP");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].url.path(), "/fal-ai/recraft/v3/text-to-image");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["prompt"], "a minimalist fox logo");
        assert_eq!(body["style"], "vector_illustration");
    }

    /// `fal/flux-2` is FLUX.2 [dev] at `fal-ai/flux-2`. Unknown ids fall back
    /// to `fal-ai/flux/dev`, so without its own arm the request silently ran
    /// on FLUX.1 [dev]. Its input takes `guidance_scale`, `num_inference_steps`
    /// and `image_size` like the other FLUX endpoints.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2>
    #[tokio::test]
    async fn flux_2_posts_to_the_flux_2_endpoint() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/output.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/fal-ai/flux-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "images": [{ "url": image_url, "content_type": "image/png" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"FAKEPNG".to_vec()))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("fal/flux-2");
        let base = make_base("a neon city at night", "fal/flux-2");
        let output = provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect("generate");
        assert_eq!(output.data, b"FAKEPNG");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].url.path(), "/fal-ai/flux-2");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["guidance_scale"], 3.5);
        assert_eq!(body["num_inference_steps"], 28);
        assert_eq!(body["image_size"]["width"], 1024);
    }

    // ─── fal audit, 2026-10-30: F04-F09, F11 ────────────────────────────────

    fn configured(api_base: Option<&str>, options: Option<Value>) -> FalProvider {
        let mut p = FalProvider::new();
        p.configure(ProviderInstanceConfig { api_key: "test-key".into(), api_base: api_base.map(str::to_string), options, ..Default::default() });
        p
    }

    /// F09: `providers.fal.api_base` is fal's QUEUE host for video and 3D;
    /// images run on its sync host, which is not that one.
    #[test]
    fn images_run_on_fals_sync_host_beside_its_queue_host() {
        assert_eq!(configured(None, None).sync_base(), "https://fal.run");
        assert_eq!(configured(Some("https://queue.fal.run"), None).sync_base(), "https://fal.run");
        assert_eq!(configured(Some("https://fal-proxy.example/fal"), None).sync_base(), "https://fal-proxy.example/fal");
        let split = configured(Some("https://queue.fal.run"), Some(json!({ "sync_api_base": "https://sync.example/" })));
        assert_eq!(split.sync_base(), "https://sync.example");
    }

    async fn image_answer(server: &MockServer, endpoint: &str, images: &MockServer) {
        Mock::given(method("POST"))
            .and(path(format!("/{endpoint}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": format!("{}/out.png", images.uri()), "content_type": "image/png" }] })))
            .mount(server)
            .await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"PNG".to_vec())).mount(images).await;
    }

    /// F05, F06: what litegen sets, `extra` cannot replace, and `num_images`
    /// is never sent — fal would bill every image it made and litegen keeps
    /// one, and Recraft V3 has no such field.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends_and_num_images_is_never_sent() {
        let (server, images) = (MockServer::start().await, MockServer::start().await);
        image_answer(&server, "fal-ai/flux/dev", &images).await;
        let mut base = make_base("a lighthouse", "fal/flux-dev");
        base.n = 2;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "prompt": "something else", "num_images": 4, "sync_mode": true, "seed": 7, "image_size": "square", "enable_safety_checker": false }));
        make_provider(&server.uri()).generate(&ref_schema("fal/flux-dev"), &base, &extras, &empty_materialized()).await.unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!((body["prompt"].as_str(), body["seed"].as_i64()), (Some("a lighthouse"), Some(42)), "{body}");
        assert_eq!(body["image_size"], json!({ "width": 1024, "height": 1024 }), "{body}");
        assert!(body.get("num_images").is_none() && body.get("sync_mode").is_none(), "{body}");
        assert_eq!(body["enable_safety_checker"], false, "a knob litegen does not own still passes: {body}");
    }

    /// fast-sdxl's container field is `format` ("enum": ["jpeg", "png"],
    /// default "jpeg"), not `output_format`: the asked container goes as it,
    /// and `extra` cannot replace it.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/fast-sdxl> (2026-11-03)
    #[tokio::test]
    async fn fast_sdxl_takes_its_container_as_format() {
        let (server, images) = (MockServer::start().await, MockServer::start().await);
        image_answer(&server, "fal-ai/fast-sdxl", &images).await;
        let mut extras = make_extras();
        extras.output_format = Some("png".into());
        extras.extra = Some(json!({ "format": "jpeg" }));
        make_provider(&server.uri()).generate(&ref_schema("fal/sdxl"), &make_base("a lighthouse", "fal/sdxl"), &extras, &empty_materialized()).await.unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["format"], "png", "{body}");
        assert!(body.get("output_format").is_none(), "fast-sdxl has no output_format: {body}");
    }

    /// F04: once fal answered 2xx it has run, and billed, the job — a result
    /// that will not download is never worth a second, billed run.
    #[tokio::test]
    async fn a_result_that_will_not_download_after_fal_ran_is_never_run_again() {
        let (server, images) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&images).await;
        // A host that answers 503, and one that refuses the connection.
        for url in [format!("{}/out.png", images.uri()), "http://127.0.0.1:1/out.png".to_string()] {
            server.reset().await;
            Mock::given(method("POST"))
                .and(path("/fal-ai/flux/dev"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": url, "content_type": "image/png" }] })))
                .mount(&server)
                .await;
            let err = make_provider(&server.uri())
                .generate(&ref_schema("fal/flux-dev"), &make_base("x", "fal/flux-dev"), &make_extras(), &empty_materialized())
                .await
                .unwrap_err();
            assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{url}: {err:?}");
        }
    }

    /// F08: fal's model errors are an array of `{loc, msg}`, a gateway's page
    /// is not JSON at all, and `X-Fal-Needs-Retry` says whether to ask again.
    #[tokio::test]
    async fn fals_errors_are_read_whole_whatever_their_shape() {
        for (answer, says, retryable) in [
            (ResponseTemplate::new(422).set_body_json(json!({ "detail": [{ "loc": ["body", "image_size"], "msg": "unsupported size", "type": "value_error" }] })), "body.image_size: unsupported size", false),
            (ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"), "Bad Gateway", true),
            (ResponseTemplate::new(429).set_body_json(json!({ "detail": "Too many concurrent requests", "error_type": "rate_limited" })), "Too many concurrent requests", true),
            (ResponseTemplate::new(500).insert_header("x-fal-needs-retry", "0").set_body_json(json!({ "detail": "the model failed" })), "the model failed", false),
            (ResponseTemplate::new(400).insert_header("x-fal-needs-retry", "1").set_body_json(json!({ "detail": "try again" })), "try again", true),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).and(path("/fal-ai/flux/dev")).respond_with(answer).mount(&server).await;
            let err = make_provider(&server.uri())
                .generate(&ref_schema("fal/flux-dev"), &make_base("x", "fal/flux-dev"), &make_extras(), &empty_materialized())
                .await
                .unwrap_err();
            match &err {
                ProviderError::RequestFailed { message, retryable: r, .. } => {
                    assert!(message.contains(says), "{says}: {message}");
                    assert_eq!(*r, retryable, "{says}: {err:?}");
                }
                other => panic!("{other:?}"),
            }
        }
    }

    /// F11: a result URL is fetched only from a public address, and never
    /// past the size limit.
    #[tokio::test]
    async fn a_result_is_fetched_only_from_a_public_address() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fal-ai/flux/dev"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": "http://169.254.169.254/latest/meta-data", "content_type": "image/png" }] })))
            .mount(&server)
            .await;
        let mut p = make_provider(&server.uri());
        p.public_assets_only = true;
        let err = p
            .generate(&ref_schema("fal/flux-dev"), &make_base("x", "fal/flux-dev"), &make_extras(), &empty_materialized())
            .await
            .unwrap_err();
        assert!(matches!(&err, ProviderError::RequestFailed { message, retryable: false, .. } if message.contains("refused")), "{err:?}");
    }

    /// F07: the key is checked against fal's Platform API; the old
    /// `rest.alpha.fal.ai/tokens/current` route is gone (404 for every key).
    #[tokio::test]
    async fn health_check_validates_the_key_against_the_platform_api() {
        use wiremock::matchers::{header, query_param};
        for (status, healthy) in [(200, true), (401, false)] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/models/pricing"))
                .and(query_param("endpoint_id", "fal-ai/trellis"))
                .and(header("authorization", "Key test-key"))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({})))
                .expect(1)
                .mount(&server)
                .await;
            let p = configured(None, Some(json!({ "platform_api_base": server.uri() })));
            assert_eq!(p.health_check().await.healthy, healthy, "{status}");
        }
    }
}
