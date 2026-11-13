use async_trait::async_trait;
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

/// Runway image generation provider: Runway's Gen-4 image models, and the
/// third-party models Runway resells on the same endpoint ([`RESOLD`]: GPT
/// Image, Gemini and Nano Banana, Seedream, Grok Imagine, Bria FIBO, Ideogram,
/// Muse), each sent its own branch of the request union.
///
/// Shares the Runway developer API host (`api.dev.runwayml.com/v1`), Bearer auth,
/// and mandatory `X-Runway-Version` header with the Runway video provider.
/// Asynchronous: `POST /text_to_image` returns a task id; poll `GET /tasks/{id}`
/// until `SUCCEEDED`, then download the first `output` URL.
///
/// @see <https://docs.dev.runwayml.com/guides/using-the-api/> — text_to_image
///   Verbatim: "curl -X POST https://api.dev.runwayml.com/v1/text_to_image"
///   Verbatim: "\"promptText\": \"@EiffelTower painted in the style of @StarryNight\", \"model\": \"gen4_image\", \"ratio\": \"1920:1080\","
///   Verbatim: "-H \"Authorization: Bearer $RUNWAYML_API_SECRET\" -H \"X-Runway-Version: 2024-11-06\""
pub struct RunwayImageProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
    /// Result downloads: no redirects, so a URL that passed the public-address
    /// check cannot bounce into a private one.
    asset_client: Client,
    /// Whether a result URL must resolve to a public address. Only the tests,
    /// whose mock vendor runs on loopback, turn it off.
    pub(crate) public_assets_only: bool,
}

const RUNWAY_VERSION: &str = "2024-11-06";

/// What litegen itself sends to `POST /text_to_image`; `extra` cannot replace it.
const OWNED_FIELDS: &[&str] = &["promptText", "model", "ratio", "seed", "referenceImages"];

/// What litegen sends a resold model's branch; `extra` cannot replace it.
/// `outputCount` is litegen's too: one image a call, never a caller's count.
const RESOLD_OWNED_FIELDS: &[&str] =
    &["promptText", "model", "ratio", "seed", "referenceImages", "quality", "background", "outputFormat", "maskUri", "outputCount"];

/// One size a resold model takes: (litegen `aspect_ratio`, `resolution` tier,
/// or "" for a model with one tier, Runway `ratio`).
type Size = (&'static str, &'static str, &'static str);

/// A third-party image model Runway sells on `POST /v1/text_to_image`: its
/// branch of the request union, read 2026-11-10. Prices, prompt limits and
/// reference caps are on its catalog row.
/// @see <https://docs.dev.runwayml.com/openapi.json> (2026-11-10)
pub(crate) struct Resold {
    /// Runway's `model`, and the catalog id's suffix.
    wire: &'static str,
    /// Its `ratio` sizes by aspect and tier, generated from the branch's enum
    /// (`.superpowers/sdd/2026-11-04-tokenhub-bedrock-fal/runway-gen/gen-image.mjs`):
    /// each size takes the named aspect within 4.5% of it and the tier
    /// nearest its pixels (GPT Image: Runway's own 1K / 2K / 4K lists).
    /// Empty when the branch's `ratio` is only "auto" (Ideogram Vector).
    sizes: &'static [Size],
    /// Whether `referenceImages[]` items take a `tag`: the GPT Image and
    /// Gemini branches do; the others set `additionalProperties: false`.
    tags: bool,
    /// Fewer images than this take no `ratio`. Bria FIBO Edit: "Choosing a
    /// size requires at least 2 reference images; a single image always keeps
    /// its own framing".
    ratio_min_refs: usize,
    /// `extra` switches that need a reference image: Grok's `edit` and
    /// Ideogram 4.5's `preciseEdit` ("Requires at least one reference image").
    needs_ref: &'static [&'static str],
}

/// gpt_image_2, gpt_image_2_5_flare, gpt_image_2_5_sunburst, gpt_image_2_6_flare, gpt_image_2_6_sunburst.
const GPT_IMAGE_2_SIZES: &[Size] = &[
    ("1:1", "1K", "1920:1920"), ("2:3", "1K", "1280:1920"), ("3:2", "1K", "1920:1280"), ("3:4", "1K", "1440:1920"), ("4:3", "1K", "1920:1440"), ("4:5", "1K", "1536:1920"), ("5:4", "1K", "1920:1536"), ("9:16", "1K", "1088:1920"), ("16:9", "1K", "1920:1088"), ("21:9", "1K", "2048:880"),
    ("1:1", "2K", "2560:2560"), ("2:3", "2K", "1712:2560"), ("3:2", "2K", "2560:1712"), ("3:4", "2K", "1920:2560"), ("4:3", "2K", "2560:1920"), ("4:5", "2K", "2048:2560"), ("5:4", "2K", "2560:2048"), ("9:16", "2K", "1440:2560"), ("16:9", "2K", "2560:1440"), ("21:9", "2K", "2912:1248"),
    ("1:1", "4K", "2880:2880"), ("2:3", "4K", "2336:3504"), ("3:2", "4K", "3504:2336"), ("3:4", "4K", "2448:3264"), ("4:3", "4K", "3264:2448"), ("4:5", "4K", "2560:3200"), ("5:4", "4K", "3200:2560"), ("9:16", "4K", "2160:3840"), ("16:9", "4K", "3840:2160"), ("21:9", "4K", "3840:1648"),
];

/// gemini_2.5_flash.
const GEMINI_2_5_FLASH_SIZES: &[Size] = &[
    ("1:1", "", "1024:1024"), ("2:3", "", "832:1248"), ("3:2", "", "1248:832"), ("3:4", "", "864:1184"), ("4:3", "", "1184:864"), ("4:5", "", "896:1152"), ("5:4", "", "1152:896"), ("9:16", "", "768:1344"), ("16:9", "", "1344:768"), ("21:9", "", "1536:672"),
];

/// gemini_image3_pro.
const GEMINI_IMAGE3_PRO_SIZES: &[Size] = &[
    ("1:1", "1K", "1024:1024"), ("2:3", "1K", "832:1248"), ("3:2", "1K", "1248:832"), ("3:4", "1K", "864:1184"), ("4:3", "1K", "1184:864"), ("4:5", "1K", "896:1152"), ("5:4", "1K", "1152:896"), ("9:16", "1K", "768:1344"), ("16:9", "1K", "1344:768"), ("21:9", "1K", "1536:672"),
    ("1:1", "2K", "2048:2048"), ("2:3", "2K", "1696:2528"), ("3:2", "2K", "2528:1696"), ("3:4", "2K", "1792:2400"), ("4:3", "2K", "2400:1792"), ("4:5", "2K", "1856:2304"), ("5:4", "2K", "2304:1856"), ("9:16", "2K", "1536:2752"), ("16:9", "2K", "2752:1536"), ("21:9", "2K", "3168:1344"),
    ("1:1", "4K", "4096:4096"), ("2:3", "4K", "3392:5056"), ("3:2", "4K", "5056:3392"), ("3:4", "4K", "3584:4800"), ("4:3", "4K", "4800:3584"), ("4:5", "4K", "3712:4608"), ("5:4", "4K", "4608:3712"), ("9:16", "4K", "3072:5504"), ("16:9", "4K", "5504:3072"), ("21:9", "4K", "6336:2688"),
];

