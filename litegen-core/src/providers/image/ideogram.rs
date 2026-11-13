use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// The Ideogram generation endpoint a catalog id is served by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Generation {
    /// `POST /v1/ideogram-v3/generate`: `prompt`, `aspect_ratio`, `seed`,
    /// `style_type`, reference images; JSON or multipart.
    V3,
    /// `POST /v1/ideogram-v4/generate`: `text_prompt`, `resolution`,
    /// `rendering_speed`; multipart/form-data only.
    V4,
    /// `POST /v2/image/precise-edit/ideogram-4-5`: `image`, `mask`,
    /// `context_window`, `prompt`, `quality`, `seed`; multipart/form-data.
    /// Synchronous by default, answering the same `data[]` shape.
    ///
    /// @see <https://developer.ideogram.ai/v2/api-reference/images/precise-edit/ideogram-4-5.md>
    ///   Verbatim: "`y_min,x_min,y_max,x_max` names the region explicitly, by its
    ///   corners. Rows come first" and "The maximum edges are exclusive".
    PreciseEdit45,
    /// `POST /v2/image/generate/ideogram-4-5` (multipart): `prompt`, `images`
    /// (the edited one first), `mask`, `size`, `background`, `quality`, `seed`.
    /// @see <https://developer.ideogram.ai/v2/api-reference/images/generate/ideogram-4-5.md> (2026-11-03)
    Generate45,
    /// `POST /v2/image/generate/{slug}`: a model Ideogram resells (Nano Banana),
    /// multipart: `prompt`, `images`, `aspect_ratio`, `resolution_tier`, `seed`,
    /// `num_images`; synchronous, the same `data[]` answer.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-03)
    V2Resold(&'static str),
    /// `POST /v2/image/generate/p-image-ideogram` (JSON): `prompt`, `quality`
    /// ("Defaults to `medium`"), `resolution` "1k" | "2k" ("Defaults to
    /// `1k`"), `aspect_ratio` as "16x9", `seed`, `num_images`.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-04)
    V2PImage,
}

/// Ideogram 3.0 and 4.0 image generation provider.
///
/// Synchronous: `POST /v1/ideogram-v3/generate` returns `{created, data: [{url,
/// seed, resolution, is_image_safe, style_type}]}`. Auth via the `Api-Key`
/// header (NOT Bearer). Reference images (style/character) are multipart file
/// uploads, so requests with refs use `multipart/form-data`; prompt-only
/// requests use JSON. Image-only. Result URLs expire, so bytes are fetched
/// immediately.
///
/// The `ideogram/ideogram-v4*` ids go to `POST /v1/ideogram-v4/generate`
/// instead (see [`Generation::V4`]): a different request contract with the
/// same response shape.
///
/// @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (2026-10-30) — the v1 spec
/// @see <https://developer.ideogram.ai/v1/api-reference/generate-images/generate-v3> (2026-10-30)
///   Verbatim (the page's Python sample, a JSON body): `json={ "prompt": "A picture of a cat",
///   "rendering_speed": "TURBO" }`
/// @see <https://developer.ideogram.ai/v1/api-reference/generate-images/generate-v4.md> (2026-10-30)
pub struct IdeogramProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
    /// Fetches the result: no redirects, no key, and only from public
    /// addresses, judged again at connect.
    asset_client: Client,
    /// Fetches a result on the configured API host itself, which the operator
    /// chose (it may be an internal relay), so it isn't held to public
    /// addresses.
    own_asset_client: Client,
    /// Results only from public addresses; tests serve them from loopback.
    pub(crate) public_assets_only: bool,
}

