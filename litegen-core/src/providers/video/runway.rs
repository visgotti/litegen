use crate::providers::auth::Authorize;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::MaterializedRequest;
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    CancelOutcome, VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Runway video generation provider: gen4.5 (text- and image-to-video),
/// gen4_turbo (image-to-video), and the third-party models Runway resells on
/// the same endpoints ([`RESOLD`]: Veo, Gemini Omni Flash, Kling, MiniMax H3,
/// Seedance, Grok Imagine, Wan, HappyHorse), each sent its own branch.
///
/// Submits text-to-video / image-to-video tasks to the Runway API
/// (`api.dev.runwayml.com`) with the required `X-Runway-Version: 2024-11-06` header.
///
/// @see <https://docs.dev.runwayml.com/api/> — Runway API reference (Start generating + Task management)
/// @see <https://docs.dev.runwayml.com/guides/using-the-api/> — API getting-started guide
pub struct RunwayProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
    /// How every request to the vendor's API is authenticated.
    auth: crate::providers::auth::AuthSpec,
}

/// What litegen itself sends to `/text_to_video` and `/image_to_video`;
/// `extra` cannot replace it.
const OWNED_FIELDS: &[&str] = &["model", "promptText", "promptImage", "duration", "ratio", "seed"];

/// What litegen sends a resold model's branch; `extra` cannot replace it.
const RESOLD_OWNED_FIELDS: &[&str] =
    &["model", "promptText", "promptImage", "duration", "ratio", "seed", "references", "resolution", "audio", "negativePrompt"];

/// One size a resold model takes: (litegen `aspect_ratio`, `resolution` tier,
/// or "" for a model with one tier, Runway `ratio`).
type Size = (&'static str, &'static str, &'static str);

/// How one mode of a resold model's branch takes the output's shape.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    /// `ratio`: the table's size at the asked (else default) aspect and tier.
    Size,
    /// The same, only when an aspect or a tier is asked: the mode's `ratio` is
    /// optional and the image sets the shape otherwise. A tier alone is
    /// `auto_<tier>` where the branch has those, else the default aspect.
    SizeIfAsked,
    /// `ratio`: the aspect itself ("16:9"), asked or default.
    Named,
    /// The aspect itself, only when asked.
    NamedIfAsked,
    /// `ratio`: `auto_<tier>` at the asked (else default) tier; the image sets
    /// the aspect.
    AutoTier,
    /// No `ratio`.
    None,
}

/// A third-party video model Runway sells on `POST /v1/text_to_video` and
/// `/v1/image_to_video`: its two branches of the request unions, read
/// 2026-11-10. Prices, bounds and roles are on its catalog row.
/// @see <https://docs.dev.runwayml.com/openapi.json> (2026-11-10)
pub(crate) struct ResoldVideo {
    /// Runway's `model`, and the catalog id's suffix.
    wire: &'static str,
    /// The text-to-video `ratio` sizes by aspect and tier (image-to-video's
    /// enum holds the same), generated from the branch
    /// (`.superpowers/sdd/2026-11-04-tokenhub-bedrock-fal/runway-gen/gen-video.mjs`).
    sizes: &'static [Size],
    t2v: Shape,
    i2v: Shape,
    /// Whether image-to-video's `ratio` takes `auto_<tier>`.
    auto_tiers: bool,
    /// The `resolution` field's values, (litegen tier, Runway value), where a
    /// mode takes one.
    resolution: &'static [(&'static str, &'static str)],
    resolution_t2v: bool,
    resolution_i2v: bool,
    /// `references` maxItems per mode: reference images in a mode with none
    /// are refused.
    references_t2v: usize,
    references_i2v: usize,
    /// Text-to-video requires `promptText` though image-to-video doesn't, so
    /// the row's prompt is optional and a text-only request without one is
    /// refused here.
    t2v_needs_prompt: bool,
}

const NO_SIZES: &[Size] = &[];

/// veo3.1, veo3.1_fast.
const VEO3_1_SIZES: &[Size] = &[
    ("9:16", "720p", "720:1280"), ("16:9", "720p", "1280:720"),
    ("9:16", "1080p", "1080:1920"), ("16:9", "1080p", "1920:1080"),
];

/// gemini_omni_flash.
const GEMINI_OMNI_FLASH_SIZES: &[Size] = &[
    ("9:16", "720p", "720:1280"), ("16:9", "720p", "1280:720"),
];

/// gemini_omni_flash_1.1, gemini_omni_flash_1.2.
const GEMINI_OMNI_FLASH_1_1_SIZES: &[Size] = &[
    ("9:16", "360p", "360:640"), ("16:9", "360p", "640:360"),
    ("9:16", "720p", "720:1280"), ("16:9", "720p", "1280:720"),
    ("9:16", "1080p", "1080:1920"), ("16:9", "1080p", "1920:1080"),
    ("9:16", "4k", "2160:3840"), ("16:9", "4k", "3840:2160"),
];

/// kling2.5_turbo_pro.
const KLING2_5_TURBO_PRO_SIZES: &[Size] = &[
    ("1:1", "720p", "720:720"), ("9:16", "720p", "720:1280"), ("16:9", "720p", "1280:720"),
    ("1:1", "1080p", "1440:1440"), ("9:16", "1080p", "1080:1920"), ("16:9", "1080p", "1920:1080"),
];

/// kling3.0_standard, klingO3_standard.
const KLING3_0_STANDARD_SIZES: &[Size] = &[
    ("1:1", "", "960:960"), ("9:16", "", "720:1280"), ("16:9", "", "1280:720"),
];

/// kling3.0_pro, klingO3_pro.
const KLING3_0_PRO_SIZES: &[Size] = &[
    ("1:1", "", "1440:1440"), ("9:16", "", "1080:1920"), ("16:9", "", "1920:1080"),
];

/// kling3.0_4k, klingO3_4k.
const KLING3_0_4K_SIZES: &[Size] = &[
    ("1:1", "", "2880:2880"), ("9:16", "", "2160:3840"), ("16:9", "", "3840:2160"),
];

/// kling_4.
const KLING_4_SIZES: &[Size] = &[
    ("1:1", "720p", "940:940"), ("9:16", "720p", "720:1280"), ("16:9", "720p", "1280:720"), ("21:9", "720p", "1460:624"),
    ("1:1", "1080p", "1440:1440"), ("9:16", "1080p", "1080:1920"), ("16:9", "1080p", "1920:1080"), ("21:9", "1080p", "2200:940"),
];

/// kling_4_flash.
const KLING_4_FLASH_SIZES: &[Size] = &[
    ("1:1", "", "960:960"), ("9:16", "", "720:1280"), ("16:9", "", "1280:720"), ("21:9", "", "1464:628"),
];

/// seedance2.
const SEEDANCE2_SIZES: &[Size] = &[
    ("1:1", "480p", "640:640"), ("3:4", "480p", "560:752"), ("4:3", "480p", "752:560"), ("9:16", "480p", "496:864"), ("16:9", "480p", "864:496"), ("21:9", "480p", "992:432"),
    ("1:1", "720p", "960:960"), ("3:4", "720p", "834:1112"), ("4:3", "720p", "1112:834"), ("9:16", "720p", "720:1280"), ("16:9", "720p", "1280:720"), ("21:9", "720p", "1470:630"),
    ("1:1", "1080p", "1440:1440"), ("3:4", "1080p", "1248:1664"), ("4:3", "1080p", "1664:1248"), ("9:16", "1080p", "1080:1920"), ("16:9", "1080p", "1920:1080"), ("21:9", "1080p", "2206:946"),
    ("1:1", "4k", "3840:3840"), ("3:4", "4k", "2880:3840"), ("4:3", "4k", "3840:2880"), ("9:16", "4k", "2160:3840"), ("16:9", "4k", "3840:2160"), ("21:9", "4k", "3840:1646"),
];