/// gemini_image3.1_flash.
const GEMINI_IMAGE3_1_FLASH_SIZES: &[Size] = &[
    ("1:1", "0.5K", "512:512"), ("2:3", "0.5K", "416:624"), ("3:2", "0.5K", "624:416"), ("3:4", "0.5K", "432:592"), ("4:3", "0.5K", "592:432"), ("4:5", "0.5K", "448:576"), ("5:4", "0.5K", "576:448"), ("9:16", "0.5K", "384:672"), ("16:9", "0.5K", "672:384"), ("21:9", "0.5K", "768:336"), ("1:4", "0.5K", "256:1024"), ("4:1", "0.5K", "1024:256"), ("1:8", "0.5K", "176:1408"), ("8:1", "0.5K", "1408:176"),
    ("1:1", "1K", "1024:1024"), ("2:3", "1K", "832:1248"), ("3:2", "1K", "1248:832"), ("3:4", "1K", "864:1184"), ("4:3", "1K", "1184:864"), ("4:5", "1K", "896:1152"), ("5:4", "1K", "1152:896"), ("9:16", "1K", "768:1344"), ("16:9", "1K", "1344:768"), ("21:9", "1K", "1536:672"), ("1:4", "1K", "512:2048"), ("4:1", "1K", "2048:512"), ("1:8", "1K", "352:2816"), ("8:1", "1K", "2816:352"),
    ("1:1", "2K", "2048:2048"), ("2:3", "2K", "1696:2528"), ("3:2", "2K", "2528:1696"), ("3:4", "2K", "1792:2400"), ("4:3", "2K", "2400:1792"), ("4:5", "2K", "1856:2304"), ("5:4", "2K", "2304:1856"), ("9:16", "2K", "1536:2752"), ("16:9", "2K", "2752:1536"), ("21:9", "2K", "3168:1344"), ("1:4", "2K", "1024:4096"), ("4:1", "2K", "4096:1024"), ("1:8", "2K", "704:5632"), ("8:1", "2K", "5632:704"),
    ("1:1", "4K", "4096:4096"), ("2:3", "4K", "3392:5056"), ("3:2", "4K", "5056:3392"), ("3:4", "4K", "3584:4800"), ("4:3", "4K", "4800:3584"), ("4:5", "4K", "3712:4608"), ("5:4", "4K", "4608:3712"), ("9:16", "4K", "3072:5504"), ("16:9", "4K", "5504:3072"), ("21:9", "4K", "6336:2688"), ("1:4", "4K", "2048:8192"), ("4:1", "4K", "8192:2048"), ("1:8", "4K", "1408:11264"), ("8:1", "4K", "11264:1408"),
];

/// gemini_image3.1_flash_lite.
const GEMINI_IMAGE3_1_FLASH_LITE_SIZES: &[Size] = &[
    ("1:1", "", "1024:1024"), ("2:3", "", "848:1264"), ("3:2", "", "1264:848"), ("3:4", "", "896:1200"), ("4:3", "", "1200:896"), ("4:5", "", "928:1152"), ("5:4", "", "1152:928"), ("9:16", "", "768:1376"), ("16:9", "", "1376:768"), ("21:9", "", "1584:672"),
];

/// nano_banana_2.1.
const NANO_BANANA_2_1_SIZES: &[Size] = &[
    ("1:1", "1K", "1024:1024"), ("2:3", "1K", "848:1264"), ("3:2", "1K", "1264:848"), ("3:4", "1K", "896:1200"), ("4:3", "1K", "1200:896"), ("4:5", "1K", "928:1152"), ("5:4", "1K", "1152:928"), ("9:16", "1K", "768:1376"), ("16:9", "1K", "1376:768"), ("21:9", "1K", "1584:672"), ("1:4", "1K", "512:2064"), ("4:1", "1K", "2064:512"), ("1:8", "1K", "352:2928"), ("8:1", "1K", "2928:352"),
    ("1:1", "2K", "2048:2048"), ("2:3", "2K", "1696:2528"), ("3:2", "2K", "2528:1696"), ("3:4", "2K", "1792:2400"), ("4:3", "2K", "2400:1792"), ("4:5", "2K", "1856:2304"), ("5:4", "2K", "2304:1856"), ("9:16", "2K", "1536:2752"), ("16:9", "2K", "2752:1536"), ("21:9", "2K", "3168:1344"), ("1:4", "2K", "1024:4128"), ("4:1", "2K", "4128:1024"), ("1:8", "2K", "704:5856"), ("8:1", "2K", "5856:704"),
    ("1:1", "4K", "4096:4096"), ("2:3", "4K", "3392:5056"), ("3:2", "4K", "5056:3392"), ("3:4", "4K", "3584:4800"), ("4:3", "4K", "4800:3584"), ("4:5", "4K", "3712:4608"), ("5:4", "4K", "4608:3712"), ("9:16", "4K", "3072:5504"), ("16:9", "4K", "5504:3072"), ("21:9", "4K", "6336:2688"), ("1:4", "4K", "2048:8256"), ("4:1", "4K", "8256:2048"), ("1:8", "4K", "1408:11712"), ("8:1", "4K", "11712:1408"),
];

/// muse_image.
const MUSE_IMAGE_SIZES: &[Size] = &[
    ("1:1", "", "1600:1600"), ("2:3", "", "1280:1920"), ("3:2", "", "1920:1280"), ("3:4", "", "1344:1792"), ("4:3", "", "1792:1344"), ("9:16", "", "1152:2016"), ("16:9", "", "2016:1152"), ("21:9", "", "2352:1008"),
];

/// seedream5_pro.
const SEEDREAM5_PRO_SIZES: &[Size] = &[
    ("1:1", "1K", "1024:1024"), ("2:3", "1K", "864:1296"), ("3:2", "1K", "1296:864"), ("3:4", "1K", "896:1184"), ("4:3", "1K", "1184:896"), ("9:16", "1K", "768:1376"), ("16:9", "1K", "1376:768"),
    ("1:1", "2K", "2048:2048"), ("2:3", "2K", "1664:2496"), ("3:2", "2K", "2496:1664"), ("3:4", "2K", "1728:2304"), ("4:3", "2K", "2304:1728"), ("9:16", "2K", "1530:2720"), ("16:9", "2K", "2720:1530"),
];

/// seedream5_lite.
const SEEDREAM5_LITE_SIZES: &[Size] = &[
    ("1:1", "2K", "2048:2048"), ("2:3", "2K", "1664:2496"), ("3:2", "2K", "2496:1664"), ("3:4", "2K", "1728:2304"), ("4:3", "2K", "2304:1728"), ("9:16", "2K", "1600:2848"), ("16:9", "2K", "2848:1600"), ("21:9", "2K", "3136:1344"),
    ("1:1", "3K", "3072:3072"), ("2:3", "3K", "2496:3744"), ("3:2", "3K", "3744:2496"), ("3:4", "3K", "2592:3456"), ("4:3", "3K", "3456:2592"), ("9:16", "3K", "2304:4096"), ("16:9", "3K", "4096:2304"), ("21:9", "3K", "4704:2016"),
];