impl IdeogramProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(180), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::raw_header("Api-Key"),
            client: Self::api_client(&Default::default()),
            asset_client: crate::util::ssrf::public_only_download_client(
                std::time::Duration::from_secs(120),
                std::time::Duration::from_secs(30),
            ),
            own_asset_client: crate::util::ssrf::no_redirect_download_client(
                std::time::Duration::from_secs(120),
                std::time::Duration::from_secs(30),
            ),
            public_assets_only: true,
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.ideogram.ai")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("ideogram".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// Map model id → optional rendering_speed (TURBO/QUALITY/DEFAULT).
    ///
    /// 4.0 has the same tiers as 3.0 (`RenderingSpeed` enum). Its `FLASH` is
    /// "coming soon": "requests with `rendering_speed=FLASH` currently return
    /// a 400", so no catalog id maps to it.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (POST /v1/ideogram-v4/generate)
    fn rendering_speed(model_id: &str) -> Option<&'static str> {
        match model_id.strip_prefix("ideogram/").unwrap_or(model_id) {
            "ideogram-v3-turbo" | "ideogram-v4-turbo" => Some("TURBO"),
            "ideogram-v3-quality" | "ideogram-v4-quality" => Some("QUALITY"),
            _ => None,
        }
    }

    /// Which generation endpoint serves a catalog id. Only the three 4.0 ids
    /// route to v4; everything else keeps the 3.0 request unchanged.
    fn generation(model_id: &str) -> Option<Generation> {
        Some(match model_id.strip_prefix("ideogram/").unwrap_or(model_id) {
            "ideogram-v3" | "ideogram-v3-turbo" | "ideogram-v3-quality" => Generation::V3,
            "ideogram-v4" | "ideogram-v4-turbo" | "ideogram-v4-quality" => Generation::V4,
            "ideogram-4-5-precise-edit" => Generation::PreciseEdit45,
            "ideogram-4-5" => Generation::Generate45,
            // Resold on v2 ("/v2/image/generate/nano-banana-2", ".../nano-banana-pro").
            "nano-banana-2" => Generation::V2Resold("nano-banana-2"),
            "nano-banana-pro" => Generation::V2Resold("nano-banana-pro"),
            // P-Image, Ideogram's budget tier, on v2 with a JSON body.
            "p-image-ideogram" => Generation::V2PImage,
            // An id with no arm is refused, not run as Ideogram 3.0.
            _ => return None,
        })
    }

    /// What litegen itself sends to `POST /v1/ideogram-v3/generate`; `extra`
    /// cannot replace it.
    const V3_OWNED_FIELDS: &'static [&'static str] = &[
        "prompt", "aspect_ratio", "resolution", "rendering_speed", "negative_prompt", "num_images", "seed", "style_type",
        "style_reference_images", "character_reference_images", "character_reference_images_mask",
    ];

    /// A 3.0 request's extras: only the row's `extra_allowlist`, and never a
    /// field litegen sends (`rendering_speed` QUALITY on a TURBO row would
    /// bill another tier).
    fn v3_extras<'a>(model: &ModelSchema, extras: &'a ImageExtras) -> Vec<(&'a String, &'a Value)> {
        let owned = |k: &str| !model.extra_allowlist.iter().any(|a| a == k) || Self::V3_OWNED_FIELDS.contains(&k);
        crate::providers::extra_fields(extras.extra.as_ref(), owned).collect()
    }

    /// The text fields of a `POST /v1/ideogram-v4/generate` request.
    ///
    /// The spec declares only a `multipart/form-data` body, with `text_prompt`
    /// (mutually exclusive with the structured `json_prompt`), `resolution`
    /// (`ResolutionV4`, "WxH"), `rendering_speed` and
    /// `enable_copyright_detection`. There is no `prompt`, `aspect_ratio`,
    /// `seed`, `negative_prompt`, `num_images`, `style_type` or reference
    /// image field, so none is sent. Extras are limited to the model's
    /// `extra_allowlist` (the validator only filters them in strict mode), and
    /// multipart carries scalars as text (`enable_copyright_detection` is a
    /// boolean).
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (POST /v1/ideogram-v4/generate)
    fn v4_fields(
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
    ) -> Vec<(String, String)> {
        let mut fields = vec![("text_prompt".to_string(), base.prompt.clone())];
        if let Some(size) = extras.size.as_deref() {
            // The validator accepts `WxH` or `WXH`; ResolutionV4 values are lowercase.
            fields.push(("resolution".to_string(), size.replace('X', "x")));
        }
        if let Some(s) = Self::rendering_speed(&model.id) {
            fields.push(("rendering_speed".to_string(), s.to_string()));
        }
        for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| !model.extra_allowlist.iter().any(|a| a == k)) {
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Bool(_) | Value::Number(_) => v.to_string(),
                _ => continue,
            };
            fields.push((k.clone(), text));
        }
        fields
    }

    /// Send a generate request and download the first image. Both endpoints
    /// answer `{created, data: [{url, seed, resolution, is_image_safe, …}]}`;
    /// `url` is null when `is_image_safe` is false ("If false, the url field
    /// will be empty"), and a 422 carries `{"error": …}` for a prompt that
    /// failed the safety check.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (ImageGenerationObjectV4, GenerateImageSafetyError)
    async fn send(
        &self,
        builder: reqwest::RequestBuilder,
        creds: &ProviderCredentials,
    ) -> Result<GenerationOutput, ProviderError> {
        let builder = crate::providers::auth::apply(&self.auth, creds, builder)?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Ideogram request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // The generation is the call: one that timed out may have run.
                retryable: crate::providers::may_resend_after(&e, true),
            })?;
        let data = crate::providers::read_vendor_json(resp, "Ideogram").await?;

        let first = &data["data"][0];
        let image_url = match first["url"].as_str() {
            Some(url) if !url.is_empty() => url,
            _ => {
                let message = if first["is_image_safe"] == Value::Bool(false) {
                    "Ideogram withheld the image: data[0].is_image_safe is false".to_string()
                } else {
                    "Ideogram response missing data[0].url".to_string()
                };
                return Err(ProviderError::RequestFailed {
                    message,
                    status_code: None,
                    provider_error: Some(data.clone()),
                    retryable: false,
                });
            }
        };

        // Billed already: a result that will not download is a failure, never
        // a reason for the router to generate again. A result on the
        // configured API host is that host's own; any other must be public.
        let own = crate::providers::model3d::fal::same_origin_url(self.api_base(), Some(image_url)).is_some();
        let client = if own { &self.own_asset_client } else { &self.asset_client };
        let bytes = super::fetch_result(client, image_url, self.public_assets_only && !own, "ideogram image").await?;
        let mut metadata = HashMap::new();
        metadata.insert("url".to_string(), Value::String(image_url.to_string()));
        if let Some(seed) = first["seed"].as_i64() {
            metadata.insert("seed".to_string(), Value::Number(seed.into()));
        }

        Ok(GenerationOutput {
            data: bytes,
            content_type: "image/png".to_string(),
            metadata,
            more: Vec::new(),
        })
    }

    /// Ideogram uses `WxH` aspect-ratio tokens (e.g. "16x9"), not "16:9".
    /// What litegen itself sends to Ideogram 4.5 generate; `extra` cannot
    /// replace it. `num_images` is never sent (one image is billed), nor
    /// `async` (litegen waits) or `webhook_url` (litegen fires its own).
    const GENERATE_45_OWNED: &'static [&'static str] =
        &["prompt", "images", "mask", "size", "background", "quality", "seed", "num_images", "async", "webhook_url"];

    /// Ideogram 4.5 generate: one multipart request, answered when the image
    /// is ready (`async` false, the default), as Precise Edit is.
    /// @see <https://developer.ideogram.ai/v2/api-reference/images/generate/ideogram-4-5.md> (2026-11-03)
    async fn generate_4_5(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        creds: &ProviderCredentials,
    ) -> Result<GenerationOutput, ProviderError> {
        let field = |r: &crate::proxy::materializer::MaterializedRef| match &r.form {
            MaterializedRefForm::MultipartField { field_name, .. } => Some(field_name.clone()),
            _ => None,
        };
        let images = materialized.refs.iter().filter(|r| field(r).as_deref() == Some("images")).count();
        let masked = materialized.refs.iter().any(|r| field(r).as_deref() == Some("mask"));
        let refuse = |why: &str| Err(ProviderError::InvalidRequest(format!("'{}': {why}", model.id)));
        // "`very_low`, the fastest and cheapest, requires source images."
        if extras.quality.as_deref() == Some("very_low") && images == 0 {
            return refuse("quality very_low needs a source image");
        }
        if masked {
            if images == 0 {
                return refuse("a mask edits a source image: send one with role init");
            }
            // "With a mask, the output is always the first source image's own
            // size, so `size` cannot be set."
            if extras.size.is_some() {
                return refuse("with a mask the output is the source image's size; send no size");
            }
            // "A masked request can include at most three other source images."
            if images > 4 {
                return refuse("takes at most 3 other images beside a mask");
            }
        }
        let url = format!("{}/v2/image/generate/ideogram-4-5", self.api_base());
        let mut form = Form::new().text("prompt", base.prompt.clone());
        if let Some(size) = extras.size.as_deref() {
            form = form.text("size", size.to_string());
        }
        if let Some(q) = extras.quality.as_deref() {
            form = form.text("quality", q.to_string());
        }
        if let Some(b) = extras.background.as_deref() {
            form = form.text("background", b.to_string());
        }
        if let Some(seed) = base.seed {
            form = form.text("seed", seed.to_string());
        }
        let owned = |k: &str| !model.extra_allowlist.iter().any(|a| a == k) || Self::GENERATE_45_OWNED.contains(&k);
        for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
            let text = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            form = form.text(k.clone(), text);
        }
        // "The first image is the one being edited; any others are extra
        // references": the `init` first, then each reference, then the mask.
        let order = |r: &&crate::proxy::materializer::MaterializedRef| match r.role.as_str() {
            "init" => 0,
            "mask" => 2,
            _ => 1,
        };
        let mut refs: Vec<_> = materialized.refs.iter().collect();
        refs.sort_by_key(order);
        for r in refs {
            if let MaterializedRefForm::MultipartField { field_name, bytes, content_type } = &r.form {
                let part = Part::bytes(bytes.to_vec())
                    .file_name(format!("{field_name}.png"))
                    .mime_str(content_type)
                    .map_err(|e| ProviderError::InvalidRequest(format!("ideogram 4.5: {e}")))?;
                form = form.part(field_name.clone(), part);
            }
        }
        self.send(self.client.post(&url).multipart(form), creds).await
    }

    /// A model Ideogram resells on v2: litegen's prompt, ratio, tier and seed as
    /// its own fields, the images in order (the edited one first), one image.
    async fn generate_v2_resold(
        &self,
        slug: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        creds: &ProviderCredentials,
    ) -> Result<GenerationOutput, ProviderError> {
        let url = format!("{}/v2/image/generate/{slug}", self.api_base());
        // One image: litegen returns and bills one, whatever `n` was.
        let mut form = Form::new().text("prompt", base.prompt.clone()).text("num_images", "1");
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            form = form.text("aspect_ratio", ar.to_string());
        }
        // "resolution_tier … The output resolution tier." (1K, 2K, 4K: litegen's own spelling).
        form = form.text("resolution_tier", extras.resolution.clone().unwrap_or_else(|| "1K".into()));
        if let Some(seed) = base.seed {
            form = form.text("seed", seed.to_string());
        }
        let mut refs: Vec<_> = materialized.refs.iter().collect();
        refs.sort_by_key(|r| u8::from(r.role != "init"));
        for r in refs {
            if let MaterializedRefForm::MultipartField { field_name, bytes, content_type } = &r.form {
                let part = Part::bytes(bytes.to_vec())
                    .file_name(format!("{field_name}.png"))
                    .mime_str(content_type)
                    .map_err(|e| ProviderError::InvalidRequest(format!("ideogram {slug}: {e}")))?;
                form = form.part(field_name.clone(), part);
            }
        }
        self.send(self.client.post(&url).multipart(form), creds).await
    }

    /// P-Image (`POST /v2/image/generate/p-image-ideogram`, JSON): the prompt,
    /// one image, the asked `quality` or the row's medium, the tier as
    /// Ideogram's "1k" | "2k" (litegen's 1K | 2K), the ratio as "16x9", and the
    /// seed. `extra` adds `magic_prompt` and `enable_copyright_detection` but
    /// can't replace these.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-04)
    async fn generate_p_image(&self, base: &BaseGenerationRequest, extras: &ImageExtras, creds: &ProviderCredentials) -> Result<GenerationOutput, ProviderError> {
        let url = format!("{}/v2/image/generate/p-image-ideogram", self.api_base());
        let mut body = serde_json::Map::new();
        let owned = |k: &str| {
            matches!(k, "prompt" | "num_images" | "quality" | "resolution" | "aspect_ratio" | "seed" | "async" | "webhook_url" | "custom_width" | "custom_height")
        };
        for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
            body.insert(k.clone(), v.clone());
        }
        body.insert("prompt".into(), Value::String(base.prompt.clone()));
        // One image: litegen returns and bills one, whatever `n` was.
        body.insert("num_images".into(), json!(1));
        body.insert("quality".into(), Value::String(extras.quality.clone().unwrap_or_else(|| "medium".into())));
        body.insert("resolution".into(), Value::String(extras.resolution.as_deref().unwrap_or("1K").to_lowercase()));
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body.insert("aspect_ratio".into(), Value::String(Self::ideogram_aspect(ar)));
        }
        if let Some(seed) = base.seed {
            body.insert("seed".into(), json!(seed));
        }
        self.send(self.client.post(&url).json(&Value::Object(body)), creds).await
    }

    fn ideogram_aspect(ar: &str) -> String {
        ar.replace(':', "x")
    }

}