/// seedance2_fast, seedance2_mini.
const SEEDANCE2_FAST_SIZES: &[Size] = &[
    ("1:1", "480p", "640:640"), ("3:4", "480p", "560:752"), ("4:3", "480p", "752:560"), ("9:16", "480p", "496:864"), ("16:9", "480p", "864:496"), ("21:9", "480p", "992:432"),
    ("1:1", "720p", "960:960"), ("3:4", "720p", "834:1112"), ("4:3", "720p", "1112:834"), ("9:16", "720p", "720:1280"), ("16:9", "720p", "1280:720"), ("21:9", "720p", "1470:630"),
];

/// seedance2_5.
const SEEDANCE2_5_SIZES: &[Size] = &[
    ("1:1", "480p", "640:640"), ("3:4", "480p", "560:752"), ("4:3", "480p", "752:560"), ("9:16", "480p", "480:854"), ("16:9", "480p", "854:480"), ("21:9", "480p", "992:432"),
    ("1:1", "720p", "960:960"), ("3:4", "720p", "834:1112"), ("4:3", "720p", "1112:834"), ("9:16", "720p", "720:1280"), ("16:9", "720p", "1280:720"), ("21:9", "720p", "1470:630"),
    ("1:1", "1080p", "1440:1440"), ("3:4", "1080p", "1248:1664"), ("4:3", "1080p", "1664:1248"), ("9:16", "1080p", "1080:1920"), ("16:9", "1080p", "1920:1080"), ("21:9", "1080p", "2206:946"),
];

/// grok_imagine_1_5_lite.
const GROK_IMAGINE_1_5_LITE_SIZES: &[Size] = &[
    ("1:1", "480p", "480:480"), ("2:3", "480p", "480:720"), ("3:2", "480p", "720:480"), ("3:4", "480p", "480:640"), ("4:3", "480p", "640:480"), ("9:16", "480p", "480:848"), ("16:9", "480p", "848:480"),
    ("1:1", "720p", "720:720"), ("2:3", "720p", "720:1088"), ("3:2", "720p", "1088:720"), ("3:4", "720p", "720:960"), ("4:3", "720p", "960:720"), ("9:16", "720p", "720:1280"), ("16:9", "720p", "1280:720"),
    ("1:1", "1080p", "1424:1424"), ("2:3", "1080p", "1152:1744"), ("3:2", "1080p", "1744:1152"), ("3:4", "1080p", "1232:1648"), ("4:3", "1080p", "1648:1232"), ("9:16", "1080p", "1072:1904"), ("16:9", "1080p", "1904:1072"),
];

/// wan3, wan3_prime.
const WAN3_SIZES: &[Size] = &[
    ("1:1", "480p", "624:624"), ("3:4", "480p", "544:720"), ("4:3", "480p", "720:544"), ("9:16", "480p", "480:832"), ("16:9", "480p", "832:480"),
    ("1:1", "720p", "960:960"), ("3:4", "720p", "832:1104"), ("4:3", "720p", "1104:832"), ("9:16", "720p", "720:1280"), ("16:9", "720p", "1280:720"),
    ("1:1", "1080p", "1440:1440"), ("3:4", "1080p", "1248:1648"), ("4:3", "1080p", "1648:1248"), ("9:16", "1080p", "1080:1920"), ("16:9", "1080p", "1920:1080"),
];

/// happyhorse_1_0.
const HAPPYHORSE_1_0_SIZES: &[Size] = &[
    ("1:1", "720p", "960:960"), ("3:4", "720p", "832:1108"), ("4:3", "720p", "1108:832"), ("9:16", "720p", "720:1280"), ("16:9", "720p", "1280:720"),
    ("1:1", "1080p", "1440:1440"), ("3:4", "1080p", "1248:1662"), ("4:3", "1080p", "1662:1248"), ("9:16", "1080p", "1080:1920"), ("16:9", "1080p", "1920:1080"),
];

/// Every resold video model litegen carries (models/runway.yaml).
/// @see <https://docs.dev.runwayml.com/guides/pricing.md> (2026-11-10)
const RESOLD: &[ResoldVideo] = &[
    ResoldVideo { wire: "veo3.1", sizes: VEO3_1_SIZES, t2v: Shape::Size, i2v: Shape::Size, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 0, references_i2v: 0, t2v_needs_prompt: true },
    ResoldVideo { wire: "veo3.1_fast", sizes: VEO3_1_SIZES, t2v: Shape::Size, i2v: Shape::Size, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 0, references_i2v: 0, t2v_needs_prompt: true },
    ResoldVideo { wire: "gemini_omni_flash", sizes: GEMINI_OMNI_FLASH_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 0, references_i2v: 0, t2v_needs_prompt: true },
    ResoldVideo { wire: "gemini_omni_flash_1.1", sizes: GEMINI_OMNI_FLASH_1_1_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 5, references_i2v: 0, t2v_needs_prompt: false },
    ResoldVideo { wire: "gemini_omni_flash_1.2", sizes: GEMINI_OMNI_FLASH_1_1_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 5, references_i2v: 0, t2v_needs_prompt: false },
    ResoldVideo { wire: "kling2.5_turbo_pro", sizes: KLING2_5_TURBO_PRO_SIZES, t2v: Shape::Size, i2v: Shape::None, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 0, references_i2v: 0, t2v_needs_prompt: false },
    ResoldVideo { wire: "kling3.0_standard", sizes: KLING3_0_STANDARD_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 0, references_i2v: 0, t2v_needs_prompt: false },
    ResoldVideo { wire: "kling3.0_pro", sizes: KLING3_0_PRO_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 0, references_i2v: 0, t2v_needs_prompt: false },
    ResoldVideo { wire: "kling3.0_4k", sizes: KLING3_0_4K_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 0, references_i2v: 0, t2v_needs_prompt: false },
    ResoldVideo { wire: "klingO3_standard", sizes: KLING3_0_STANDARD_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 0, references_i2v: 7, t2v_needs_prompt: false },
    ResoldVideo { wire: "klingO3_pro", sizes: KLING3_0_PRO_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 0, references_i2v: 7, t2v_needs_prompt: false },
    ResoldVideo { wire: "klingO3_4k", sizes: KLING3_0_4K_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 0, references_i2v: 7, t2v_needs_prompt: false },
    ResoldVideo { wire: "kling_4", sizes: KLING_4_SIZES, t2v: Shape::Size, i2v: Shape::AutoTier, auto_tiers: true, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 10, references_i2v: 10, t2v_needs_prompt: true },
    ResoldVideo { wire: "kling_4_flash", sizes: KLING_4_FLASH_SIZES, t2v: Shape::Size, i2v: Shape::None, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 10, references_i2v: 10, t2v_needs_prompt: true },
    ResoldVideo { wire: "hailuo3", sizes: NO_SIZES, t2v: Shape::Named, i2v: Shape::NamedIfAsked, auto_tiers: false, resolution: &[("768P", "768P"), ("2K", "2K")], resolution_t2v: true, resolution_i2v: true, references_t2v: 9, references_i2v: 0, t2v_needs_prompt: false },
    ResoldVideo { wire: "h3_max", sizes: NO_SIZES, t2v: Shape::None, i2v: Shape::None, auto_tiers: false, resolution: &[("480P", "480p"), ("768P", "768p")], resolution_t2v: true, resolution_i2v: true, references_t2v: 0, references_i2v: 0, t2v_needs_prompt: false },
    ResoldVideo { wire: "seedance2", sizes: SEEDANCE2_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 9, references_i2v: 0, t2v_needs_prompt: true },
    ResoldVideo { wire: "seedance2_fast", sizes: SEEDANCE2_FAST_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 9, references_i2v: 0, t2v_needs_prompt: true },
    ResoldVideo { wire: "seedance2_mini", sizes: SEEDANCE2_FAST_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 9, references_i2v: 0, t2v_needs_prompt: true },
    ResoldVideo { wire: "seedance2_5", sizes: SEEDANCE2_5_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: false, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 30, references_i2v: 0, t2v_needs_prompt: false },
    ResoldVideo { wire: "grok_imagine_1_5", sizes: NO_SIZES, t2v: Shape::Named, i2v: Shape::None, auto_tiers: false, resolution: &[("480p", "480p"), ("720p", "720p"), ("1080p", "1080p")], resolution_t2v: true, resolution_i2v: true, references_t2v: 7, references_i2v: 0, t2v_needs_prompt: true },
    ResoldVideo { wire: "grok_imagine_1_5_lite", sizes: GROK_IMAGINE_1_5_LITE_SIZES, t2v: Shape::Size, i2v: Shape::AutoTier, auto_tiers: true, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 0, references_i2v: 0, t2v_needs_prompt: true },
    ResoldVideo { wire: "wan3", sizes: WAN3_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: true, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 10, references_i2v: 0, t2v_needs_prompt: false },
    ResoldVideo { wire: "wan3_prime", sizes: WAN3_SIZES, t2v: Shape::Size, i2v: Shape::SizeIfAsked, auto_tiers: true, resolution: &[], resolution_t2v: false, resolution_i2v: false, references_t2v: 10, references_i2v: 0, t2v_needs_prompt: false },
    ResoldVideo { wire: "happyhorse_1_0", sizes: HAPPYHORSE_1_0_SIZES, t2v: Shape::Size, i2v: Shape::None, auto_tiers: false, resolution: &[("720p", "720p"), ("1080p", "1080p")], resolution_t2v: false, resolution_i2v: true, references_t2v: 0, references_i2v: 0, t2v_needs_prompt: true },
];