/// seedream5_flash.
const SEEDREAM5_FLASH_SIZES: &[Size] = &[
    ("1:1", "1K", "1024:1024"), ("2:3", "1K", "832:1248"), ("3:2", "1K", "1248:832"), ("3:4", "1K", "896:1184"), ("4:3", "1K", "1184:896"), ("9:16", "1K", "768:1376"), ("16:9", "1K", "1376:768"), ("21:9", "1K", "1568:672"),
    ("1:1", "2K", "2048:2048"), ("2:3", "2K", "1664:2496"), ("3:2", "2K", "2496:1664"), ("3:4", "2K", "1728:2304"), ("4:3", "2K", "2304:1728"), ("9:16", "2K", "1600:2848"), ("16:9", "2K", "2848:1600"), ("21:9", "2K", "3136:1344"),
];

/// grok_imagine_image_2.
const GROK_IMAGINE_IMAGE_2_SIZES: &[Size] = &[
    ("1:1", "1K", "1024:1024"), ("2:3", "1K", "832:1248"), ("3:2", "1K", "1248:832"), ("3:4", "1K", "864:1152"), ("4:3", "1K", "1152:864"), ("9:16", "1K", "720:1280"), ("16:9", "1K", "1280:720"), ("2:1", "1K", "1408:704"), ("1:2", "1K", "704:1408"), ("19.5:9", "1K", "1248:576"), ("9:19.5", "1K", "576:1248"), ("20:9", "1K", "1280:576"), ("9:20", "1K", "576:1280"),
    ("1:1", "2K", "2048:2048"), ("2:3", "2K", "1664:2496"), ("3:2", "2K", "2496:1664"), ("3:4", "2K", "1776:2368"), ("4:3", "2K", "2368:1776"), ("9:16", "2K", "1584:2816"), ("16:9", "2K", "2816:1584"), ("2:1", "2K", "2912:1456"), ("1:2", "2K", "1456:2912"), ("19.5:9", "2K", "2912:1344"), ("9:19.5", "2K", "1344:2912"), ("20:9", "2K", "3200:1440"), ("9:20", "2K", "1440:3200"),
];

/// bria_fibo_1_5.
const BRIA_FIBO_1_5_SIZES: &[Size] = &[
    ("1:1", "1K", "1024:1024"), ("2:3", "1K", "768:1152"), ("3:2", "1K", "1152:768"), ("3:4", "1K", "768:1024"), ("4:3", "1K", "1024:768"), ("4:5", "1K", "768:960"), ("5:4", "1K", "960:768"), ("9:16", "1K", "576:1024"), ("16:9", "1K", "1024:576"),
    ("1:1", "2K", "2048:2048"), ("2:3", "2K", "1536:2304"), ("3:2", "2K", "2304:1536"), ("3:4", "2K", "1536:2048"), ("4:3", "2K", "2048:1536"), ("4:5", "2K", "1536:1920"), ("5:4", "2K", "1920:1536"), ("9:16", "2K", "1152:2048"), ("16:9", "2K", "2048:1152"),
];

/// bria_fibo_edit_1_5.
const BRIA_FIBO_EDIT_1_5_SIZES: &[Size] = &[
    ("1:1", "", "1024:1024"), ("2:3", "", "768:1152"), ("3:2", "", "1152:768"), ("3:4", "", "768:1024"), ("4:3", "", "1024:768"), ("4:5", "", "768:960"), ("5:4", "", "960:768"), ("9:16", "", "576:1024"), ("16:9", "", "1024:576"),
];

/// ideogram_4_0.
const IDEOGRAM_4_0_SIZES: &[Size] = &[
    ("1:1", "", "2048:2048"), ("2:3", "", "1664:2496"), ("3:2", "", "2496:1664"), ("3:4", "", "1728:2304"), ("4:3", "", "2304:1728"), ("9:16", "", "1440:2560"), ("16:9", "", "2560:1440"), ("12:5", "", "3072:1280"), ("5:12", "", "1280:3072"),
];

/// ideogram_4_5.
const IDEOGRAM_4_5_SIZES: &[Size] = &[
    ("1:1", "1K", "1024:1024"), ("2:3", "1K", "832:1248"), ("3:2", "1K", "1248:832"), ("3:4", "1K", "864:1152"), ("4:3", "1K", "1152:864"), ("4:5", "1K", "896:1120"), ("5:4", "1K", "1120:896"), ("9:16", "1K", "720:1280"), ("16:9", "1K", "1280:720"), ("2:1", "1K", "1440:720"), ("1:2", "1K", "720:1440"),
    ("1:1", "2K", "2048:2048"), ("2:3", "2K", "1664:2496"), ("3:2", "2K", "2496:1664"), ("3:4", "2K", "1728:2304"), ("4:3", "2K", "2304:1728"), ("4:5", "2K", "1792:2240"), ("5:4", "2K", "2240:1792"), ("9:16", "2K", "1440:2560"), ("16:9", "2K", "2560:1440"), ("2:1", "2K", "2880:1440"), ("1:2", "2K", "1440:2880"),
];

/// Every resold image model litegen carries (models/runway.yaml). Not
/// carried: recraft_v4_1_flash and recraft_v4_1_vector, which have no price
/// on the pricing guide.
/// @see <https://docs.dev.runwayml.com/guides/pricing.md> (2026-11-10)
const RESOLD: &[Resold] = &[
    Resold { wire: "gpt_image_2", sizes: GPT_IMAGE_2_SIZES, tags: true, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "gpt_image_2_5_flare", sizes: GPT_IMAGE_2_SIZES, tags: true, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "gpt_image_2_5_sunburst", sizes: GPT_IMAGE_2_SIZES, tags: true, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "gpt_image_2_6_flare", sizes: GPT_IMAGE_2_SIZES, tags: true, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "gpt_image_2_6_sunburst", sizes: GPT_IMAGE_2_SIZES, tags: true, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "gemini_2.5_flash", sizes: GEMINI_2_5_FLASH_SIZES, tags: true, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "gemini_image3_pro", sizes: GEMINI_IMAGE3_PRO_SIZES, tags: true, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "gemini_image3.1_flash", sizes: GEMINI_IMAGE3_1_FLASH_SIZES, tags: true, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "gemini_image3.1_flash_lite", sizes: GEMINI_IMAGE3_1_FLASH_LITE_SIZES, tags: true, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "nano_banana_2.1", sizes: NANO_BANANA_2_1_SIZES, tags: true, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "muse_image", sizes: MUSE_IMAGE_SIZES, tags: false, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "seedream5_pro", sizes: SEEDREAM5_PRO_SIZES, tags: false, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "seedream5_lite", sizes: SEEDREAM5_LITE_SIZES, tags: false, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "seedream5_flash", sizes: SEEDREAM5_FLASH_SIZES, tags: false, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "grok_imagine_image_2", sizes: GROK_IMAGINE_IMAGE_2_SIZES, tags: false, ratio_min_refs: 0, needs_ref: &["edit"] },
    Resold { wire: "bria_fibo_1_5", sizes: BRIA_FIBO_1_5_SIZES, tags: false, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "bria_fibo_edit_1_5", sizes: BRIA_FIBO_EDIT_1_5_SIZES, tags: false, ratio_min_refs: 2, needs_ref: &[] },
    Resold { wire: "ideogram_4_0", sizes: IDEOGRAM_4_0_SIZES, tags: false, ratio_min_refs: 0, needs_ref: &[] },
    Resold { wire: "ideogram_4_5", sizes: IDEOGRAM_4_5_SIZES, tags: false, ratio_min_refs: 0, needs_ref: &["preciseEdit"] },
    Resold { wire: "ideogram_vector", sizes: &[], tags: false, ratio_min_refs: 0, needs_ref: &[] },
];