impl Default for IdeogramProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for IdeogramProvider {
    fn name(&self) -> &str {
        "ideogram"
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
            .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.key_pool.is_some())
    }

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let generation = Self::generation(&model.id)
            .ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "ideogram".into() })?;
        if generation == Generation::PreciseEdit45 {
            let url = format!("{}/v2/image/precise-edit/ideogram-4-5", self.api_base());
            let mut form = Form::new().text("prompt", base.prompt.clone());
            // The region, as Ideogram's own box: rows first, ends exclusive.
            if let Some(w) = extras.edit.as_ref().and_then(|e| e.window) {
                form = form.text("context_window", format!("{},{},{},{}", w.y, w.x, w.y + w.h, w.x + w.w));
            }
            if let Some(q) = extras.quality.as_deref() {
                form = form.text("quality", q.to_string());
            }
            if let Some(seed) = base.seed {
                form = form.text("seed", seed.to_string());
            }
            // "A request with a `mask` can include at most three [reference
            // images], because the mask takes up one reference slot" — a mask
            // litegen drew from a region counts, so this is checked here.
            // @see https://developer.ideogram.ai/v2/api-reference/images/precise-edit/ideogram-4-5.md (2026-11-03)
            let field_count = |name: &str| {
                materialized.refs.iter().filter(|r| matches!(&r.form, MaterializedRefForm::MultipartField { field_name, .. } if field_name == name)).count()
            };
            if field_count("mask") > 0 && field_count("reference_images") > 3 {
                return Err(ProviderError::InvalidRequest(format!(
                    "'{}' takes at most 3 reference images beside a mask or region (the mask takes the fourth slot)", model.id
                )));
            }
            // `image`, `mask` (already black-edits: litegen converted it) and
            // each `reference_images` part.
            let mut has_image = false;
            for r in &materialized.refs {
                if let MaterializedRefForm::MultipartField { field_name, bytes, content_type } = &r.form {
                    has_image |= field_name == "image";
                    let part = Part::bytes(bytes.to_vec())
                        .file_name(format!("{field_name}.png"))
                        .mime_str(content_type)
                        .map_err(|e| ProviderError::InvalidRequest(format!("ideogram precise edit: {e}")))?;
                    form = form.part(field_name.clone(), part);
                }
            }
            if !has_image {
                return Err(ProviderError::InvalidRequest(format!("'{}' edits an image: send one with role init", model.id)));
            }
            return self.send(self.client.post(&url).multipart(form), &creds).await;
        }
        if generation == Generation::Generate45 {
            return self.generate_4_5(model, base, extras, materialized, &creds).await;
        }
        if let Generation::V2Resold(slug) = generation {
            return self.generate_v2_resold(slug, base, extras, materialized, &creds).await;
        }
        if generation == Generation::V2PImage {
            return self.generate_p_image(base, extras, &creds).await;
        }
        if generation == Generation::V4 {
            let url = format!("{}/v1/ideogram-v4/generate", self.api_base());
            let mut form = Form::new();
            for (k, v) in Self::v4_fields(model, base, extras) {
                form = form.text(k, v);
            }
            return self.send(self.client.post(&url).multipart(form), &creds).await;
        }

        let url = format!("{}/v1/ideogram-v3/generate", self.api_base());
        let speed = Self::rendering_speed(&model.id);

        // Collect the common string fields once (shared by JSON + multipart).
        let mut fields: Vec<(&str, String)> = vec![("prompt", base.prompt.clone())];
        // "aspect_ratio … Cannot be combined with resolution": an exact size
        // (`resolution`, the ResolutionV3 enum) wins, and the ratio it already
        // fixes is not sent beside it.
        // @see https://developer.ideogram.ai/openapi/ideogram-api-v1.json (2026-11-03)
        if let Some(size) = extras.size.as_deref() {
            fields.push(("resolution", size.to_string()));
        } else if let Some(ar) = extras.aspect_ratio.as_deref() {
            fields.push(("aspect_ratio", Self::ideogram_aspect(ar)));
        }
        if let Some(s) = speed {
            fields.push(("rendering_speed", s.to_string()));
        }
        if let Some(np) = base.negative_prompt.as_deref() {
            fields.push(("negative_prompt", np.to_string()));
        }
        if base.n > 1 {
            fields.push(("num_images", base.n.to_string()));
        }
        if let Some(seed) = base.seed {
            fields.push(("seed", seed.to_string()));
        }
        if let Some(style) = extras.style.as_deref() {
            fields.push(("style_type", style.to_string()));
        }

        let has_refs = materialized
            .refs
            .iter()
            .any(|r| matches!(r.form, MaterializedRefForm::MultipartField { .. }));

        let base_builder = self.client.post(&url);
        let builder = if has_refs {
            // multipart/form-data: text fields + reference-image file parts.
            let mut form = Form::new();
            for (k, v) in &fields {
                form = form.text(k.to_string(), v.clone());
            }
            // Extras as text parts. The spec does not say how multipart
            // carries a list or an object: `style_codes` ("A list of
            // 8-character hexadecimal codes") goes as repeated parts,
            // `color_palette` as JSON text.
            for (k, v) in Self::v3_extras(model, extras) {
                match v {
                    Value::String(text) => form = form.text(k.clone(), text.clone()),
                    Value::Array(items) => {
                        for item in items {
                            form = form.text(k.clone(), item.as_str().map(str::to_string).unwrap_or_else(|| item.to_string()));
                        }
                    }
                    other => form = form.text(k.clone(), other.to_string()),
                }
            }
            for r in &materialized.refs {
                if let MaterializedRefForm::MultipartField { field_name, bytes, content_type } = &r.form {
                    let part = Part::bytes(bytes.to_vec())
                        .file_name("ref")
                        .mime_str(content_type)
                        .map_err(|e| ProviderError::InvalidRequest(format!("ideogram ref mime: {e}")))?;
                    form = form.part(field_name.clone(), part);
                }
            }
            base_builder.multipart(form)
        } else {
            // JSON body for prompt-only requests.
            let mut body = serde_json::Map::new();
            for (k, v) in &fields {
                // num_images/seed are numeric in JSON.
                let val = match *k {
                    "num_images" | "seed" => v.parse::<i64>().map(Value::from).unwrap_or(Value::String(v.clone())),
                    _ => Value::String(v.clone()),
                };
                body.insert(k.to_string(), val);
            }
            for (k, v) in Self::v3_extras(model, extras) {
                body.insert(k.clone(), v.clone());
            }
            base_builder.header("Content-Type", "application/json").json(&Value::Object(body))
        };

        self.send(builder, &creds).await
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

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Ideogram provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult {
            healthy: true,
            message: "Ideogram provider configured".into(),
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> IdeogramProvider {
        let mut p = IdeogramProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: " id-key".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("id-key".to_string());
        p.configure(cfg);
        p.public_assets_only = false;
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(),
            model: model.to_string(),
            n: 1,
            negative_prompt: None,
            seed: None,
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: None,
            resolution: None, aspect_ratio: Some("16:9".to_string()),
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
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

    /// S6 I1: Ideogram bills the generation once it answers, so a result
    /// that will not download is never asked for by a second generation.
    #[tokio::test]
    async fn a_result_that_will_not_download_is_never_generated_again() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/ideogram-v3/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "url": "http://127.0.0.1:1/gone.png", "seed": 1, "is_image_safe": true }]
            })))
            .mount(&server)
            .await;
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
        let err = make_provider(&server.uri())
            .generate(&ref_schema("ideogram/ideogram-v3"), &make_base("x", "ideogram/ideogram-v3"), &make_extras(), &materialized)
            .await
            .unwrap_err();
        assert!(!err.is_retryable(), "{err:?}");
    }

    /// A gateway's 5xx page and a 429 turned the call away, judged by their
    /// status before any parse; a 400 is Ideogram's refusal.
    #[tokio::test]
    async fn a_gateway_page_or_a_rate_limit_may_be_sent_again() {
        for (status, body, retryable) in [(503, "<html>bad gateway</html>", true), (429, "slow down", true), (400, "{\"error\":\"bad\"}", false)] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).respond_with(ResponseTemplate::new(status).set_body_string(body)).mount(&server).await;
            let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
            let err = make_provider(&server.uri())
                .generate(&ref_schema("ideogram/ideogram-v3"), &make_base("x", "ideogram/ideogram-v3"), &make_extras(), &materialized)
                .await
                .unwrap_err();
            assert!(
                matches!(&err, ProviderError::RequestFailed { retryable: r, status_code: Some(s), .. } if *r == retryable && *s == status),
                "{status}: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn generates_ideogram_v3_json() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/img.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/v1/ideogram-v3/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": "2026-05-30T00:00:00Z",
                "data": [{ "url": image_url.clone(), "seed": 42, "is_image_safe": true, "resolution": "1344x768" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"IDEOGRAMPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("ideogram/ideogram-v3");
        let base = make_base("a vintage travel poster of Mars", "ideogram/ideogram-v3");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"IDEOGRAMPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("api-key").unwrap(), "id-key");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["prompt"], "a vintage travel poster of Mars");
        assert_eq!(body["aspect_ratio"], "16x9"); // ':' -> 'x'
    }

    // ─── Ideogram 4.0: POST /v1/ideogram-v4/generate ────────────────────────
    // @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (post_generate_image_v4)

    /// Extract a simple text field's value out of a multipart/form-data body.
    fn multipart_field(body: &[u8], name: &str) -> Option<String> {
        let s = String::from_utf8_lossy(body).into_owned();
        let marker = format!("name=\"{name}\"");
        let at = s.find(&marker)? + marker.len();
        let rest = &s[at..];
        let value_at = rest.find("\r\n\r\n")? + 4;
        let value = &rest[value_at..];
        let end = value.find("\r\n")?;
        Some(value[..end].to_string())
    }

    fn v4_extras(size: Option<&str>) -> ImageExtras {
        ImageExtras { size: size.map(str::to_string), resolution: None, aspect_ratio: None, ..make_extras() }
    }

    /// Start an image host serving b"V4PNG", then mount the v4 generate
    /// response built from that host's image URL. Returns (api, image host).
    async fn mock_v4(respond: impl FnOnce(String) -> ResponseTemplate) -> (MockServer, MockServer) {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"V4PNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/ideogram-v4/generate"))
            .respond_with(respond(format!("{}/v4.png", image_server.uri())))
            .mount(&server)
            .await;
        (server, image_server)
    }

    fn v4_ok(image_url: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "response_type": "url",
            "created": "2026-09-11T00:00:00Z",
            "data": [{ "url": image_url, "prompt": "a cat, magic-prompted", "resolution": "1280x720", "is_image_safe": true, "seed": 7 }]
        }))
    }

    fn empty_materialized() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    #[test]
    fn only_the_v4_ids_route_to_the_v4_endpoint() {
        for id in ["ideogram/ideogram-v4", "ideogram/ideogram-v4-turbo", "ideogram/ideogram-v4-quality"] {
            assert_eq!(IdeogramProvider::generation(id), Some(Generation::V4), "{id}");
        }
        for id in ["ideogram/ideogram-v3", "ideogram/ideogram-v3-turbo", "ideogram/ideogram-v3-quality"] {
            assert_eq!(IdeogramProvider::generation(id), Some(Generation::V3), "{id}");
        }
    }

    /// Text-only submit: multipart to the v4 path with `text_prompt` (not
    /// `prompt`), `resolution` from `size`, the Api-Key header, and none of the
    /// v3-only fields. The default tier sends no `rendering_speed`.
    #[tokio::test]
    async fn v4_submits_multipart_text_prompt_to_the_v4_path() {
        let (server, _img) = mock_v4(|url| v4_ok(&url)).await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("ideogram/ideogram-v4");
        let base = make_base("a neon sign that says OPEN", "ideogram/ideogram-v4");
        let out = provider
            .generate(&schema, &base, &v4_extras(Some("1280x720")), &empty_materialized())
            .await
            .expect("generate");
        assert_eq!(out.data, b"V4PNG");
        assert_eq!(out.metadata.get("seed"), Some(&json!(7)));

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let post = &received[0];
        assert_eq!(post.url.path(), "/v1/ideogram-v4/generate");
        assert_eq!(post.headers.get("api-key").unwrap(), "id-key");
        let ct = post.headers.get("content-type").unwrap().to_str().unwrap();
        assert!(ct.starts_with("multipart/form-data"), "content-type {ct}");
        assert_eq!(multipart_field(&post.body, "text_prompt").as_deref(), Some("a neon sign that says OPEN"));
        assert_eq!(multipart_field(&post.body, "resolution").as_deref(), Some("1280x720"));
        for absent in ["prompt", "rendering_speed", "aspect_ratio", "num_images", "seed", "style_type", "negative_prompt"] {
            assert_eq!(multipart_field(&post.body, absent), None, "{absent} must not be sent to v4");
        }
    }

    /// The -turbo / -quality rows send their tier; allowlisted extras pass as
    /// text, and keys outside the allowlist are not forwarded.
    #[tokio::test]
    async fn v4_tiers_send_rendering_speed_and_allowlisted_extras() {
        for (id, speed) in [("ideogram/ideogram-v4-turbo", "TURBO"), ("ideogram/ideogram-v4-quality", "QUALITY")] {
            let (server, _img) = mock_v4(|url| v4_ok(&url)).await;

            let provider = make_provider(&server.uri());
            let schema = ref_schema(id);
            let mut extras = v4_extras(None);
            extras.extra = Some(json!({ "enable_copyright_detection": true, "magic_prompt": "OFF" }));
            provider
                .generate(&schema, &make_base("a cat", id), &extras, &empty_materialized())
                .await
                .expect("generate");

            let body = &server.received_requests().await.unwrap()[0].body;
            assert_eq!(multipart_field(body, "rendering_speed").as_deref(), Some(speed), "{id}");
            assert_eq!(multipart_field(body, "enable_copyright_detection").as_deref(), Some("true"), "{id}");
            assert_eq!(multipart_field(body, "magic_prompt"), None, "{id}: not in the v4 allowlist");
            assert_eq!(multipart_field(body, "resolution"), None, "{id}: no size, no resolution");
        }
    }

    /// Vendor failures surface as errors carrying the vendor's message: a 400
    /// (e.g. FLASH on v4) and the 422 safety error `{"error": …}`.
    #[tokio::test]
    async fn v4_vendor_errors_carry_the_vendor_message() {
        for (status, body) in [
            (400, json!({ "detail": "rendering_speed=FLASH is not supported for Ideogram V4" })),
            (422, json!({ "error": "Prompt failed the safety check" })),
        ] {
            let (server, _img) = mock_v4(|_| ResponseTemplate::new(status).set_body_json(body.clone())).await;
            let provider = make_provider(&server.uri());
            let err = provider
                .generate(&ref_schema("ideogram/ideogram-v4"), &make_base("x", "ideogram/ideogram-v4"), &v4_extras(None), &empty_materialized())
                .await
                .expect_err("vendor error must fail");
            match err {
                ProviderError::RequestFailed { message, status_code, provider_error, retryable } => {
                    assert_eq!(status_code, Some(status));
                    assert_eq!(provider_error, Some(body.clone()));
                    let vendor_text = body.as_object().unwrap().values().next().unwrap().as_str().unwrap();
                    assert!(message.contains(vendor_text), "{message}");
                    assert!(!retryable);
                }
                other => panic!("expected RequestFailed, got {other:?}"),
            }
        }
    }

    /// A 200 without a usable image fails instead of fetching nothing: an
    /// empty `data`, and `url: null` with `is_image_safe: false`.
    #[tokio::test]
    async fn v4_success_without_an_image_url_fails() {
        for (body, expect) in [
            (json!({ "created": "2026-09-11T00:00:00Z", "data": [] }), "missing data[0].url"),
            (
                json!({ "created": "2026-09-11T00:00:00Z", "data": [{ "url": null, "prompt": "p", "resolution": "1024x1024", "is_image_safe": false, "seed": 1 }] }),
                "is_image_safe is false",
            ),
        ] {
            let (server, _img) = mock_v4(|_| ResponseTemplate::new(200).set_body_json(body)).await;
            let provider = make_provider(&server.uri());
            let err = provider
                .generate(&ref_schema("ideogram/ideogram-v4"), &make_base("x", "ideogram/ideogram-v4"), &v4_extras(None), &empty_materialized())
                .await
                .expect_err("no image must fail");
            assert!(err.to_string().contains(expect), "{err}");
        }
    }

    // ─── Ideogram audit, 2026-10-30 ─────────────────────────────────────────

    /// F11: every catalog row has its own endpoint, and an id with none is
    /// refused rather than run as Ideogram 3.0.
    #[test]
    fn every_catalog_row_has_its_own_endpoint_and_an_unknown_id_is_refused() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        // Video rows are video/ideogram.rs's (its own resolution test).
        for m in catalog.for_provider("ideogram").filter(|m| m.media_type == crate::capabilities::MediaType::Image) {
            assert!(IdeogramProvider::generation(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(IdeogramProvider::generation("ideogram/ideogram-9").is_none());
    }

    /// Ideogram answers every 3.0 generate with a 400, so only the outbound
    /// body matters.
    async fn refusing_v3() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/ideogram-v3/generate"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "stop" })))
            .mount(&server)
            .await;
        server
    }

    /// F4: on 3.0 `extra` carries only the row's allowlist, and never over
    /// what litegen sends (`rendering_speed` QUALITY on a TURBO row would bill
    /// another tier, `num_images` more images).
    #[tokio::test]
    async fn a_3_0_request_sends_only_allowlisted_extras_and_never_over_what_litegen_sends() {
        let server = refusing_v3().await;
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "prompt": "other", "rendering_speed": "QUALITY", "num_images": 4, "magic_prompt": "OFF", "not_allowlisted": 1
        }));
        let _ = make_provider(&server.uri())
            .generate(&ref_schema("ideogram/ideogram-v3-turbo"), &make_base("a lantern", "ideogram/ideogram-v3-turbo"), &extras, &empty_materialized())
            .await;
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["prompt"], "a lantern");
        assert_eq!(body["rendering_speed"], "TURBO");
        assert!(body.get("num_images").is_none() && body.get("not_allowlisted").is_none(), "{body}");
        assert_eq!(body["magic_prompt"], "OFF");
    }

    /// F4: with reference images the body is multipart, and an allowlisted
    /// list (`style_codes`, "A list of 8-character hexadecimal codes") or
    /// object (`color_palette`) goes too, not only strings. The spec does not
    /// say how multipart carries them: a list as repeated parts, an object as
    /// JSON text.
    /// @see <https://developer.ideogram.ai/v1/api-reference/generate-images/generate-v3> (2026-10-30)
    #[tokio::test]
    async fn a_3_0_request_with_references_sends_every_allowlisted_extra() {
        let server = refusing_v3().await;
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "style_codes": ["AAAAAAAA", "BBBBBBBB"], "color_palette": { "name": "EMBER" }, "rendering_speed": "QUALITY"
        }));
        let refs = MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "style".into(),
                form: MaterializedRefForm::MultipartField {
                    field_name: "style_reference_images".into(),
                    bytes: bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\n"),
                    content_type: "image/png".into(),
                },
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        };
        let _ = make_provider(&server.uri())
            .generate(&ref_schema("ideogram/ideogram-v3-turbo"), &make_base("x", "ideogram/ideogram-v3-turbo"), &extras, &refs)
            .await;
        let received = server.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&received[0].body).to_string();
        assert_eq!(body.matches("name=\"style_codes\"").count(), 2, "{body}");
        assert!(body.contains("AAAAAAAA") && body.contains("BBBBBBBB"), "{body}");
        assert_eq!(multipart_field(&received[0].body, "color_palette").as_deref(), Some("{\"name\":\"EMBER\"}"));
        assert_eq!(body.matches("name=\"rendering_speed\"").count(), 1, "{body}");
        assert_eq!(multipart_field(&received[0].body, "rendering_speed").as_deref(), Some("TURBO"));
    }

    // ─── Vendor feature audit, 2026-11-03 ───────────────────────────────────

    /// 3.0: "aspect_ratio … Output aspect ratio. Cannot be combined with
    /// resolution." An exact size goes as `resolution` and the ratio it already
    /// fixes is not sent beside it; a ratio alone still goes.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (2026-11-03)
    #[tokio::test]
    async fn a_3_0_size_goes_as_resolution_without_an_aspect_ratio() {
        let server = refusing_v3().await;
        let provider = make_provider(&server.uri());
        let id = "ideogram/ideogram-v3";
        let mut both = make_extras();
        both.size = Some("1024x1024".into());
        both.aspect_ratio = Some("16:9".into());
        let _ = provider.generate(&ref_schema(id), &make_base("x", id), &both, &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }).await;
        let _ = provider.generate(&ref_schema(id), &make_base("x", id), &make_extras(), &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }).await;
        let received = server.received_requests().await.unwrap();
        let sized: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(sized["resolution"], "1024x1024", "{sized}");
        assert!(sized.get("aspect_ratio").is_none(), "{sized}");
        let ratio: Value = serde_json::from_slice(&received[1].body).unwrap();
        assert_eq!(ratio["aspect_ratio"], "16x9", "{ratio}");
        assert!(ratio.get("resolution").is_none(), "{ratio}");
    }

    fn parts(fields: &[&str]) -> MaterializedRequest {
        MaterializedRequest {
            refs: fields
                .iter()
                .map(|f| crate::proxy::materializer::MaterializedRef {
                    role: match *f { "image" => "init", "mask" => "mask", _ => "reference" }.into(),
                    form: MaterializedRefForm::MultipartField {
                        field_name: (*f).into(),
                        bytes: bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\n"),
                        content_type: "image/png".into(),
                    },
                    time_s: None,
                })
                .collect(),
            cleanup: Cleanup::empty(),
        }
    }

    /// Precise Edit: "`reference_images` … Optional images to guide the edit
    /// (max 4 …). A request with a `mask` can include at most three, because
    /// the mask takes up one reference slot." Each goes as its own
    /// `reference_images` part; four beside a mask (one litegen drew from a
    /// region counts) are refused before anything is sent.
    /// @see <https://developer.ideogram.ai/v2/api-reference/images/precise-edit/ideogram-4-5.md> (2026-11-03)
    #[tokio::test]
    async fn a_masked_precise_edit_takes_at_most_three_references() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/image/precise-edit/ideogram-4-5"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "stop" })))
            .mount(&server)
            .await;
        let provider = make_provider(&server.uri());
        let id = "ideogram/ideogram-4-5-precise-edit";
        let r = "reference_images";
        let err = provider
            .generate(&ref_schema(id), &make_base("x", id), &make_extras(), &parts(&["image", "mask", r, r, r, r]))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing sent");
        for fields in [&["image", r, r, r, r][..], &["image", "mask", r, r, r][..]] {
            let _ = provider.generate(&ref_schema(id), &make_base("x", id), &make_extras(), &parts(fields)).await;
        }
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 2);
        for (req, n) in received.iter().zip([4, 3]) {
            let body = String::from_utf8_lossy(&req.body).to_string();
            assert_eq!(body.matches("name=\"reference_images\"").count(), n, "{body}");
        }
    }

    // ─── Ideogram 4.5: POST /v2/image/generate/ideogram-4-5 ────────────────
    // @see <https://developer.ideogram.ai/v2/api-reference/images/generate/ideogram-4-5.md> (2026-11-03)

    /// Source images as 4.5 takes them: (role, multipart field, a byte tag).
    fn parts_45(items: &[(&str, &str, &'static [u8])]) -> MaterializedRequest {
        MaterializedRequest {
            refs: items
                .iter()
                .map(|(role, field, tag)| crate::proxy::materializer::MaterializedRef {
                    role: (*role).into(),
                    form: MaterializedRefForm::MultipartField { field_name: (*field).into(), bytes: bytes::Bytes::from_static(tag), content_type: "image/png".into() },
                    time_s: None,
                })
                .collect(),
            cleanup: Cleanup::empty(),
        }
    }

    async fn refusing_45() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/image/generate/ideogram-4-5"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "stop" })))
            .mount(&server)
            .await;
        server
    }

    fn extras_45() -> ImageExtras {
        ImageExtras { aspect_ratio: None, ..make_extras() }
    }

    /// "POST https://api.ideogram.ai/v2/image/generate/ideogram-4-5" as
    /// multipart: prompt, size, quality, background and seed as their own
    /// fields; `extra` adds `magic_prompt` but cannot replace a field litegen
    /// sends, nor ask for more than one image.
    #[tokio::test]
    async fn ideogram_4_5_generates_with_its_own_fields() {
        let server = refusing_45().await;
        let id = "ideogram/ideogram-4-5";
        let mut base = make_base("a lighthouse", id);
        base.seed = Some(7);
        let mut extras = extras_45();
        extras.size = Some("2048x2048".into());
        extras.quality = Some("high".into());
        extras.background = Some("transparent".into());
        extras.extra = Some(json!({ "magic_prompt": "off", "quality": "low", "num_images": 4, "prompt": "other" }));
        let _ = make_provider(&server.uri()).generate(&ref_schema(id), &base, &extras, &empty_materialized()).await;
        let req = &server.received_requests().await.unwrap()[0];
        assert_eq!(req.url.path(), "/v2/image/generate/ideogram-4-5");
        for (field, want) in [("prompt", "a lighthouse"), ("size", "2048x2048"), ("quality", "high"), ("background", "transparent"), ("seed", "7"), ("magic_prompt", "off")] {
            assert_eq!(multipart_field(&req.body, field).as_deref(), Some(want), "{field}");
        }
        let body = String::from_utf8_lossy(&req.body).to_string();
        assert!(!body.contains("name=\"num_images\"") && !body.contains("name=\"images\""), "{body}");
        assert_eq!(body.matches("name=\"quality\"").count(), 1, "{body}");
    }

    /// Nano Banana resold on Ideogram v2 (`POST /v2/image/generate/nano-banana-2`,
    /// multipart): prompt, the images (the edited one first), aspect_ratio,
    /// `resolution_tier` (litegen's tier), seed, one image ("num_images" 1);
    /// `async` and `webhook_url` never go out.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-03)
    #[tokio::test]
    async fn nano_banana_through_ideogram_sends_its_own_fields() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "stop" }))).mount(&server).await;
        let id = "ideogram/nano-banana-2";
        let mut base = make_base("a koi pond", id);
        base.seed = Some(7);
        base.n = 3;
        let mut extras = extras_45();
        extras.aspect_ratio = Some("21:9".into());
        extras.resolution = Some("2K".into());
        let refs = parts_45(&[("reference", "images", b"REF-A"), ("init", "images", b"INIT")]);
        let _ = make_provider(&server.uri()).generate(&ref_schema(id), &base, &extras, &refs).await;
        let req = &server.received_requests().await.unwrap()[0];
        assert_eq!(req.url.path(), "/v2/image/generate/nano-banana-2");
        for (field, want) in [("prompt", "a koi pond"), ("aspect_ratio", "21:9"), ("resolution_tier", "2K"), ("seed", "7"), ("num_images", "1")] {
            assert_eq!(multipart_field(&req.body, field).as_deref(), Some(want), "{field}");
        }
        let body = String::from_utf8_lossy(&req.body).to_string();
        assert!(body.find("INIT").unwrap() < body.find("REF-A").unwrap(), "the edited image first: {body}");
        assert!(!body.contains("name=\"async\"") && !body.contains("name=\"webhook_url\""), "{body}");
    }

    /// P-Image on Ideogram v2 (`POST /v2/image/generate/p-image-ideogram`,
    /// JSON): prompt, `quality` (the row's default, medium, when none is
    /// asked), `resolution` as Ideogram's "1k" | "2k", aspect_ratio as "16x9",
    /// seed, one image; `async` and `webhook_url` never go out.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-04)
    #[tokio::test]
    async fn p_image_through_ideogram_sends_its_json_fields() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "stop" }))).mount(&server).await;
        let id = "ideogram/p-image-ideogram";
        let mut base = make_base("a paper boat", id);
        base.seed = Some(7);
        base.n = 3;
        let mut extras = extras_45();
        extras.quality = Some("high".into());
        extras.resolution = Some("2K".into());
        extras.aspect_ratio = Some("16:9".into());
        let p = make_provider(&server.uri());
        let _ = p.generate(&ref_schema(id), &base, &extras, &empty_materialized()).await;
        let _ = p.generate(&ref_schema(id), &make_base("a paper boat", id), &extras_45(), &empty_materialized()).await;
        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].url.path(), "/v2/image/generate/p-image-ideogram");
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).expect("a JSON body");
        assert_eq!(body, json!({ "prompt": "a paper boat", "num_images": 1, "quality": "high", "resolution": "2k", "aspect_ratio": "16x9", "seed": 7 }));
        let plain: serde_json::Value = serde_json::from_slice(&received[1].body).expect("a JSON body");
        assert_eq!((plain["quality"].as_str(), plain["resolution"].as_str()), (Some("medium"), Some("1k")), "the row's defaults go out: {plain}");
    }

    /// "The first image is the one being edited; any others are extra
    /// references": the `init` goes first whatever order the request listed
    /// them in, then each reference, then the mask.
    #[tokio::test]
    async fn ideogram_4_5_sends_the_edited_image_first() {
        let server = refusing_45().await;
        let id = "ideogram/ideogram-4-5";
        let refs = parts_45(&[("reference", "images", b"REF-A"), ("mask", "mask", b"MASK"), ("init", "images", b"INIT"), ("reference", "images", b"REF-B")]);
        let _ = make_provider(&server.uri()).generate(&ref_schema(id), &make_base("x", id), &extras_45(), &refs).await;
        let body = String::from_utf8_lossy(&server.received_requests().await.unwrap()[0].body).to_string();
        let at = |tag: &str| body.find(tag).unwrap_or_else(|| panic!("{tag} missing: {body}"));
        assert!(at("INIT") < at("REF-A") && at("REF-A") < at("REF-B") && at("REF-B") < at("MASK"), "{body}");
        assert_eq!((body.matches("name=\"images\"").count(), body.matches("name=\"mask\"").count()), (3, 1), "{body}");
    }

    /// What 4.5 cannot make is refused before any call: "`very_low` … requires
    /// source images"; "With a mask, the output is always the first source
    /// image's own size, so `size` cannot be set"; "A masked request can
    /// include at most three other source images"; a mask needs an image.
    #[tokio::test]
    async fn what_ideogram_4_5_cannot_make_is_refused_before_any_call() {
        let server = refusing_45().await;
        let id = "ideogram/ideogram-4-5";
        let p = make_provider(&server.uri());
        let none = empty_materialized();
        let very_low = ImageExtras { quality: Some("very_low".into()), ..extras_45() };
        let sized = ImageExtras { size: Some("1024x1024".into()), ..extras_45() };
        let plain = extras_45();
        let masked = parts_45(&[("init", "images", b"I"), ("mask", "mask", b"M")]);
        let crowded = parts_45(&[
            ("init", "images", b"I"),
            ("reference", "images", b"A"),
            ("reference", "images", b"B"),
            ("reference", "images", b"C"),
            ("reference", "images", b"D"),
            ("mask", "mask", b"M"),
        ]);
        let mask_only = parts_45(&[("mask", "mask", b"M")]);
        for (extras, refs, why) in [
            (&very_low, &none, "very_low without source images"),
            (&sized, &masked, "a size beside a mask"),
            (&plain, &crowded, "four others beside a mask"),
            (&plain, &mask_only, "a mask with no image"),
        ] {
            let err = p.generate(&ref_schema(id), &make_base("x", id), extras, refs).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{why}: {err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing sent");
    }
}