/// Which request a catalog id takes.
enum Native {
    /// Runway's own gen4.5 and gen4_turbo.
    Gen4(&'static str),
    /// A resold model's branches.
    Resold(&'static ResoldVideo),
}

impl RunwayProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(600), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            client: Self::api_client(&Default::default()),
            auth: crate::providers::auth::AuthSpec::bearer(),
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.dev.runwayml.com/v1")
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("runway".into()))
    }

    /// Map internal model ID to Runway API model name.
    ///
    /// `gen3a_turbo` and `gen4_aleph` were removed from the API on 2026-07-30
    /// ("Requests that use these model identifiers will fail") and are not in
    /// the `/v1/image_to_video` model union. `gen4_turbo` is the fast tier,
    /// `gen4.5` the quality tier.
    /// @see <https://docs.dev.runwayml.com/api-details/api_changelog/>
    fn resolve_model(model: &str) -> Option<Native> {
        match model {
            "runway/gen4.5" | "runway-gen4.5" | "gen4.5" => Some(Native::Gen4("gen4.5")),
            "runway/gen4-turbo" | "runway-gen4-turbo" | "gen4_turbo" => Some(Native::Gen4("gen4_turbo")),
            // An id with no arm is refused, not billed as gen4_turbo (L13).
            id => {
                let wire = id.strip_prefix("runway/")?;
                RESOLD.iter().find(|r| r.wire == wire).map(Native::Resold)
            }
        }
    }

    /// The endpoint and body of a gen4.5 or gen4_turbo request.
    fn gen4_request(
        &self,
        model_name: &'static str,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<(&'static str, Value), ProviderError> {
        // The start frame (`first_frame`, or `init` from older callers), as a
        // URL or a data URI of its own type; one Runway cannot take is
        // refused here.
        let prompt_image: Option<String> = match materialized.refs.iter().find(|r| r.role == "first_frame" || r.role == "init") {
            Some(r) => crate::providers::image::runway::runway_uri(&r.form)?,
            None => None,
        };

        // gen4_turbo is only in the `/v1/image_to_video` model union — Runway's
        // `/v1/text_to_video` has no gen4_turbo branch — so a prompt-only
        // request can only be rejected. Fail here, before the round trip.
        // @see <https://docs.dev.runwayml.com/openapi.json>
        if prompt_image.is_none() && model_name == "gen4_turbo" {
            return Err(ProviderError::InvalidRequest(
                "runway gen4_turbo is image-to-video only: supply an init image, \
                 or use runway/gen4.5 for text-to-video"
                    .to_string(),
            ));
        }

        // Choose endpoint: image_to_video if image provided, else text_to_video
        let endpoint = if prompt_image.is_some() {
            "image_to_video"
        } else {
            "text_to_video"
        };

        let duration = extras.duration_seconds as u64;

        let mut body = json!({
            "model": crate::providers::wire_model(self.config.as_ref(), &model.id, model_name),
            "duration": duration,
        });
        // promptText is "A non-empty string", and optional for gen4_turbo.
        if !base.prompt.is_empty() {
            body["promptText"] = Value::String(base.prompt.clone());
        }

        if let Some(img_url) = prompt_image {
            body["promptImage"] = Value::String(img_url);
        }

        // Runway video requires `ratio` as a pixel-pair string. The gen4 enum is
        // 1280:720 | 720:1280 | 1104:832 | 832:1104 | 960:960 | 1584:672 — the
        // gen3-era 1280:768 / 768:1280 pair is no longer accepted. The unified
        // aspect_ratio (validated against models/runway.yaml, which lists exactly
        // the enum above) is forwarded verbatim; default to landscape 720p when
        // none was supplied so the required field is always present.
        let ratio = extras.aspect_ratio.as_deref().unwrap_or("1280:720");
        body["ratio"] = Value::String(ratio.to_string());

        if let Some(seed) = base.seed {
            body["seed"] = json!(seed);
        }

        // Shallow-merge extra, never over what litegen sends.
        if let Some(body_obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
                body_obj.insert(k.clone(), v.clone());
            }
        }
        Ok((endpoint, body))
    }

    /// The `ratio` a resold model's mode takes (see [`Shape`]); `None` when
    /// the mode sends none.
    fn resold_ratio(r: &ResoldVideo, shape: Shape, model: &ModelSchema, extras: &VideoExtras) -> Result<Option<String>, ProviderError> {
        use crate::capabilities::schema::ParamSpec;
        let default = |param: &str| match model.params.get(param) {
            Some(ParamSpec::AspectRatio(a)) => a.default.clone(),
            Some(ParamSpec::String(s)) => s.default.clone(),
            _ => None,
        };
        let asked = extras.aspect_ratio.is_some() || extras.resolution.is_some();
        let aspect = || extras.aspect_ratio.clone().or_else(|| default("aspect_ratio"));
        let tier = || extras.resolution.clone().or_else(|| default("resolution"));
        let auto = |t: String| format!("auto_{}", t.to_lowercase());
        let size = || {
            let aspect = aspect().unwrap_or_default();
            let tier = tier().unwrap_or_else(|| r.sizes.first().map_or("", |s| s.1).to_string());
            r.sizes
                .iter()
                .find(|(a, t, _)| *a == aspect && *t == tier)
                .map(|s| Some(s.2.to_string()))
                .ok_or_else(|| ProviderError::InvalidRequest(format!("{} has no {aspect} size at {tier:?}", model.id)))
        };
        match shape {
            Shape::None => Ok(None),
            Shape::Named => Ok(aspect()),
            Shape::NamedIfAsked => Ok(extras.aspect_ratio.clone()),
            Shape::AutoTier => Ok(tier().map(auto)),
            Shape::SizeIfAsked if !asked => Ok(None),
            Shape::SizeIfAsked if extras.aspect_ratio.is_none() && r.auto_tiers => Ok(tier().map(auto)),
            Shape::Size | Shape::SizeIfAsked => size(),
        }
    }

    /// The endpoint and body of a resold model's request: image-to-video when
    /// a start frame comes, else text-to-video, each with only its own fields.
    fn resold_request(
        &self,
        r: &ResoldVideo,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<(&'static str, Value), ProviderError> {
        use crate::providers::image::runway::runway_uri;
        let (mut first, mut last, mut references) = (None, None, Vec::new());
        for rf in &materialized.refs {
            let Some(uri) = runway_uri(&rf.form)? else { continue };
            match rf.role.as_str() {
                "first_frame" | "init" => first = Some(uri),
                "last_frame" => last = Some(uri),
                _ => references.push(json!({ "uri": uri })),
            }
        }
        // Veo: "This model does not support generating with only a last frame";
        // every promptImage array here leads with its first frame.
        if last.is_some() && first.is_none() {
            return Err(ProviderError::InvalidRequest(format!("{}: a last frame needs a first frame", model.id)));
        }
        let i2v = first.is_some();
        let (shape, references_max, resolution_field) =
            if i2v { (r.i2v, r.references_i2v, r.resolution_i2v) } else { (r.t2v, r.references_t2v, r.resolution_t2v) };
        if !references.is_empty() && references_max == 0 {
            return Err(ProviderError::InvalidRequest(format!(
                "{}: reference images go {}",
                model.id,
                if i2v { "without a start frame (text-to-video)" } else { "with a start frame (image-to-video)" }
            )));
        }
        if !i2v && r.t2v_needs_prompt && base.prompt.is_empty() {
            return Err(ProviderError::InvalidRequest(format!("{}: text-to-video needs a prompt (or attach a first_frame)", model.id)));
        }

        // `extra` first, so nothing below can be replaced by it.
        let mut body = json!({});
        for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| RESOLD_OWNED_FIELDS.contains(&k)) {
            body[k] = v.clone();
        }
        body["model"] = Value::String(crate::providers::wire_model(self.config.as_ref(), &model.id, r.wire).to_string());
        if !base.prompt.is_empty() {
            body["promptText"] = Value::String(base.prompt.clone());
        }
        if let Some(first) = first {
            body["promptImage"] = match last {
                Some(last) => json!([{ "uri": first, "position": "first" }, { "uri": last, "position": "last" }]),
                None => Value::String(first),
            };
        }
        if !references.is_empty() {
            body["references"] = Value::Array(references);
        }
        if let Some(ratio) = Self::resold_ratio(r, shape, model, extras)? {
            body["ratio"] = Value::String(ratio);
        }
        if resolution_field {
            let tier = extras.resolution.clone().or_else(|| match model.params.get("resolution") {
                Some(crate::capabilities::schema::ParamSpec::String(s)) => s.default.clone(),
                _ => None,
            });
            if let Some(tier) = tier {
                let wire = r.resolution.iter().find(|(t, _)| *t == tier).map(|(_, w)| *w).ok_or_else(|| {
                    ProviderError::InvalidRequest(format!("{} has no resolution {tier}", model.id))
                })?;
                body["resolution"] = Value::String(wire.to_string());
            }
        }
        body["duration"] = json!(extras.duration_seconds as u64);
        // Each reaches only a model whose row declares it (the validator).
        if let Some(audio) = extras.audio {
            body["audio"] = Value::Bool(audio);
        }
        if let Some(seed) = base.seed {
            body["seed"] = json!(seed);
        }
        if let Some(negative) = base.negative_prompt.as_deref().filter(|n| !n.is_empty()) {
            body["negativePrompt"] = Value::String(negative.to_string());
        }
        Ok((if i2v { "image_to_video" } else { "text_to_video" }, body))
    }
}