/// Which request a catalog id takes.
enum Native {
    /// Runway's own Gen-4 image models.
    Gen4(&'static str),
    /// A resold model's branch.
    Resold(&'static Resold),
}

/// Whether bytes are an SVG document (Ideogram Vector "returns one SVG per
/// request"), which the image sniffer doesn't know.
fn is_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    (head.starts_with("<?xml") || head.starts_with("<svg")) && head.contains("<svg")
}

/// The longest data URI Runway takes: `"maxLength": 5242880` ("to fit an
/// image within the 5MB limit, the binary file must be 3.3MB or less").
/// @see <https://docs.dev.runwayml.com/assets/inputs.md> (2026-10-30)
const MAX_DATA_URI_CHARS: usize = 5_242_880;

/// A reference as Runway takes it, for both the image and the video
/// adapter: a URL, or a data URI of a JPEG, PNG or WebP within
/// [`MAX_DATA_URI_CHARS`] ("Data URIs are accepted in any field where a URL is
/// accepted"; "GIF images are not supported"). Anything else is refused
/// before any call. `None` for a form Runway has no field for.
/// @see <https://docs.dev.runwayml.com/assets/inputs.md> (2026-10-30)
pub(crate) fn runway_uri(form: &MaterializedRefForm) -> Result<Option<String>, ProviderError> {
    let uri = match form {
        MaterializedRefForm::Url(u) => u.clone(),
        MaterializedRefForm::Base64(b64) => crate::proxy::materializer::base64_data_uri(b64, None),
        MaterializedRefForm::MultipartField { .. } => return Ok(None),
    };
    if let Some(rest) = uri.strip_prefix("data:") {
        let mime = rest.split([';', ',']).next().unwrap_or("");
        if !matches!(mime, "image/jpeg" | "image/jpg" | "image/png" | "image/webp") {
            return Err(ProviderError::InvalidRequest(format!(
                "runway takes a JPEG, PNG or WebP reference, not {}",
                if mime.is_empty() { "an untyped one" } else { mime }
            )));
        }
        if uri.len() > MAX_DATA_URI_CHARS {
            return Err(ProviderError::InvalidRequest(format!(
                "runway takes a reference of at most {MAX_DATA_URI_CHARS} characters as a data URI (3.3 MB of image); this one is {}",
                uri.len()
            )));
        }
    }
    Ok(Some(uri))
}

impl RunwayImageProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(180), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Self::api_client(&Default::default()),
            asset_client: crate::util::ssrf::public_only_download_client(
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
            .unwrap_or("https://api.dev.runwayml.com/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("runway".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// The Runway model for a litegen id; `None` for an id with no arm, which
    /// is refused rather than billed as another model (L13).
    fn resolve_model(model_id: &str) -> Option<Native> {
        match model_id.strip_prefix("runway/").unwrap_or(model_id) {
            "gen4_image_turbo" | "gen4-image-turbo" => Some(Native::Gen4("gen4_image_turbo")),
            "gen4_image" | "gen4-image" => Some(Native::Gen4("gen4_image")),
            id => RESOLD.iter().find(|r| r.wire == id).map(Native::Resold),
        }
    }

    /// The body of a Gen-4 image request.
    fn gen4_body(
        &self,
        native: &'static str,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<Value, ProviderError> {
        let mut body = json!({
            "promptText": base.prompt,
            "model": crate::providers::wire_model(self.config.as_ref(), &model.id, native),
            "ratio": Self::resolve_ratio(extras),
        });
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }

        // Reference images: {uri, tag}. uri may be a URL or a data URI of the
        // image's own type.
        let mut refs: Vec<Value> = Vec::new();
        for (i, r) in materialized.refs.iter().enumerate() {
            let tag = if r.role.is_empty() || r.role == "init" { format!("ref{i}") } else { r.role.clone() };
            if let Some(uri) = runway_uri(&r.form)? {
                refs.push(json!({ "uri": uri, "tag": tag }));
            }
        }
        if !refs.is_empty() {
            body["referenceImages"] = Value::Array(refs);
        }

        if let Some(obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
                obj.insert(k.clone(), v.clone());
            }
        }
        Ok(body)
    }

    /// A resold model's `ratio`: the size at the asked (else the row's
    /// default) aspect and tier. A row with no default aspect (its branch's
    /// `ratio` is optional) sends none unless an aspect or a tier is asked
    /// for: an edit then keeps its image's size. A tier alone is 1:1; an
    /// aspect alone is at the row's default tier, else the lowest. `None` for
    /// a branch whose `ratio` is only "auto".
    fn resold_ratio(r: &Resold, model: &ModelSchema, extras: &ImageExtras) -> Result<Option<&'static str>, ProviderError> {
        use crate::capabilities::schema::ParamSpec;
        if r.sizes.is_empty() {
            return Ok(None);
        }
        let default = |param: &str| match model.params.get(param) {
            Some(ParamSpec::AspectRatio(a)) => a.default.clone(),
            Some(ParamSpec::String(s)) => s.default.clone(),
            _ => None,
        };
        let default_aspect = default("aspect_ratio");
        if extras.aspect_ratio.is_none() && extras.resolution.is_none() && default_aspect.is_none() {
            return Ok(None);
        }
        let aspect = extras.aspect_ratio.clone().or(default_aspect).unwrap_or_else(|| "1:1".to_string());
        let tier = extras.resolution.clone().or_else(|| default("resolution")).unwrap_or_else(|| r.sizes[0].1.to_string());
        r.sizes
            .iter()
            .find(|(a, t, _)| *a == aspect && *t == tier)
            .map(|size| Some(size.2))
            .ok_or_else(|| ProviderError::InvalidRequest(format!("{} has no {aspect} size at {tier:?}", model.id)))
    }

    /// The body of a resold model's branch: exactly its own fields.
    fn resold_body(
        &self,
        r: &Resold,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<Value, ProviderError> {
        let extra = extras.extra.as_ref().and_then(Value::as_object);
        let switched_on = |key: &str| extra.and_then(|e| e.get(key)) == Some(&Value::Bool(true));
        let mut images = Vec::new();
        let mut mask = None;
        for rf in &materialized.refs {
            let Some(uri) = runway_uri(&rf.form)? else { continue };
            if rf.role == "mask" {
                // Ideogram 4.5 `maskUri`: "A black-and-white mask the same size
                // as the first reference image" (converted to black-edits by
                // the materializer, the row's convention).
                mask = Some(uri);
            } else if r.tags {
                images.push(json!({ "uri": uri, "tag": format!("ref{}", images.len()) }));
            } else {
                images.push(json!({ "uri": uri }));
            }
        }
        if let Some(key) = r.needs_ref.iter().find(|k| switched_on(k)).filter(|_| images.is_empty()) {
            return Err(ProviderError::InvalidRequest(format!("{}: `{key}` needs a reference image", model.id)));
        }
        // Ideogram 4.5: "`very_low` requires reference images and is not
        // available with `preciseEdit`".
        if extras.quality.as_deref() == Some("very_low") && (images.is_empty() || switched_on("preciseEdit")) {
            return Err(ProviderError::InvalidRequest(format!(
                "{}: quality very_low needs a reference image and no preciseEdit",
                model.id
            )));
        }
        let ratio = Self::resold_ratio(r, model, extras)?;
        if ratio.is_some() && images.len() < r.ratio_min_refs {
            return Err(ProviderError::InvalidRequest(format!(
                "{} keeps a single image's framing: an aspect_ratio needs at least {} images",
                model.id, r.ratio_min_refs
            )));
        }

        // `extra` first, so nothing below can be replaced by it.
        let mut body = json!({});
        for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| RESOLD_OWNED_FIELDS.contains(&k)) {
            body[k] = v.clone();
        }
        body["promptText"] = Value::String(base.prompt.clone());
        body["model"] = Value::String(crate::providers::wire_model(self.config.as_ref(), &model.id, r.wire).to_string());
        if let Some(ratio) = ratio {
            body["ratio"] = Value::String(ratio.to_string());
        }
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        // Each reaches only a branch whose row declares it (the validator).
        for (field, value) in [("quality", &extras.quality), ("background", &extras.background), ("outputFormat", &extras.output_format)] {
            if let Some(v) = value {
                body[field] = Value::String(v.clone());
            }
        }
        if !images.is_empty() {
            body["referenceImages"] = Value::Array(images);
        }
        if let Some(mask) = mask {
            body["maskUri"] = Value::String(mask);
        }
        Ok(body)
    }

    /// The exact `ratio` enum shared by `gen4_image` and `gen4_image_turbo` on
    /// `POST /v1/text_to_image`. Anything outside this set is a 400.
    /// @see <https://docs.dev.runwayml.com/openapi.json>
    pub(crate) const RATIOS: [&'static str; 16] = [
        "1024:1024", "1080:1080", "1168:880", "1360:768", "1440:1080", "1080:1440",
        "1808:768", "1920:1080", "1080:1920", "2112:912", "1280:720", "720:1280",
        "720:720", "960:720", "720:960", "1680:720",
    ];

    /// Map a unified size / aspect ratio to a Runway `ratio` (pixel WxH string).
    ///
    /// Every return value is a member of [`Self::RATIOS`]. A caller-supplied
    /// `size` is snapped to the closest enum member by aspect ratio rather than
    /// forwarded verbatim — `1000x800` is not a legal Runway ratio. Note there
    /// is no true 3:2 pair in the enum, which is why `3:2` / `2:3` are not in
    /// any Runway model's `allowed` list.
    fn resolve_ratio(extras: &ImageExtras) -> String {
        if let Some(size) = extras.size.as_deref() {
            if let Some((w, h)) = size.split_once('x') {
                if let (Ok(w), Ok(h)) = (w.parse::<f64>(), h.parse::<f64>()) {
                    if h > 0.0 {
                        return Self::closest_ratio(w / h).to_string();
                    }
                }
            }
        }
        match extras.aspect_ratio.as_deref() {
            Some("9:16") => "1080:1920",
            Some("1:1") => "1024:1024",
            Some("4:3") => "1440:1080",
            Some("3:4") => "1080:1440",
            Some("21:9") => "2112:912",
            Some("16:9") => "1920:1080",
            Some(other) => Self::closest_ratio(Self::parse_ratio(other).unwrap_or(16.0 / 9.0)),
            None => "1920:1080",
        }
        .to_string()
    }

    /// Parse a `"w:h"` or `"wxh"` ratio string into a numeric ratio.
    fn parse_ratio(s: &str) -> Option<f64> {
        let (a, b) = s.split_once(':').or_else(|| s.split_once('x'))?;
        let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
        if b == 0.0 { None } else { Some(a / b) }
    }

    /// The enum member whose aspect ratio is nearest `target`.
    fn closest_ratio(target: f64) -> &'static str {
        Self::RATIOS
            .iter()
            .copied()
            .min_by(|a, b| {
                let da = (Self::parse_ratio(a).unwrap_or(1.0) - target).abs();
                let db = (Self::parse_ratio(b).unwrap_or(1.0) - target).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or("1920:1080")
    }

    async fn poll_task(&self, id: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/tasks/{}", self.api_base(), id);
        // As long as litegen waits for this call (the vendor timeout,
        // docs/gateway-settings.md); 180 s when called directly.
        let started = std::time::Instant::now();
        let mut budget = crate::providers::PollBudget::new(std::time::Duration::from_secs(180), std::time::Duration::from_secs(2));
        while budget.next().await {
            let builder = crate::providers::auth::apply(
                &self.auth,
                &creds,
                self.client.get(&url).header("X-Runway-Version", RUNWAY_VERSION),
            )?;
            let resp = match crate::providers::inject_trace_headers(builder).send().await {
                Ok(resp) => resp,
                // The task is Runway's now; a dropped poll is asked again.
                Err(_) => continue,
            };
            // A 5xx or 429 is asked again; any other error status is the answer.
            let data = match crate::providers::video::read_poll_json(resp, "runway").await {
                Ok(data) => data,
                Err(ProviderError::RequestFailed { retryable: true, .. }) => continue,
                Err(e) => return Err(e),
            };
            match data["status"].as_str() {
                Some("SUCCEEDED") => return Ok(data),
                Some("FAILED") | Some("CANCELLED") => {
                    let err = data["failure"].as_str().or_else(|| data["error"].as_str()).unwrap_or("Unknown error").to_string();
                    return Err(ProviderError::RequestFailed {
                        message: format!("Runway task failed: {err}"),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    });
                }
                Some("PENDING") | Some("THROTTLED") | Some("RUNNING") => continue,
                other => return Err(super::unknown_state("runway", other)),
            }
        }
        Err(ProviderError::Timeout { timeout_ms: budget.total_ms(started) })
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        super::fetch_result(&self.asset_client, url, self.public_assets_only, "runway image").await
    }
}