impl Default for RunwayProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for RunwayProvider {
    fn name(&self) -> &str {
        "runway"
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

    /// Submit a task: `POST {api_base}/text_to_video` (or `/image_to_video`
    /// when an init image is supplied), with `X-Runway-Version: 2024-11-06`.
    ///
    /// @see <https://docs.dev.runwayml.com/api/> — Start generating. Proves the `model`/`promptText`/
    ///   `promptImage`/`duration`/`ratio`/`seed` request fields, the version header, and the `id` response.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let api_key = self.api_key()?;
        let (endpoint, body) = match Self::resolve_model(&model.id) {
            Some(Native::Gen4(model_name)) => self.gen4_request(model_name, model, base, extras, materialized)?,
            Some(Native::Resold(r)) => self.resold_request(r, model, base, extras, materialized)?,
            None => return Err(ProviderError::ModelNotSupported { model: model.id.clone(), provider: "runway".into() }),
        };
        let url = format!("{}/{}", self.api_base(), endpoint);

        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(&url)
                .authorized(&self.auth, &api_key)?
                .header("X-Runway-Version", "2024-11-06")
                .header("Content-Type", "application/json")
                .json(&body),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Runway request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            // Only a connection that never opened is safe to retry: a request
            // that timed out may have been accepted and billed.
            retryable: e.is_connect(),
        })?;

        let status = resp.status();
        let retry_after = crate::providers::retry_after_ms(resp.headers());
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let resp_json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let err = resp_json["message"]
                .as_str()
                .or_else(|| resp_json["error"].as_str())
                .map(str::to_string)
                .unwrap_or_else(|| text.chars().take(200).collect());
            if let Some(e) = crate::providers::rate_limited("runway", status, retry_after, &format!("Runway API error: {err}"), Some(&resp_json)) {
                return Err(e);
            }
            return Err(ProviderError::RequestFailed {
                message: format!("Runway API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                // Turned away, not run: "429 | If the client makes too many requests … | Yes".
                // @see <https://docs.dev.runwayml.com/errors/errors.md>
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }
        let resp_json: Value = serde_json::from_str(&text).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Runway response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        let job_id = resp_json["id"]
            .as_str()
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Runway response missing id".to_string(),
                status_code: None,
                provider_error: Some(resp_json.clone()),
                retryable: false,
            })?
            .to_string();

        Ok(VideoGenerationHandle {
            provider_job_id: job_id,
            provider: "runway".to_string(),
            model: model.id.clone(),
        })
    }

    /// Poll a task (`GET {api_base}/tasks/{id}`), reading `status`
    /// (`SUCCEEDED`/`FAILED`/`RUNNING`/`THROTTLED`) and the `output[]` URLs.
    ///
    /// @see <https://docs.dev.runwayml.com/api/> — Task management (retrieve a task)
    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let api_key = self.api_key()?;

        let resp = crate::providers::inject_trace_headers(
            self.client
                .get(format!("{}/tasks/{}", self.api_base(), handle.provider_job_id))
                .authorized(&self.auth, &api_key)?
                .header("X-Runway-Version", "2024-11-06"),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: e.to_string(),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        // A 404 is "the referenced resource is not available", not a task
        // still running; a 5xx or 429 is asked again.
        // @see <https://docs.dev.runwayml.com/errors/errors.md>
        let data = super::read_poll_json(resp, "runway").await?;

        // The task response union's status constants are
        // PENDING | THROTTLED | CANCELLED | RUNNING | FAILED | SUCCEEDED.
        // CANCELLED is terminal — treating it as Pending polls until timeout.
        let mut status = match data["status"].as_str() {
            Some("SUCCEEDED") => GenerationStatus::Completed,
            Some("FAILED") | Some("CANCELLED") => GenerationStatus::Failed,
            Some("RUNNING") | Some("THROTTLED") => GenerationStatus::Processing,
            Some("PENDING") => GenerationStatus::Pending,
            other => return Ok(super::unknown_state("runway", other)),
        };

        let video_url = data["output"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(String::from);

        let mut error = data["failure"]
            .as_str()
            .or_else(|| data["failureCode"].as_str())
            .map(String::from);
        // A success with nothing to deliver is a failure, not an empty result.
        if status == GenerationStatus::Completed && video_url.is_none() {
            status = GenerationStatus::Failed;
            error = Some("runway task succeeded with no output".into());
        }

        // RUNNING `progress` is a fraction: "minimum": 0, "maximum": 1.
        let progress = match status {
            GenerationStatus::Completed => 100,
            GenerationStatus::Processing => (data["progress"].as_f64().unwrap_or(0.5) * 100.0).round().clamp(1.0, 99.0) as u8,
            _ => 0,
        };

        Ok(VideoGenerationPollResult {
            status,
            progress,
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error,
            metadata: HashMap::new(),
        })
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &VideoGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(
            model.pricing.base_cost_usd,
            0.0,
            CostSource::Estimated,
            Some(json!({ "model": model.id })),
        ))
    }

    /// Cancel a task that has not finished (`DELETE {api_base}/tasks/{id}`).
    /// The same call DELETES a finished task — refunding nothing and dropping
    /// its output — so the task's status is read first and a finished one is
    /// left alone.
    ///
    /// @see <https://docs.dev.runwayml.com/openapi.json> — `DELETE /v1/tasks/{id}`, "Cancel or delete
    /// a task": "Tasks that are running, pending, or throttled can be canceled by invoking this
    /// method. Invoking this method for other tasks will delete them." 204 on success; 404 for a
    /// task already deleted or aborted.
    async fn cancel(&self, handle: &VideoGenerationHandle) -> Result<CancelOutcome, ProviderError> {
        let api_key = self.api_key()?;
        let url = format!("{}/tasks/{}", self.api_base(), handle.provider_job_id);
        let failed = |e: reqwest::Error| ProviderError::RequestFailed {
            message: format!("runway cancel failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        };
        let task = crate::providers::inject_trace_headers(
            self.client
                .get(&url)
                .authorized(&self.auth, &api_key)?
                .header("X-Runway-Version", "2024-11-06"),
        )
        .send()
        .await
        .map_err(failed)?;
        if task.status().as_u16() == 404 {
            return Ok(CancelOutcome::NotFound);
        }
        let task: Value = task.json().await.unwrap_or(Value::Null);
        if matches!(task["status"].as_str(), Some("SUCCEEDED" | "FAILED" | "CANCELLED")) {
            return Ok(CancelOutcome::AlreadyFinished);
        }
        let resp = crate::providers::inject_trace_headers(
            self.client
                .delete(&url)
                .authorized(&self.auth, &api_key)?
                .header("X-Runway-Version", "2024-11-06"),
        )
        .send()
        .await
        .map_err(failed)?;
        match resp.status().as_u16() {
            200..=299 => Ok(CancelOutcome::Requested),
            404 => Ok(CancelOutcome::NotFound),
            code => Err(ProviderError::RequestFailed {
                message: format!("runway cancel answered {code}"),
                status_code: Some(code),
                provider_error: resp.json().await.ok(),
                retryable: code >= 500,
            }),
        }
    }

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Runway provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult {
            healthy: true,
            message: "Runway provider configured".into(),
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::MaterializedRefForm;
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
        for m in catalog.for_provider("runway").filter(|m| m.media_type == crate::capabilities::MediaType::Video) {
            assert!(RunwayProvider::resolve_model(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(RunwayProvider::resolve_model("runway/not-a-model").is_none());
    }

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn empty_materialized() -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    fn make_provider(api_base: &str) -> RunwayProvider {
        let mut p = RunwayProvider::new();
        p.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: "test-key".to_string(),
            api_keys: vec![],
            api_base: Some(api_base.to_string()),
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });
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

    fn make_extras() -> VideoExtras {
        VideoExtras {
            duration_seconds: 5.0,
            aspect_ratio: None,
            resolution: None,
            fps: None,
            extra: None,
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
        }
    }

    #[tokio::test]
    async fn submits_runway_job_and_returns_handle() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/text_to_video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "runway_job_1",
                "status": "RUNNING"
            })))
            .mount(&server)
            .await;

        // Text-to-video is gen4.5's job: gen4_turbo is not in /v1/text_to_video.
        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4.5");
        let base = make_base("a drone shot over a misty forest", "runway/gen4.5");
        let extras = make_extras();
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let handle = result.unwrap();

        assert_eq!(handle.provider_job_id, "runway_job_1");
        assert_eq!(handle.provider, "runway");

        // Verify outbound request body
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "gen4.5");
        assert_eq!(body["promptText"], "a drone shot over a misty forest");
        assert_eq!(body["duration"], 5);

        // Verify Runway version header sent
        let req = &received[0];
        let version_header = req.headers.get("x-runway-version");
        assert!(version_header.is_some(), "X-Runway-Version header missing");
        assert_eq!(version_header.unwrap(), "2024-11-06");
    }

    /// The start frame arrives as `first_frame` (the role every video row
    /// shares) or, from older callers, `init`; either is the promptImage.
    #[tokio::test]
    async fn a_first_frame_ref_is_the_prompt_image() {
        for role in ["first_frame", "init"] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/image_to_video"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
                .mount(&server)
                .await;
            let materialized = crate::proxy::materializer::MaterializedRequest {
                refs: vec![crate::proxy::materializer::MaterializedRef {
                    role: role.to_string(),
                    form: MaterializedRefForm::Url("https://example.test/first.png".to_string()),
                    time_s: None,
                }],
                cleanup: crate::proxy::materializer::Cleanup::empty(),
            };
            make_provider(&server.uri())
                .generate(&ref_schema("runway/gen4-turbo"), &make_base("a drone shot", "runway/gen4-turbo"), &make_extras(), &materialized)
                .await
                .unwrap_or_else(|e| panic!("{role}: {e}"));
            let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
            assert_eq!(body["promptImage"], "https://example.test/first.png", "{role}");
        }
    }

    /// `gen3a_turbo` was removed from the Runway API on 2026-07-30 ("Requests
    /// that use these model identifiers will fail"), and does not appear in the
    /// `/v1/image_to_video` model union in docs.dev.runwayml.com/openapi.json.
    /// The current fast model is `gen4_turbo`, which is image-to-video only.
    #[tokio::test]
    async fn sends_gen4_turbo_not_the_retired_gen3a_turbo() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/image_to_video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4-turbo");
        let base = make_base("a drone shot over a misty forest", "runway/gen4-turbo");
        let materialized = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: MaterializedRefForm::Url("https://example.test/first.png".to_string()),
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        provider
            .generate(&schema, &base, &make_extras(), &materialized)
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "gen4_turbo");
        assert_eq!(body["promptImage"], "https://example.test/first.png");
    }

    /// `/v1/text_to_video` has no `gen4_turbo` branch in
    /// docs.dev.runwayml.com/openapi.json (re-read 2026-09-11), so a
    /// prompt-only gen4_turbo request must fail here rather than be sent to an
    /// endpoint that can only reject it.
    #[tokio::test]
    async fn gen4_turbo_without_an_init_image_is_rejected_before_any_request() {
        let server = MockServer::start().await;
        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4-turbo");
        let base = make_base("a city at night", "runway/gen4-turbo");
        let err = provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect_err("gen4_turbo cannot do text-to-video");
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "got {err:?}");
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "no request may reach Runway"
        );
    }

    /// The quality tier is `gen4.5` (dot, not underscore) per the spec's
    /// discriminated-union variant title.
    #[tokio::test]
    async fn sends_gen4_5_model_id_verbatim() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/text_to_video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4.5");
        let base = make_base("a slow push through a canyon", "runway/gen4.5");
        provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "gen4.5");
    }

    /// `1280:768` / `768:1280` are gen3-era ratios. `/v1/text_to_video` takes
    /// only `1280:720 | 720:1280` for `gen4.5` (the six-value gen4 enum is
    /// `/v1/image_to_video`'s), so the default we substitute when the caller
    /// omits one must be a member of that, the narrowest enum.
    #[tokio::test]
    async fn default_ratio_is_a_member_of_the_gen4_enum() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/text_to_video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4.5");
        let base = make_base("a city at night", "runway/gen4.5");
        provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        const T2V_RATIOS: [&str; 2] = ["1280:720", "720:1280"];
        let ratio = body["ratio"].as_str().expect("ratio must always be sent");
        assert!(
            T2V_RATIOS.contains(&ratio),
            "ratio {ratio:?} is not in the gen4.5 text_to_video enum {T2V_RATIOS:?}"
        );
    }

    /// Every catalog ratio must also be a member of the vendor enum — the
    /// validator accepts anything in `allowed` and forwards it verbatim.
    #[test]
    fn catalog_video_ratios_are_all_in_the_gen4_enum() {
        const GEN4_RATIOS: [&str; 6] = [
            "1280:720", "720:1280", "1104:832", "832:1104", "960:960", "1584:672",
        ];
        for id in ["runway/gen4-turbo", "runway/gen4.5"] {
            let schema = ref_schema(id);
            let allowed: Vec<String> = match schema.params.get("aspect_ratio") {
                Some(crate::capabilities::schema::ParamSpec::AspectRatio(ar)) => ar.allowed.clone(),
                other => panic!("{id} aspect_ratio param is {other:?}, expected AspectRatio"),
            };
            assert!(!allowed.is_empty(), "{id} declares no aspect_ratio.allowed");
            for ar in allowed {
                assert!(
                    GEN4_RATIOS.contains(&ar.as_str()),
                    "{id} allows ratio {ar:?}, which Runway's gen4 enum rejects"
                );
            }
        }
    }

    /// `CANCELLED` is one of the six task states in the spec's response union.
    /// Falling through to `Pending` makes a cancelled job poll until timeout.
    #[tokio::test]
    async fn cancelled_task_reports_failed_rather_than_pending() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tasks/task-cancelled"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "task-cancelled",
                "status": "CANCELLED"
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let handle = VideoGenerationHandle {
            provider_job_id: "task-cancelled".to_string(),
            provider: "runway".to_string(),
            model: "runway/gen4-turbo".to_string(),
        };

        let poll = provider.poll_status(&handle).await.expect("poll");
        assert_eq!(poll.status, GenerationStatus::Failed);
    }

    // ─── Cancel (L18) ───────────────────────────────────────────────────────

    fn task(id: &str) -> VideoGenerationHandle {
        VideoGenerationHandle { provider_job_id: id.to_string(), provider: "runway".into(), model: "runway/gen4-turbo".into() }
    }

    #[tokio::test]
    async fn cancel_deletes_a_task_that_has_not_finished() {
        use crate::providers::CancelOutcome;
        use wiremock::matchers::header;
        for state in ["PENDING", "THROTTLED", "RUNNING"] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/tasks/t1"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1", "status": state })))
                .mount(&server)
                .await;
            Mock::given(method("DELETE"))
                .and(path("/tasks/t1"))
                .and(header("authorization", "Bearer test-key"))
                .and(header("x-runway-version", "2024-11-06"))
                .respond_with(ResponseTemplate::new(204))
                .expect(1)
                .mount(&server)
                .await;
            assert_eq!(make_provider(&server.uri()).cancel(&task("t1")).await.unwrap(), CancelOutcome::Requested, "{state}");
        }
    }

    /// For a finished task the same DELETE deletes it, which refunds nothing
    /// and throws its output away: a finished task is left alone.
    #[tokio::test]
    async fn cancel_leaves_a_finished_task_alone_and_reports_one_that_is_gone() {
        use crate::providers::CancelOutcome;
        for state in ["SUCCEEDED", "FAILED", "CANCELLED"] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/tasks/t1"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1", "status": state })))
                .mount(&server)
                .await;
            Mock::given(method("DELETE")).respond_with(ResponseTemplate::new(204)).expect(0).mount(&server).await;
            assert_eq!(make_provider(&server.uri()).cancel(&task("t1")).await.unwrap(), CancelOutcome::AlreadyFinished, "{state}");
        }
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/tasks/t2")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        assert_eq!(make_provider(&server.uri()).cancel(&task("t2")).await.unwrap(), CancelOutcome::NotFound);
    }

    // ─── Runway audit, 2026-10-30 ───────────────────────────────────────────

    /// Runway answers every submit with a 400, so only the outbound body matters.
    async fn refusing_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "stop" })))
            .mount(&server)
            .await;
        server
    }

    async fn sent_bodies(server: &MockServer) -> Vec<Value> {
        server.received_requests().await.unwrap().iter().map(|r| serde_json::from_slice(&r.body).unwrap()).collect()
    }

    fn with_init(form: MaterializedRefForm) -> MaterializedRequest {
        MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef { role: "init".into(), form, time_s: None }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    // ─── Resold models (2026-11-10) ─────────────────────────────────────────

    fn refs(roles: &[&str]) -> MaterializedRequest {
        MaterializedRequest {
            refs: roles
                .iter()
                .enumerate()
                .map(|(i, role)| crate::proxy::materializer::MaterializedRef {
                    role: role.to_string(),
                    form: MaterializedRefForm::Url(format!("https://example.com/{role}{i}.png")),
                    time_s: None,
                })
                .collect(),
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    fn extras_with(aspect: Option<&str>, resolution: Option<&str>, duration: f64) -> VideoExtras {
        let mut e = make_extras();
        e.aspect_ratio = aspect.map(str::to_string);
        e.resolution = resolution.map(str::to_string);
        e.duration_seconds = duration;
        e
    }

    /// The last request the server saw: its path and body.
    async fn last_sent(server: &MockServer) -> (String, Value) {
        let all = server.received_requests().await.unwrap();
        let r = all.last().expect("a request");
        (r.url.path().to_string(), serde_json::from_slice(&r.body).unwrap())
    }

    /// Every aspect × resolution a resold row offers reaches Runway in a shape
    /// its branch takes, in both modes: a `ratio` that is the aspect itself,
    /// `auto_<tier>`, or a size of that aspect (within 4.5%) larger at each
    /// higher tier; a `resolution` field only as one of the row's tiers.
    /// Driven through `generate()` against a server that refuses every submit.
    #[tokio::test]
    async fn every_aspect_and_resolution_a_resold_row_offers_reaches_runway_in_a_shape_it_takes() {
        let server = refusing_server().await;
        let provider = make_provider(&server.uri());
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        let rows: Vec<_> = catalog
            .for_provider("runway")
            .filter(|m| m.media_type == crate::capabilities::MediaType::Video && !m.id.starts_with("runway/gen4"))
            .cloned()
            .collect();
        assert_eq!(rows.len(), 25);
        let mut checked = 0;
        for m in &rows {
            let aspects: Vec<Option<String>> = match m.params.get("aspect_ratio") {
                Some(crate::capabilities::schema::ParamSpec::AspectRatio(a)) => a.allowed.iter().cloned().map(Some).collect(),
                _ => vec![None],
            };
            let tiers: Vec<Option<String>> = match m.params.get("resolution") {
                Some(crate::capabilities::schema::ParamSpec::String(s)) => s.enum_values.iter().cloned().map(Some).collect(),
                _ => vec![None],
            };
            for (mode, materialized) in [("/text_to_video", empty_materialized()), ("/image_to_video", refs(&["first_frame"]))] {
                for aspect in &aspects {
                    let mut last_pixels = 0.0;
                    for tier in &tiers {
                        let before = server.received_requests().await.unwrap().len();
                        let extras = extras_with(aspect.as_deref(), tier.as_deref(), 5.0);
                        let err = provider.generate(m, &make_base("a lighthouse at dusk", &m.id), &extras, &materialized).await.unwrap_err();
                        let all = server.received_requests().await.unwrap();
                        assert_eq!(all.len(), before + 1, "{} {mode} {aspect:?} {tier:?} never reached Runway: {err:?}", m.id);
                        assert_eq!(all[before].url.path(), mode, "{}", m.id);
                        let body: Value = serde_json::from_slice(&all[before].body).unwrap();
                        assert_eq!(body["model"], m.id.trim_start_matches("runway/"), "{}", m.id);
                        if let Some(r) = body["resolution"].as_str() {
                            let tier = tier.as_deref().unwrap_or_else(|| panic!("{} sent resolution {r} with none asked", m.id));
                            assert!(r.eq_ignore_ascii_case(tier), "{} {mode} sent resolution {r} for {tier}", m.id);
                        }
                        let Some(ratio) = body["ratio"].as_str() else { continue };
                        if let Some(t) = ratio.strip_prefix("auto_") {
                            assert!(tier.as_deref().is_some_and(|tier| tier.eq_ignore_ascii_case(t)), "{} {mode} sent {ratio} for {tier:?}", m.id);
                        } else if aspect.as_deref() == Some(ratio) {
                            // The aspect itself (hailuo3, grok_imagine_1_5).
                        } else {
                            let aspect = aspect.as_deref().unwrap_or_else(|| panic!("{} sent ratio {ratio} with no aspect", m.id));
                            let f = |s: &str| s.split_once(':').map(|(a, b)| (a.parse::<f64>().unwrap(), b.parse::<f64>().unwrap())).unwrap();
                            let ((w, h), (a, b)) = (f(ratio), f(aspect));
                            assert!(((w / h) / (a / b)).ln().abs() < 0.045, "{} {mode} {aspect} {tier:?} sent {ratio}", m.id);
                            assert!(w * h > last_pixels, "{} {mode} {aspect}: {tier:?} ({ratio}) is no larger than the tier below", m.id);
                            last_pixels = w * h;
                        }
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 300, "only {checked} cells checked");
    }

    /// A resold model gets exactly its own branch's fields, mode by mode.
    /// @see <https://docs.dev.runwayml.com/openapi.json> (2026-11-10)
    #[tokio::test]
    async fn a_resold_model_sends_its_own_branchs_fields() {
        let server = refusing_server().await;
        let provider = make_provider(&server.uri());
        let send = |id: &'static str, extras: VideoExtras, materialized: MaterializedRequest, seed: Option<i64>, negative: Option<&str>| {
            let provider = &provider;
            let server = &server;
            let negative = negative.map(str::to_string);
            async move {
                let mut base = make_base("a lighthouse at dusk", id);
                base.seed = seed;
                base.negative_prompt = negative;
                let _ = provider.generate(&ref_schema(id), &base, &extras, &materialized).await;
                last_sent(server).await
            }
        };

        // Veo 3.1 text-to-video: the 1080p 9:16 size, audio, seed and negativePrompt.
        let mut e = extras_with(Some("9:16"), Some("1080p"), 6.0);
        e.audio = Some(false);
        let (path, body) = send("runway/veo3.1", e, empty_materialized(), Some(5), Some("blur")).await;
        assert_eq!(path, "/text_to_video");
        assert_eq!(body, json!({ "model": "veo3.1", "promptText": "a lighthouse at dusk", "ratio": "1080:1920", "duration": 6, "audio": false, "seed": 5, "negativePrompt": "blur" }));
        // …and image-to-video from first and last frames, in that order, at its default size.
        let (path, body) = send("runway/veo3.1", extras_with(None, None, 8.0), refs(&["last_frame", "first_frame"]), None, None).await;
        assert_eq!(path, "/image_to_video");
        assert_eq!(body["promptImage"], json!([{ "uri": "https://example.com/first_frame1.png", "position": "first" }, { "uri": "https://example.com/last_frame0.png", "position": "last" }]));
        assert_eq!((body["ratio"].as_str(), body.get("audio")), (Some("1280:720"), None));

        // Kling 4.0 image-to-video: the tier as auto_<tier>, references beside the frame.
        let (path, body) = send("runway/kling_4", extras_with(None, None, 10.0), refs(&["first_frame", "reference", "reference"]), None, None).await;
        assert_eq!(path, "/image_to_video");
        assert_eq!(body["promptImage"], "https://example.com/first_frame0.png");
        assert_eq!(body["ratio"], "auto_1080p");
        assert_eq!(body["references"], json!([{ "uri": "https://example.com/reference1.png" }, { "uri": "https://example.com/reference2.png" }]));
        assert_eq!(body["duration"], 10);

        // MiniMax H3: references alone go text-to-video; the aspect itself and the tier.
        let (path, body) = send("runway/hailuo3", extras_with(None, None, 5.0), refs(&["reference", "reference", "reference"]), None, None).await;
        assert_eq!(path, "/text_to_video");
        assert_eq!(body["references"].as_array().map(Vec::len), Some(3));
        assert_eq!((body["ratio"].as_str(), body["resolution"].as_str()), (Some("16:9"), Some("768P")));

        // H3 Max: a resolution field, spelled as Runway's enum, and no ratio.
        let (_, body) = send("runway/h3_max", extras_with(None, Some("480P"), 5.0), refs(&["first_frame"]), None, None).await;
        assert_eq!((body["resolution"].as_str(), body.get("ratio")), (Some("480p"), None));

        // HappyHorse: text-to-video's tier is its size; image-to-video's a field.
        let (_, body) = send("runway/happyhorse_1_0", extras_with(Some("4:3"), Some("1080p"), 5.0), empty_materialized(), None, None).await;
        assert_eq!((body["ratio"].as_str(), body.get("resolution")), (Some("1662:1248"), None));
        let (_, body) = send("runway/happyhorse_1_0", extras_with(Some("4:3"), Some("1080p"), 5.0), refs(&["first_frame"]), None, None).await;
        assert_eq!((body.get("ratio"), body["resolution"].as_str()), (None, Some("1080p")));

        // Seedance 2.0 image-to-video: no ratio unless asked; a tier alone is
        // the default aspect at that tier.
        let (_, body) = send("runway/seedance2", extras_with(None, None, 5.0), refs(&["first_frame"]), None, None).await;
        assert!(body.get("ratio").is_none(), "{body}");
        let (_, body) = send("runway/seedance2", extras_with(None, Some("480p"), 5.0), refs(&["first_frame"]), None, None).await;
        assert_eq!(body["ratio"], "864:496");
        // Wan 3.0 image-to-video: a tier alone is auto_<tier>.
        let (_, body) = send("runway/wan3", extras_with(None, Some("1080p"), 5.0), refs(&["first_frame"]), None, None).await;
        assert_eq!(body["ratio"], "auto_1080p");

        // Kling 3.0 Pro: its one size; audio only when asked; extra can't
        // replace owned fields.
        let mut e = extras_with(None, None, 5.0);
        e.extra = Some(json!({ "cfgScale": 0.3, "ratio": "auto", "references": [], "promptImage": "x", "resolution": "4k", "audio": true, "duration": 99 }));
        let (_, body) = send("runway/kling3.0_pro", e, empty_materialized(), None, None).await;
        assert_eq!(body, json!({ "model": "kling3.0_pro", "promptText": "a lighthouse at dusk", "ratio": "1920:1080", "duration": 5, "cfgScale": 0.3 }));
    }

    /// Requests a branch cannot take are refused before any call:
    /// - references where the chosen mode has no `references` (Kling O3
    ///   without a frame; Seedance with one);
    /// - a last frame without a first ("does not support generating with only
    ///   a last frame");
    /// - a text-only request without the prompt text-to-video requires.
    /// @see <https://docs.dev.runwayml.com/openapi.json> (2026-11-10)
    #[tokio::test]
    async fn a_request_a_branch_cannot_take_is_refused_before_any_call() {
        let server = refusing_server().await;
        let provider = make_provider(&server.uri());
        for (id, prompt, materialized) in [
            ("runway/klingO3_pro", "x", refs(&["reference"])),
            ("runway/seedance2", "x", refs(&["first_frame", "reference"])),
            ("runway/veo3.1", "x", refs(&["last_frame"])),
            ("runway/veo3.1", "", empty_materialized()),
        ] {
            let err = provider.generate(&ref_schema(id), &make_base(prompt, id), &extras_with(None, None, 5.0), &materialized).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{id}: {err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty(), "a refused request reached Runway");
    }

    /// #16: `extra` adds Runway's own knobs but never replaces what litegen sends.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends() {
        let server = refusing_server().await;
        let mut base = make_base("a slow pan", "runway/gen4.5");
        base.seed = Some(3);
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "model": "gen4_turbo", "promptText": "other", "promptImage": "https://x/y.png",
            "duration": 10, "ratio": "960:960", "seed": 9, "contentModeration": { "publicFigureThreshold": "low" }
        }));
        let _ = make_provider(&server.uri()).generate(&ref_schema("runway/gen4.5"), &base, &extras, &empty_materialized()).await;
        let body = &sent_bodies(&server).await[0];
        assert_eq!(body["model"], "gen4.5");
        assert_eq!(body["promptText"], "a slow pan");
        assert!(body.get("promptImage").is_none(), "{body}");
        assert_eq!(body["duration"], 5);
        assert_eq!(body["ratio"], "1280:720");
        assert_eq!(body["seed"], 3);
        assert_eq!(body["contentModeration"]["publicFigureThreshold"], "low");
    }

    /// #29: gen4_turbo's `promptText` is optional (`required: ["promptImage",
    /// "ratio","model"]`) and "A non-empty string", so no prompt sends none.
    /// @see <https://docs.dev.runwayml.com/openapi.json> (POST /v1/image_to_video, 2026-10-30)
    #[tokio::test]
    async fn gen4_turbo_without_a_prompt_sends_no_prompt_text() {
        let server = refusing_server().await;
        let refs = with_init(MaterializedRefForm::Url("https://example.com/first.png".into()));
        let _ = make_provider(&server.uri())
            .generate(&ref_schema("runway/gen4-turbo"), &make_base("", "runway/gen4-turbo"), &make_extras(), &refs)
            .await;
        let body = &sent_bodies(&server).await[0];
        assert!(body.get("promptText").is_none(), "{body}");
        assert_eq!(body["promptImage"], "https://example.com/first.png");
    }

    /// #17, #18: a submit that timed out may have been accepted and billed, so
    /// it is never sent again; a 429 was turned away and may be.
    /// @see <https://docs.dev.runwayml.com/errors/errors.md> (2026-10-30)
    #[tokio::test]
    async fn a_timed_out_submit_is_never_sent_again_and_a_rate_limited_one_may_be() {
        let slow = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t" })).set_delay(std::time::Duration::from_secs(5)))
            .mount(&slow)
            .await;
        let mut p = make_provider(&slow.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let schema = ref_schema("runway/gen4.5");
        let err = p.generate(&schema, &make_base("x", "runway/gen4.5"), &make_extras(), &empty_materialized()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");

        let limited = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "error": "Too many requests" })))
            .mount(&limited)
            .await;
        let err = make_provider(&limited.uri())
            .generate(&schema, &make_base("x", "runway/gen4.5"), &make_extras(), &empty_materialized())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: true, status_code: Some(429), .. }), "{err:?}");
    }

    /// #21: a base64 first frame goes with its own type, and a GIF ("GIF images
    /// are not supported") is refused before any call.
    /// @see <https://docs.dev.runwayml.com/assets/inputs.md> (2026-10-30)
    #[tokio::test]
    async fn a_frame_goes_with_its_own_type_and_one_runway_cannot_take_is_refused() {
        use base64::Engine;
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let server = refusing_server().await;
        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4-turbo");
        let base = make_base("x", "runway/gen4-turbo");
        let webp = with_init(MaterializedRefForm::Base64(b64(b"RIFF\x24\x00\x00\x00WEBPVP8 ")));
        let _ = provider.generate(&schema, &base, &make_extras(), &webp).await;
        let sent = sent_bodies(&server).await;
        assert!(sent[0]["promptImage"].as_str().unwrap().starts_with("data:image/webp;base64,"), "{}", sent[0]);

        let gif = with_init(MaterializedRefForm::Base64(b64(b"GIF89a\x01\x00\x01\x00")));
        let err = provider.generate(&schema, &base, &make_extras(), &gif).await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        assert_eq!(server.received_requests().await.unwrap().len(), 1, "a GIF reached Runway");
    }

    /// #13, #20, #25: a poll Runway answers with an error is that error; a
    /// running task's `progress` is a fraction ("minimum": 0, "maximum": 1);
    /// a success without an output fails rather than completing with nothing.
    /// @see <https://docs.dev.runwayml.com/openapi.json> (GET /v1/tasks/{id}, 2026-10-30)
    #[tokio::test]
    async fn a_poll_says_what_runway_said() {
        let server = MockServer::start().await;
        for (id, status, body) in [
            ("gone", 404, json!({ "error": "Task not found" })),
            ("running", 200, json!({ "id": "running", "status": "RUNNING", "progress": 0.42 })),
            ("empty", 200, json!({ "id": "empty", "status": "SUCCEEDED", "output": [] })),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/tasks/{id}")))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .mount(&server)
                .await;
        }
        let provider = make_provider(&server.uri());
        let handle = |id: &str| VideoGenerationHandle { provider_job_id: id.into(), provider: "runway".into(), model: "runway/gen4.5".into() };

        let err = provider.poll_status(&handle("gone")).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { status_code: Some(404), retryable: false, .. }), "{err:?}");

        let running = provider.poll_status(&handle("running")).await.unwrap();
        assert_eq!((running.status, running.progress), (GenerationStatus::Processing, 42));

        let empty = provider.poll_status(&handle("empty")).await.unwrap();
        assert_eq!(empty.status, GenerationStatus::Failed);
        assert!(empty.error.as_deref().is_some_and(|e| e.contains("no output")), "{:?}", empty.error);
    }
}