impl Default for RunwayImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl RunwayImageProvider {
    /// A finished job's result, as `generate` returns it: its image
    /// downloaded, with the vendor's ids.
    async fn finish(&self, id: &str, final_result: Value) -> Result<GenerationOutput, ProviderError> {
        let image_url = match &final_result["output"] {
            Value::Array(arr) => arr.first().and_then(|v| v.as_str()).map(str::to_string),
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
        .ok_or_else(|| ProviderError::RequestFailed {
            message: "Runway task output missing image URL".to_string(),
            status_code: None,
            provider_error: Some(final_result.clone()),
            retryable: false,
        })?;

        let bytes = self.fetch_image_bytes(&image_url).await?;
        // Read from the bytes: the resold models answer PNG, JPEG, WebP or
        // (Ideogram Vector) SVG.
        let content_type = crate::util::content::image_content_type(&bytes)
            .or_else(|| is_svg(&bytes).then_some("image/svg+xml"))
            .unwrap_or("image/png");
        let mut metadata = HashMap::new();
        metadata.insert("task_id".to_string(), Value::String(id.to_string()));
        metadata.insert("url".to_string(), Value::String(image_url));

        Ok(GenerationOutput {
            data: bytes,
            content_type: content_type.to_string(),
            metadata,
            more: Vec::new(),
        })
    }
}

#[async_trait]
impl ImageProvider for RunwayImageProvider {
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
            .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.key_pool.is_some())
    }

    /// Submit `POST /text_to_image`, poll the task, fetch the first output.
    ///
    /// @see <https://docs.dev.runwayml.com/guides/using-the-api/> — `promptText`,
    ///   `model`, `ratio`, `referenceImages[]` ({uri, tag}), and `GET /tasks/{id}`.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/text_to_image", self.api_base());
        let body = match Self::resolve_model(&model.id) {
            Some(Native::Gen4(native)) => self.gen4_body(native, model, base, extras, materialized)?,
            Some(Native::Resold(r)) => self.resold_body(r, model, base, extras, materialized)?,
            None => return Err(ProviderError::ModelNotSupported { model: model.id.clone(), provider: "runway".into() }),
        };

        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("X-Runway-Version", RUNWAY_VERSION)
                .json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Runway image request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // Only a connection that never opened is safe to retry: a
                // request that timed out may have been accepted and billed.
                retryable: e.is_connect(),
            })?;

        let status = resp.status();
        let retry_after = crate::providers::retry_after_ms(resp.headers());
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let submit: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let err = submit["error"].as_str().map(str::to_string).unwrap_or_else(|| text.chars().take(200).collect());
            if let Some(e) = crate::providers::rate_limited("runway", status, retry_after, &format!("Runway API error: {err}"), Some(&submit)) {
                return Err(e);
            }
            return Err(ProviderError::RequestFailed {
                message: format!("Runway API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(submit),
                // Turned away, not run: "429 | If the client makes too many requests … | Yes".
                // @see <https://docs.dev.runwayml.com/errors/errors.md>
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }
        let submit: Value = serde_json::from_str(&text).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Runway response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        let id = submit["id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Runway response missing task id".to_string(),
            status_code: None,
            provider_error: Some(submit.clone()),
            retryable: false,
        })?;
        // Accepted: from here on a failure must not re-send the job (L1).
        extras.vendor_job.record(id);

        let final_result = self.poll_task(id).await?;
        self.finish(id, final_result).await
    }

    fn resumable(&self, _model: &ModelSchema) -> bool {
        true
    }

    /// One look at an accepted job, read as `generate`'s own poll loop reads it.
    async fn poll_job(&self, _model: &str, job: &str, _resume: Option<&Value>) -> Result<crate::providers::ImageJobPoll, ProviderError> {
        match super::poll_once(self.poll_task(job)).await {
            Ok(Some(result)) => self.finish(job, result).await.map(crate::providers::ImageJobPoll::Done),
            Ok(None) => Ok(crate::providers::ImageJobPoll::Running),
            Err(e) => super::job_failed(e),
        }
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
                message: "Runway image provider not configured".into(),
                latency_ms: None,
            };
        }
        // Runway has no cheap unauthenticated ping; report configured.
        HealthCheckResult {
            healthy: true,
            message: "Runway image provider configured".into(),
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// L13: every catalog row has its own arm, and an id with none is refused
    /// rather than billed as another model.
    #[test]
    fn every_catalog_row_resolves_and_an_unknown_id_does_not() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        for m in catalog.for_provider("runway").filter(|m| m.media_type == crate::capabilities::MediaType::Image) {
            assert!(RunwayImageProvider::resolve_model(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(RunwayImageProvider::resolve_model("runway/not-a-model").is_none());
    }

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> RunwayImageProvider {
        let mut p = RunwayImageProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: "rw-secret".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("rw-secret".to_string());
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

    #[tokio::test]
    async fn generates_gen4_image_via_polling() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/out.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/text_to_image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "task_1" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/tasks/task_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "task_1", "status": "SUCCEEDED", "output": [image_url.clone()]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"RUNWAYPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4_image");
        let base = make_base("a neon tokyo alley", "runway/gen4_image");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        // The vendor accepted the job before the poll: the router must never re-send it.
        assert_eq!(extras.vendor_job.get().as_deref(), Some("task_1"));
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"RUNWAYPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("authorization").unwrap(), "Bearer rw-secret");
        assert_eq!(post.headers.get("x-runway-version").unwrap(), "2024-11-06");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["promptText"], "a neon tokyo alley");
        assert_eq!(body["model"], "gen4_image");
        assert_eq!(body["ratio"], "1920:1080");
    }

    /// Exercise the real `resolve_ratio`, not a copy of its table: every value
    /// it can produce must be a member of Runway's `ratio` enum.
    #[test]
    fn resolve_ratio_only_ever_returns_enum_members() {
        let mk = |ar: Option<&str>, size: Option<&str>| ImageExtras {
            vendor_job: Default::default(),
            size: size.map(str::to_string),
            resolution: None, aspect_ratio: ar.map(str::to_string),
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
        };

        // Every aspect ratio any Runway model advertises.
        for id in ["runway/gen4_image", "runway/gen4_image_turbo"] {
            let schema = ref_schema(id);
            let allowed = match schema.params.get("aspect_ratio") {
                Some(crate::capabilities::schema::ParamSpec::AspectRatio(ar)) => ar.allowed.clone(),
                other => panic!("{id} aspect_ratio is {other:?}"),
            };
            assert!(!allowed.is_empty());
            for ar in allowed {
                let got = RunwayImageProvider::resolve_ratio(&mk(Some(&ar), None));
                assert!(
                    RunwayImageProvider::RATIOS.contains(&got.as_str()),
                    "{id} ratio {ar:?} produced {got:?}, which Runway rejects"
                );
            }
        }

        // Unset, unknown, and free-form pixel sizes must also land in the enum —
        // a caller-supplied size used to be forwarded verbatim.
        for (ar, size) in [
            (None, None),
            (Some("3:2"), None),
            (Some("2:3"), None),
            (Some("banana"), None),
            (None, Some("1000x800")),
            (None, Some("512x2048")),
            (None, Some("4096x1024")),
            (None, Some("not-a-size")),
            (None, Some("100x0")),
        ] {
            let got = RunwayImageProvider::resolve_ratio(&mk(ar, size));
            assert!(
                RunwayImageProvider::RATIOS.contains(&got.as_str()),
                "ar={ar:?} size={size:?} produced {got:?}, which Runway rejects"
            );
        }
    }

    // ─── Runway audit, 2026-10-30 ───────────────────────────────────────────

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    /// Runway answers every submit with a 400, so `generate()` returns before
    /// polling; only the outbound body matters.
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

    /// #16: `extra` adds Runway's own knobs but never replaces what litegen sends.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends() {
        let server = refusing_server().await;
        let mut base = make_base("a lighthouse", "runway/gen4_image");
        base.seed = Some(3);
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "model": "gen4_image_turbo", "promptText": "other", "ratio": "720:720", "seed": 9,
            "referenceImages": [], "contentModeration": { "publicFigureThreshold": "low" }
        }));
        let _ = make_provider(&server.uri()).generate(&ref_schema("runway/gen4_image"), &base, &extras, &no_refs()).await;
        let body = &sent_bodies(&server).await[0];
        assert_eq!(body["model"], "gen4_image");
        assert_eq!(body["promptText"], "a lighthouse");
        assert_eq!(body["ratio"], "1920:1080");
        assert_eq!(body["seed"], 3);
        assert!(body.get("referenceImages").is_none(), "{body}");
        assert_eq!(body["contentModeration"]["publicFigureThreshold"], "low");
    }

    /// #17, #18: a submit that timed out may have been accepted and billed, so
    /// it is never sent again; a 429 was turned away and may be ("429 | If the
    /// client makes too many requests … | Yes").
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
        let schema = ref_schema("runway/gen4_image");
        let err = p.generate(&schema, &make_base("x", "runway/gen4_image"), &make_extras(), &no_refs()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");

        let limited = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "error": "Too many requests" })))
            .mount(&limited)
            .await;
        let err = make_provider(&limited.uri())
            .generate(&schema, &make_base("x", "runway/gen4_image"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: true, status_code: Some(429), .. }), "{err:?}");
    }

    /// #21: "GIF images are not supported", and a data URI is at most
    /// 5,242,880 characters matching `^data:image\/.*`. A base64 reference goes
    /// with its own type; one Runway cannot take is refused before any call.
    /// @see <https://docs.dev.runwayml.com/assets/inputs.md> (2026-10-30)
    #[tokio::test]
    async fn a_reference_goes_with_its_own_type_and_one_runway_cannot_take_is_refused() {
        use base64::Engine;
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let one = |form| MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef { role: "init".into(), form, time_s: None }],
            cleanup: Cleanup::empty(),
        };
        let server = refusing_server().await;
        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4_image");
        let base = make_base("x", "runway/gen4_image");
        let jpeg = one(MaterializedRefForm::Base64(b64(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F'])));
        let _ = provider.generate(&schema, &base, &make_extras(), &jpeg).await;
        let uri = sent_bodies(&server).await[0]["referenceImages"][0]["uri"].as_str().unwrap().to_string();
        assert!(uri.starts_with("data:image/jpeg;base64,"), "{}", &uri[..40.min(uri.len())]);

        for form in [
            MaterializedRefForm::Base64(b64(b"GIF89a\x01\x00\x01\x00")),
            MaterializedRefForm::Url(format!("data:image/png;base64,{}", "A".repeat(5_242_880))),
        ] {
            let err = provider.generate(&schema, &base, &make_extras(), &one(form)).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1, "a refused reference reached Runway");
    }

    /// #13: a poll Runway answers with an error is that error, not a task
    /// still running ("404 | Requests will return a 404 when the referenced
    /// resource is not available | No").
    /// @see <https://docs.dev.runwayml.com/errors/errors.md> (2026-10-30)
    #[tokio::test]
    async fn a_poll_error_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tasks/t1"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "error": "Task not found" })))
            .mount(&server)
            .await;
        let err = make_provider(&server.uri())
            .generate(&ref_schema("runway/gen4_image"), &make_base("x", "runway/gen4_image"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { status_code: Some(404), retryable: false, .. }), "{err:?}");
    }

    // ─── Resold models (2026-11-10) ─────────────────────────────────────────

    fn extras_with(aspect: Option<&str>, resolution: Option<&str>) -> ImageExtras {
        let mut e = make_extras();
        e.aspect_ratio = aspect.map(str::to_string);
        e.resolution = resolution.map(str::to_string);
        e
    }

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
            cleanup: Cleanup::empty(),
        }
    }

    fn runway_image_rows() -> Vec<crate::capabilities::ModelSchema> {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        let mut rows: Vec<_> =
            catalog.for_provider("runway").filter(|m| m.media_type == crate::capabilities::MediaType::Image).cloned().collect();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        rows
    }

    /// Every aspect × resolution a resold row offers reaches Runway as a size
    /// of that aspect (within the 4.5% Google's 4:3 / 5:4 / 1:8 sizes sit
    /// off), larger at each higher tier. Driven through `generate()` against a
    /// server that refuses every submit, so only the outbound `ratio` counts.
    #[tokio::test]
    async fn every_aspect_and_resolution_a_resold_row_offers_reaches_runway_as_its_size() {
        let server = refusing_server().await;
        let provider = make_provider(&server.uri());
        let mut checked = 0;
        for m in runway_image_rows().into_iter().filter(|m| !m.id.starts_with("runway/gen4")) {
            let aspects = match m.params.get("aspect_ratio") {
                Some(crate::capabilities::schema::ParamSpec::AspectRatio(a)) => a.allowed.clone(),
                _ => continue,
            };
            let tiers: Vec<Option<String>> = match m.params.get("resolution") {
                Some(crate::capabilities::schema::ParamSpec::String(s)) => s.enum_values.iter().cloned().map(Some).collect(),
                _ => vec![None],
            };
            // Bria FIBO Edit sizes only a request of two or more images.
            let materialized = if m.id.ends_with("bria_fibo_edit_1_5") { refs(&["init", "init"]) } else { no_refs() };
            for aspect in &aspects {
                let mut last_pixels = 0u64;
                for tier in &tiers {
                    let before = server.received_requests().await.unwrap().len();
                    let err = provider
                        .generate(&m, &make_base("x", &m.id), &extras_with(Some(aspect), tier.as_deref()), &materialized)
                        .await
                        .unwrap_err();
                    let sent = server.received_requests().await.unwrap();
                    assert_eq!(sent.len(), before + 1, "{} {aspect} {tier:?} never reached Runway: {err:?}", m.id);
                    let body: Value = serde_json::from_slice(&sent[before].body).unwrap();
                    let ratio = body["ratio"].as_str().unwrap_or_else(|| panic!("{} {aspect} {tier:?}: no ratio in {body}", m.id));
                    let (w, h) = ratio.split_once(':').map(|(w, h)| (w.parse::<f64>().unwrap(), h.parse::<f64>().unwrap())).unwrap();
                    let (a, b) = aspect.split_once(':').map(|(a, b)| (a.parse::<f64>().unwrap(), b.parse::<f64>().unwrap())).unwrap();
                    assert!(((w / h) / (a / b)).ln().abs() < 0.045, "{} {aspect} {tier:?} sent {ratio}", m.id);
                    let pixels = (w * h) as u64;
                    assert!(pixels > last_pixels, "{} {aspect}: {tier:?} ({ratio}) is no larger than the tier below", m.id);
                    last_pixels = pixels;
                    checked += 1;
                }
            }
        }
        assert!(checked > 400, "only {checked} cells checked");
    }

    /// A resold model gets exactly its own branch's fields: `quality`,
    /// `background` and tagged `referenceImages` on GPT Image; a plain
    /// `{uri}` and `outputFormat` on Seedream (its branch sets
    /// `additionalProperties: false`); never `outputCount`.
    /// @see <https://docs.dev.runwayml.com/openapi.json> (2026-11-10)
    #[tokio::test]
    async fn a_resold_model_sends_its_own_branchs_fields() {
        let server = refusing_server().await;
        let provider = make_provider(&server.uri());

        let mut extras = extras_with(Some("16:9"), Some("2K"));
        extras.quality = Some("xhigh".into());
        extras.background = Some("transparent".into());
        extras.extra = Some(json!({ "outputCount": 4, "model": "gen4_image", "ratio": "auto", "quality": "low" }));
        let id = "runway/gpt_image_2_5_sunburst";
        let _ = provider.generate(&ref_schema(id), &make_base("a lighthouse", id), &extras, &refs(&["init", "init"])).await;
        let body = &sent_bodies(&server).await[0];
        assert_eq!(body["model"], "gpt_image_2_5_sunburst");
        assert_eq!(body["promptText"], "a lighthouse");
        assert_eq!(body["ratio"], "2560:1440");
        assert_eq!(body["quality"], "xhigh");
        assert_eq!(body["background"], "transparent");
        assert_eq!(body["referenceImages"], json!([{ "uri": "https://example.com/init0.png", "tag": "ref0" }, { "uri": "https://example.com/init1.png", "tag": "ref1" }]));
        assert!(body.get("outputCount").is_none() && body.get("seed").is_none(), "{body}");

        let mut extras = extras_with(None, None);
        extras.output_format = Some("jpeg".into());
        extras.extra = Some(json!({ "grounding": true }));
        let id = "runway/seedream5_pro";
        let _ = provider.generate(&ref_schema(id), &make_base("a poster", id), &extras, &refs(&["init"])).await;
        let body = &sent_bodies(&server).await[1];
        assert_eq!(body["model"], "seedream5_pro");
        // The row's defaults, 1:1 at 1K.
        assert_eq!(body["ratio"], "1024:1024");
        assert_eq!(body["outputFormat"], "jpeg");
        assert_eq!(body["grounding"], true);
        assert_eq!(body["referenceImages"], json!([{ "uri": "https://example.com/init0.png" }]));
        assert!(body.get("quality").is_none() && body.get("background").is_none(), "{body}");

        // A model whose `ratio` is optional gets none unless one is asked for;
        // a resolution alone is 1:1 at that tier.
        let id = "runway/ideogram_4_5";
        let _ = provider.generate(&ref_schema(id), &make_base("a sign", id), &extras_with(None, None), &no_refs()).await;
        let _ = provider.generate(&ref_schema(id), &make_base("a sign", id), &extras_with(None, Some("2K")), &no_refs()).await;
        let id = "runway/ideogram_vector";
        let mut extras = extras_with(None, None);
        extras.quality = Some("high".into());
        let mut base = make_base("a fox logo", id);
        base.seed = Some(7);
        let _ = provider.generate(&ref_schema(id), &base, &extras, &no_refs()).await;
        let bodies = sent_bodies(&server).await;
        assert!(bodies[2].get("ratio").is_none(), "{}", bodies[2]);
        assert_eq!(bodies[3]["ratio"], "2048:2048");
        assert!(bodies[4].get("ratio").is_none(), "{}", bodies[4]);
        assert_eq!((bodies[4]["quality"].as_str(), bodies[4]["seed"].as_u64()), (Some("high"), Some(7)));
    }

    /// Ideogram 4.5 edits the first reference image under `maskUri` ("A
    /// black-and-white mask the same size as the first reference image");
    /// the mask is no reference image itself.
    /// @see <https://docs.dev.runwayml.com/openapi.json> (2026-11-10)
    #[tokio::test]
    async fn an_ideogram_mask_goes_as_mask_uri() {
        let server = refusing_server().await;
        let id = "runway/ideogram_4_5";
        let _ = make_provider(&server.uri()).generate(&ref_schema(id), &make_base("a red door", id), &extras_with(None, None), &refs(&["init", "mask"])).await;
        let body = &sent_bodies(&server).await[0];
        assert_eq!(body["maskUri"], "https://example.com/mask1.png");
        assert_eq!(body["referenceImages"], json!([{ "uri": "https://example.com/init0.png" }]));
    }

    /// Requests Runway documents as impossible are refused before any call:
    /// - Bria FIBO Edit: "Choosing a size requires at least 2 reference images".
    /// - Ideogram 4.5: "`very_low` requires reference images and is not
    ///   available with `preciseEdit`".
    /// - `edit` (Grok) and `preciseEdit` (Ideogram) "Requires at least one
    ///   reference image".
    /// @see <https://docs.dev.runwayml.com/openapi.json> (2026-11-10)
    #[tokio::test]
    async fn a_request_runway_documents_as_impossible_is_refused_before_any_call() {
        let server = refusing_server().await;
        let provider = make_provider(&server.uri());
        let with = |quality: Option<&str>, extra: Option<Value>| {
            let mut e = extras_with(None, None);
            e.quality = quality.map(str::to_string);
            e.extra = extra;
            e
        };
        for (id, extras, materialized) in [
            ("runway/bria_fibo_edit_1_5", extras_with(Some("16:9"), None), refs(&["init"])),
            ("runway/ideogram_4_5", with(Some("very_low"), None), no_refs()),
            ("runway/ideogram_4_5", with(Some("very_low"), Some(json!({ "preciseEdit": true }))), refs(&["init"])),
            ("runway/grok_imagine_image_2", with(None, Some(json!({ "edit": true }))), no_refs()),
            ("runway/ideogram_4_5", with(None, Some(json!({ "preciseEdit": true }))), no_refs()),
        ] {
            let err = provider.generate(&ref_schema(id), &make_base("x", id), &extras, &materialized).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{id}: {err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty(), "a refused request reached Runway");
    }

    /// The result's type is read from its bytes: Seedream's JPEG, Ideogram
    /// Vector's SVG ("returns one SVG per request").
    /// @see <https://docs.dev.runwayml.com/guides/pricing.md> (2026-11-10)
    #[tokio::test]
    async fn the_result_is_labelled_by_its_bytes() {
        for (id, bytes, want) in [
            ("runway/seedream5_flash", b"\xff\xd8\xff\xe0JFIF".to_vec(), "image/jpeg"),
            ("runway/ideogram_vector", b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(), "image/svg+xml"),
        ] {
            let server = MockServer::start().await;
            let files = MockServer::start().await;
            Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" }))).mount(&server).await;
            Mock::given(method("GET"))
                .and(path("/tasks/t1"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1", "status": "SUCCEEDED", "output": [format!("{}/out", files.uri())] })))
                .mount(&server)
                .await;
            Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(bytes)).mount(&files).await;
            let out = make_provider(&server.uri()).generate(&ref_schema(id), &make_base("x", id), &extras_with(None, None), &no_refs()).await.unwrap();
            assert_eq!(out.content_type, want, "{id}");
        }
    }

    /// #23: the result is fetched without the key, only from a public address,
    /// and at most 64 MiB; Runway has billed the task by then, so a refusal is
    /// final.
    #[tokio::test]
    async fn a_result_is_fetched_only_from_a_public_address() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tasks/t1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "t1", "status": "SUCCEEDED", "output": [format!("{}/out.png", image_server.uri())]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"\x89PNG\r\n\x1a\n".to_vec()))
            .mount(&image_server)
            .await;
        let mut provider = make_provider(&server.uri());
        provider.public_assets_only = true;
        let err = provider
            .generate(&ref_schema("runway/gen4_image"), &make_base("x", "runway/gen4_image"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
        assert!(image_server.received_requests().await.unwrap().is_empty(), "a private address was fetched");
    }
}
