// litegen-core/src/capabilities/schema.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = CapabilityMediaType)]
pub enum MediaType {
    Image,
    Video,
    /// Serializes as "model3d" — snake_case does not underscore before a digit.
    Model3d,
}

// An operation is a catalog model, and each modality has one job endpoint
// (PixelGenie 3D doc §2 rule 1, docs/superpowers/plans/2026-10-30-…): validation,
// pricing, routing and `docs_url` then stay per operation, with no new routes.

/// What a catalog model does.
///
/// An operation on an existing asset is a model of its own, submitted to its
/// modality's one job endpoint: `tripo/rig` and `meshy/animate` go to
/// `POST /v1/models3d/generations` like any generation, and are polled,
/// priced, cancelled, re-hosted and billed the same way, because each
/// produces a new asset.
///
/// One task per model. `generate` is the default in every modality. It still
/// covers image edits driven by `init` and `mask` references, and 3D models
/// that infer text, image or multiview input from their references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelTask {
    /// Make a new asset: text-, image- or multiview-to-X, image edits included.
    #[default]
    Generate,
    /// 3D: add a skeleton and skin to `source_model`.
    Rig,
    /// 3D: give a rigged `source_model` animation clips.
    Animate,
    /// 3D: paint new textures onto `source_model`.
    Texture,
    /// 3D: retopologise `source_model`.
    Remesh,
    /// 3D: split `source_model` into named parts. Image: masks for what
    /// point, box or text prompts select in an image (PixelGenie L7), served
    /// by `POST /v1/images/segmentations`.
    Segment,
    /// Image: views of the `init` image's subject from other sides.
    Multiview,
    /// Image: replace the background behind the `init` image's subject.
    BackgroundSwap,
    /// Video: make `source_video` longer.
    Extend,
    /// Video: regenerate a time window of `source_video`.
    Retake,
    /// Video: edit the whole of `source_video`.
    VideoEdit,
}

impl ModelTask {
    pub const ALL: [ModelTask; 11] = [
        ModelTask::Generate,
        ModelTask::Rig,
        ModelTask::Animate,
        ModelTask::Texture,
        ModelTask::Remesh,
        ModelTask::Segment,
        ModelTask::Multiview,
        ModelTask::BackgroundSwap,
        ModelTask::Extend,
        ModelTask::Retake,
        ModelTask::VideoEdit,
    ];

    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            ModelTask::Generate => "generate",
            ModelTask::Rig => "rig",
            ModelTask::Animate => "animate",
            ModelTask::Texture => "texture",
            ModelTask::Remesh => "remesh",
            ModelTask::Segment => "segment",
            ModelTask::Multiview => "multiview",
            ModelTask::BackgroundSwap => "background_swap",
            ModelTask::Extend => "extend",
            ModelTask::Retake => "retake",
            ModelTask::VideoEdit => "video_edit",
        }
    }

    /// Whether a model of this media type may declare this task.
    pub fn allowed_for(self, media: MediaType) -> bool {
        use ModelTask::*;
        match media {
            MediaType::Model3d => matches!(self, Generate | Rig | Animate | Texture | Remesh | Segment),
            MediaType::Image => matches!(self, Generate | Multiview | BackgroundSwap | Segment),
            MediaType::Video => matches!(self, Generate | Extend | Retake | VideoEdit),
        }
    }
}

impl MediaType {
    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            MediaType::Image => "image",
            MediaType::Video => "video",
            MediaType::Model3d => "model3d",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelCapabilityFlags {
    #[serde(default)] pub text_to_image: bool,
    #[serde(default)] pub image_to_image: bool,
    #[serde(default)] pub inpainting: bool,
    #[serde(default)] pub text_to_video: bool,
    #[serde(default)] pub image_to_video: bool,
    #[serde(default)] pub text_to_3d: bool,
    #[serde(default)] pub image_to_3d: bool,
    #[serde(default)] pub multiview_to_3d: bool,
    /// Takes one `init` image and returns it as an RGBA PNG with the
    /// background made transparent (PixelGenie L8).
    #[serde(default)] pub background_removal: bool,
    /// Draws native pixel art: its output is at native size, one image pixel
    /// per art pixel, and needs no reducing (PixelGenie L16).
    #[serde(default)] pub native_pixel_art: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[schema(as = CapabilityModelPricing)]
#[serde(deny_unknown_fields)]
pub struct ModelPricing {
    /// The vendor's price for the model's default request.
    pub base_cost_usd: f64,
    /// How the price follows the request (3D doc §3G, L14): rules applied in
    /// order to a running price that starts at `base_cost_usd`. The quote,
    /// the quota reservation and the charge all come from them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variable_pricing: Vec<PriceRule>,
}

/// One pricing rule. When every `when` entry matches the request, its one
/// effect applies: a price (`usd`), a rate (`per_second_usd` × the clip's
/// seconds, `per_megapixel_usd` × the image's megapixels, `per_item_usd` × the
/// count of the `per_item` list) or an adjustment (`multiply`, `add_usd`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PriceRule {
    /// Request facts the rule needs: a param (as sent, else its default), an
    /// `extra.<field>`, or a derived fact (the `views`, `added_views`,
    /// `keyframes`, `animations` and `references` counts, `duration_seconds`,
    /// `megapixels`), each mapped to the value it must have, `"*"` for any
    /// value present, or a range a number must fall in, such as
    /// `{"gt": 1.048576, "lte": 4.194304}` (`gt`, `gte`, `lt`, `lte`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub when: std::collections::BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_second_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_megapixel_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_item_usd: Option<f64>,
    /// The list `per_item_usd` counts (`animations`, `views`, `keyframes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiply: Option<f64>,
    /// Scale the price by the image's `megapixels` over this many, for an
    /// image larger than this: a vendor that bills by output size (OpenAI's
    /// image tokens) past the size its base price is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiply_per_megapixel_above: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_usd: Option<f64>,
}

/// How a model treats the request's `prompt`. Lengths count characters.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptSpec {
    /// An empty or omitted prompt is refused with `prompt_required`. 3D models
    /// that take images set this false, yet still need a prompt when a request
    /// carries no image (text-to-3D).
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)] pub min_length: Option<usize>,
    /// `0` means the model takes no prompt at all (an image-only endpoint
    /// whose vendor has no prompt field): a client should not offer one, and a
    /// non-empty prompt is refused in strict mode and dropped in lax mode.
    #[serde(default)] pub max_length: Option<usize>,
    /// A video model whose text-to-video mode needs the prompt though its
    /// image-to-video mode does not (so `required` is false): a request with
    /// no `first_frame` or `init` image and an empty prompt is refused with
    /// `prompt_required` before anything is quoted or reserved. Absent
    /// (false): an empty text-to-video request goes to the vendor, as for a
    /// vendor whose shots can carry the prompts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required_for_text_to_video: bool,
}

// ─── ParamSpec ──────────────────────────────────────────────────────────────
//
// Each variant payload lives in its own named struct so utoipa emits a real
// component schema for it (instead of an anonymous oneOf entry). The wire
// format is unchanged thanks to `#[serde(tag = "kind")]` on the enum and
// serde's flattening of newtype variants.

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecBool {
    #[serde(default)] pub default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecInt {
    #[serde(default)] pub min: Option<i64>,
    #[serde(default)] pub max: Option<i64>,
    #[serde(default)] pub default: Option<i64>,
    /// The only values the vendor takes, when it takes a list rather than a
    /// range (`duration_seconds`, PixelGenie L10).
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub values: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecFloat {
    #[serde(default)] pub min: Option<f64>,
    #[serde(default)] pub max: Option<f64>,
    #[serde(default)] pub default: Option<f64>,
    /// The only values the vendor takes, when it takes a list rather than a
    /// range (`duration_seconds`, PixelGenie L10).
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub values: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecString {
    /// The most characters a value may have, counted as JSON Schema's
    /// `maxLength` counts them (characters, not bytes).
    #[serde(default)] pub max_length: Option<usize>,
    #[serde(default)] pub enum_values: Vec<String>,
    /// A regular expression the value must match, anywhere in it as JSON
    /// Schema's `pattern` does: `^…$` anchors it to the whole value. A value
    /// it does not match is refused (`param_invalid`).
    #[serde(default)] pub pattern: Option<String>,
    #[serde(default)] pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

impl ParamSpecString {
    /// Whether `v` matches the spec's `pattern` (N4), searched anywhere in it
    /// as JSON Schema's `pattern` is; true with no pattern. A pattern that is
    /// no regular expression matches nothing (the loader refuses one).
    pub fn matches_pattern(&self, v: &str) -> bool {
        self.pattern.as_deref().is_none_or(|_| self.compiled_pattern().is_some_and(|r| r.is_match(v)))
    }

    /// The spec's `pattern`, compiled; `None` with no pattern, or one that is
    /// no regular expression.
    pub fn compiled_pattern(&self) -> Option<std::sync::Arc<regex::Regex>> {
        self.pattern.as_deref().and_then(|p| PATTERNS.get(p))
    }
}

/// Every `pattern` compiled once (passthrough review #3): a string param is
/// matched against its pattern on each request that sends it, and compiling
/// a regular expression costs far more than matching one. Keyed by the
/// pattern's text, so every copy of a spec shares one (the registry hands
/// each request its own copy of the schema). A pattern that is no regular
/// expression is kept as `None` (the loader refuses such a spec).
struct PatternCache {
    compiled: std::sync::RwLock<HashMap<String, Option<std::sync::Arc<regex::Regex>>>>,
    /// At most this many patterns are kept; past it a new one is compiled
    /// for each use, as before, and nothing kept is dropped.
    capacity: usize,
}

impl PatternCache {
    fn new(capacity: usize) -> Self {
        PatternCache { compiled: std::sync::RwLock::new(HashMap::new()), capacity }
    }

    /// `pattern` compiled; `None` when it is no regular expression.
    fn get(&self, pattern: &str) -> Option<std::sync::Arc<regex::Regex>> {
        if let Some(kept) = self.compiled.read().unwrap_or_else(|p| p.into_inner()).get(pattern) {
            return kept.clone();
        }
        let fresh = regex::Regex::new(pattern).ok().map(std::sync::Arc::new);
        let mut compiled = self.compiled.write().unwrap_or_else(|p| p.into_inner());
        // Another thread may have kept it meanwhile: every caller gets one.
        if let Some(kept) = compiled.get(pattern) {
            return kept.clone();
        }
        if compiled.len() < self.capacity {
            compiled.insert(pattern.to_string(), fresh.clone());
        }
        fresh
    }
}

/// The patterns of every spec in this process.
static PATTERNS: std::sync::LazyLock<PatternCache> = std::sync::LazyLock::new(|| PatternCache::new(MAX_CACHED_PATTERNS));

/// How many patterns [`PATTERNS`] keeps: the catalog's and the runtime
/// models' are a handful.
const MAX_CACHED_PATTERNS: usize = 1024;

/// A param whose value is a SET of enum members rather than one of them.
///
/// Exists for `output_formats`, where the vendors genuinely disagree on
/// cardinality: Meshy emits every requested container from one job, Rodin's
/// `geometry_file_format` is a scalar so only one is reachable, and Stability
/// emits GLB with no choice at all. `max_items` is how a model states which of
/// those it is, so an unsatisfiable request fails in validation rather than
/// after the vendor has been billed.
///
/// For `output_formats` the catalog yaml and the API describe two different
/// sets, on purpose:
///
/// - **yaml** `enum_values` declares what the VENDOR emits natively — the only
///   thing a catalog author can verify against the vendor's schema.
/// - **`GET /v1/models/{id}`** serves what a CALLER may request: the native
///   set plus every container litegen derives locally, with `derivable_values`
///   saying which those are. That is the same set the validator accepts, so a
///   client rendering `params` offers exactly what will be honoured.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecStringArray {
    /// Accepted members. See the type docs for what this means in yaml versus
    /// in an API response.
    #[serde(default)] pub enum_values: Vec<String>,
    /// For `output_formats`: how many VENDOR jobs one request may need — not
    /// how many members it may name. Every member of `derivable_values` comes
    /// out of one source container, so a request costs
    /// `|requested \ derivable_values| + (requested ∩ derivable_values ≠ ∅ ? 1 : 0)`
    /// jobs, and more than `max_items` is refused with `param_too_many`.
    /// `None` means no limit.
    #[serde(default)] pub max_items: Option<usize>,
    /// Applied when the caller omits the param entirely. For
    /// `output_formats`: in yaml, empty means the resolver falls back to
    /// `glb`; `GET /v1/models/{id}` serves the resolved list, so it is never
    /// empty there.
    #[serde(default)] pub default: Vec<String>,
    /// The members litegen produces locally from a single vendor job (for
    /// `output_formats`, whatever its mesh converter covers — today `glb`,
    /// `obj`, `stl`, `ply`). Present only in API
    /// responses, and only when the model's vendor emits at least one of them —
    /// with nothing readable to convert from, nothing is derivable. Computed
    /// by the server; the catalog loader refuses a yaml that sets it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derivable_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecAspectRatio {
    pub allowed: Vec<String>,
    #[serde(default)] pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
    /// A video model whose frame image sets the clip's shape: with a
    /// `first_frame`, `last_frame` or `init` image the vendor takes no ratio,
    /// so `aspect_ratio` is refused (`param_unsupported`) when `strict` and
    /// dropped and named otherwise. Absent: the ratio applies with any image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames_set_shape: Option<FramesSetShape>,
}

/// When a frame image sets a video's shape (`ParamSpecAspectRatio`'s
/// `frames_set_shape`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FramesSetShape {
    /// With any frame image, a `reference` image included.
    Always,
    /// With a frame image and no `reference` image: a request that also
    /// carries a `reference` image goes to a vendor mode that takes a ratio.
    WithoutReferences,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecSeed {
    pub min: i64,
    pub max: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

/// What an operation on an existing mesh takes as its `source_model` (3D doc
/// §3A). litegen reads the source with its own mesh reader and refuses one the
/// vendor would refuse — before the billed submit — then converts it to a
/// container in `formats` if it is not already one.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecSourceModel {
    /// Containers the vendor accepts, from the `Model3dFormat` vocabulary. A
    /// source in any other container litegen can read is converted to the
    /// first of these it can write.
    #[serde(default)]
    pub formats: Vec<String>,
    /// Largest file the vendor accepts, in bytes, as sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// Most triangles the vendor accepts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_faces: Option<u64>,
    /// The source must already be rigged (have a skin).
    #[serde(default)]
    pub requires_skin: bool,
    /// The source must be textured.
    #[serde(default)]
    pub requires_texture: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamSpec {
    Bool(ParamSpecBool),
    Int(ParamSpecInt),
    Float(ParamSpecFloat),
    String(ParamSpecString),
    StringArray(ParamSpecStringArray),
    Size(SizeSpec),
    AspectRatio(ParamSpecAspectRatio),
    Seed(ParamSpecSeed),
    SourceModel(ParamSpecSourceModel),
    AnimationList(ParamSpecAnimationList),
    Region(ParamSpecRegion),
    SegmentationPrompts(ParamSpecSegmentationPrompts),
    Expand(ParamSpecExpand),
    Layout(ParamSpecLayout),
    SourceVideo(ParamSpecSourceVideo),
    TimeRange(ParamSpecTimeRange),
    Palette(ParamSpecPalette),
}

/// `palette` on an image model (PixelGenie L16): the colours it keeps to.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecPalette {
    /// Every pixel is one of the palette's colours (PixelLab's forced
    /// palette, Retro Diffusion's `input_palette`); otherwise the palette is
    /// a hint (Ideogram's `color_palette`, Recraft's `controls`).
    #[serde(default)]
    pub forced: bool,
    /// The most colours one palette may hold.
    pub max_colors: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

/// `source_video` on a video operation (3D doc §5): the clips it works on.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecSourceVideo {
    /// Containers the vendor accepts: `mp4`, `mov`, `webm`, `gif`.
    #[serde(default)]
    pub formats: Vec<String>,
    /// The longest source it takes, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_s: Option<f64>,
    /// Only the vendor's own earlier generations, on the same account (Luma,
    /// Kling): any other source is `source_not_chainable`.
    #[serde(default)]
    pub own_generations_only: bool,
    /// An `extend` model's answer: the whole clip, or only the added part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extend_output: Option<ExtendOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

/// What an `extend` model returns (3D doc §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExtendOutput {
    /// The source and the added part, as one clip.
    Full,
    /// Only the added part.
    Added,
}

/// `time_range` on a video operation (3D doc §5): the windows it takes.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecTimeRange {
    /// The shortest window, in seconds (LTX's 2 s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_window_s: Option<f64>,
    /// Starts and ends fall on whole seconds (Runway).
    #[serde(default)]
    pub whole_seconds: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

/// `layout` on a text-to-image model (3D doc §4): the boxes it composes.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecLayout {
    /// What its boxes may hold, listed as `layout_kinds`.
    pub kinds: Vec<LayoutKind>,
    /// The most boxes one request may send.
    pub max_items: usize,
    /// Each text box holds one word (Recraft's `text_layout`).
    #[serde(default)]
    pub single_word_text: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

/// What a layout box holds (3D doc §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LayoutKind {
    /// An object, described by the box's `prompt`.
    Object,
    /// Words, the box's `text`, rendered there.
    Text,
}

impl LayoutKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            LayoutKind::Object => "object",
            LayoutKind::Text => "text",
        }
    }
}

/// `expand` on a model (3D doc §4): how its vendor grows a canvas.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecExpand {
    /// The vendor only centres the source: left must equal right, and top
    /// bottom (anything else is `expand_unsupported`).
    #[serde(default)]
    pub centered_only: bool,
    /// The most pixels it adds to one side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_px: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

/// `prompts` on a segmentation model (PixelGenie L7): what it can be asked
/// to select.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecSegmentationPrompts {
    /// The prompt types the model takes, listed as `segmentation_prompts`.
    pub types: Vec<SegmentationPromptType>,
    /// The most prompts one request may send.
    pub max_items: usize,
    /// The most masks one answer holds: a text prompt finds at most this
    /// many instances.
    pub max_masks: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

/// The kinds of segmentation prompt (PixelGenie L7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SegmentationPromptType {
    /// A spot on (label 1) or off (label 0) the object.
    Point,
    /// A box around the object.
    Box,
    /// The object named: every instance of it.
    Text,
}

impl SegmentationPromptType {
    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            SegmentationPromptType::Point => "point",
            SegmentationPromptType::Box => "box",
            SegmentationPromptType::Text => "text",
        }
    }
}

/// The clips an `animate` model makes (`animations`, 3D doc §3C).
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecAnimationList {
    /// The most clips one request may ask for.
    pub max_items: usize,
    /// The model's presets: its own ids, each optionally tagged with the
    /// canonical preset (`Model3dAnimationPreset`) it plays. A request names
    /// either.
    #[serde(default)]
    pub presets: Vec<AnimationPresetSpec>,
    /// Present when the model makes clips from a prompt (text-to-motion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_to_motion: Option<TextToMotionSpec>,
    /// How `in_place: true` is honoured: by the vendor's own flag, or by
    /// litegen removing the root's horizontal travel.
    #[serde(default)]
    pub in_place: InPlaceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AnimationPresetSpec {
    /// The vendor's own id for the clip.
    pub id: String,
    /// The vendor's own name for it, for people choosing one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The canonical preset it plays, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<String>,
    /// Whether the vendor made it to loop.
    #[serde(default)]
    pub loopable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TextToMotionSpec {
    pub min_duration_s: f64,
    pub max_duration_s: f64,
}

/// Who keeps a clip in place.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InPlaceMode {
    /// litegen removes the root's horizontal travel.
    #[default]
    Litegen,
    /// The vendor's own flag (Tripo's `animate_in_place`).
    Native,
}

// ─── SizeSpec ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SizeSpecFreeform {
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
    #[serde(default)] pub multiple_of: Option<u32>,
    /// The fewest and most pixels (width × height) the vendor makes (L13: a
    /// size it cannot make is refused, never snapped).
    #[serde(default, skip_serializing_if = "Option::is_none")] pub min_pixels: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub max_pixels: Option<u64>,
    /// The widest long-to-short side ratio the vendor makes.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub max_aspect: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SizeSpecEnum {
    // JSON shape is `[[w, h], ...]`; represented in OpenAPI as `Vec<Vec<u32>>`
    // (OpenAPI 3.1's prefixItems is not portable across codegen tools yet).
    #[schema(value_type = Vec<Vec<u32>>)]
    pub values: Vec<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SizeSpec {
    Freeform(SizeSpecFreeform),
    Enum(SizeSpecEnum),
}

// ─── RefInputSpec / RefProviderFormat ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RefInputSpec {
    pub max_total: u32,
    #[serde(default)] pub default_role: Option<String>,
    // `provider_format` is a discriminated union; openapi-python-client 0.21 can't
    // model the oneOf+allOf shape utoipa emits, so we expose the field as a raw
    // JSON value at the OpenAPI level. The standalone `RefProviderFormat` schema
    // is still emitted (TypeScript handles it fine) — Python users can decode it
    // manually with the named variant structs.
    #[schema(value_type = serde_json::Value)]
    pub provider_format: RefProviderFormat,
    pub roles: HashMap<String, RefRoleSpec>,
    /// Whether the vendor takes a `data:` URI where it takes a URL (default
    /// yes). When it does not, litegen publishes an uploaded reference at an
    /// unguessable URL of its own for the vendor to fetch (L4).
    #[serde(default = "default_true")]
    pub data_uri: bool,
    /// For a multiview-to-3D vendor that takes its views as ONE ordered list
    /// (3D doc §3E): the views in that list's order. A view needs every view
    /// before it, since the vendor reads them by position. Empty when the
    /// vendor takes each view by name and any may be omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<crate::types::ImageView>)]
    pub view_order: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RefRoleSpec {
    #[serde(default)] pub required: bool,
    pub min_count: u32,
    pub max_count: u32,
    /// The mask role's VENDOR convention: which pixels it edits. Callers
    /// always send white-edits masks; litegen converts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convention: Option<MaskConvention>,
    /// The mask role's vendor grows masks itself (Stability's `grow_mask`):
    /// `mask_grow_px` goes to it, explicitly, instead of litegen dilating.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native_grow: bool,
    /// The `keyframe` role's timing (3D doc §5): `any` time each, or `even`
    /// spacing over the clip. Required on that role, refused on others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<KeyframeTiming>,
    /// The `keyframe` role's time grid, when the vendor takes only multiples
    /// of it (PixVerse: whole seconds). A time off it is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_step_s: Option<f64>,
    /// The latest a `keyframe` may sit, for a model with no duration param
    /// to bound its clip: the longest clip it makes or prices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_time_s: Option<f64>,
}

/// Where a model's keyframes may sit in the clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyframeTiming {
    /// Each keyframe at its own `time_s`.
    Any,
    /// Evenly spaced from the start of the clip to its end.
    Even,
}

/// Where a model is in its vendor's lifecycle (L14).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    #[default]
    Active,
    /// The vendor deprecated it or announced its shutdown; still served.
    Deprecated,
    /// Past its shutdown date: neither listed nor served.
    Retired,
}

impl ModelSchema {
    /// The model's lifecycle status today.
    pub fn status(&self) -> ModelStatus {
        self.status_on(chrono::Utc::now().date_naive())
    }

    /// Retired strictly after `retires_at` (the vendor still serves it on the
    /// day itself); deprecated from `deprecated_at` on, or at once when a
    /// shutdown date is announced.
    pub fn status_on(&self, today: chrono::NaiveDate) -> ModelStatus {
        let date = |d: &Option<String>| d.as_deref().and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
        match (date(&self.deprecated_at), date(&self.retires_at)) {
            (_, Some(retires)) if today > retires => ModelStatus::Retired,
            (_, Some(_)) => ModelStatus::Deprecated,
            (Some(deprecated), None) if today >= deprecated => ModelStatus::Deprecated,
            _ => ModelStatus::Active,
        }
    }
}

/// Which pixels of a mask mean "edit".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MaskConvention {
    /// White (128 or more) edits, black keeps — the one callers send.
    WhiteEdits,
    /// Black edits, white keeps (Ideogram).
    BlackEdits,
    /// Fully transparent edits, opaque keeps (OpenAI).
    TransparentEdits,
}

/// How a model receives a `region`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RegionInput {
    /// Its own box field (Ideogram's `context_window`).
    Box,
    /// Drawn as the mask.
    Mask,
}

/// `region` on a model (3D doc §4).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamSpecRegion {
    pub input: RegionInput,
    /// The vendor's smallest box side, in pixels. A smaller region gets a
    /// window enlarged to it, with the exact region sent as the mask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_window_px: Option<u32>,
    /// The vendor's largest box aspect ratio (6 for 1:6–6:1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_aspect: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

/// What a vendor promises about the pixels outside an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PreservesUnmasked {
    /// Copied exactly (Ideogram 4.5, Bria).
    Exact,
    /// Kept similar.
    Similar,
    /// The vendor says nothing.
    Unspecified,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RefProviderFormatMultipart {
    pub field_map: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "form", rename_all = "snake_case", from = "RefProviderFormatRead")]
pub enum RefProviderFormat {
    Url,
    Base64,
    Multipart(RefProviderFormatMultipart),
}

/// How `RefProviderFormat` is read (N5): serde ignores every other key of an
/// internally tagged unit variant, so `url` and `base64` are read as empty
/// structs that deny one, as every other catalog struct does (D2).
#[derive(Deserialize)]
#[serde(tag = "form", rename_all = "snake_case", deny_unknown_fields)]
enum RefProviderFormatRead {
    Url {},
    Base64 {},
    Multipart(RefProviderFormatMultipart),
}

impl From<RefProviderFormatRead> for RefProviderFormat {
    fn from(read: RefProviderFormatRead) -> Self {
        match read {
            RefProviderFormatRead::Url {} => RefProviderFormat::Url,
            RefProviderFormatRead::Base64 {} => RefProviderFormat::Base64,
            RefProviderFormatRead::Multipart(m) => RefProviderFormat::Multipart(m),
        }
    }
}

/// The axes a 3D model's vendor builds its meshes along (3D doc §2 rule 3).
/// Where they differ from glTF's (+Y up, +Z front), litegen adds a root
/// rotation and the manifest says `oriented_by: "litegen"`. An operation that
/// hands back its source's own geometry says `source` for both: its output
/// faces wherever the source did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeOrientation {
    /// `+x`, `-x`, `+y`, `-y`, `+z` or `-z`; `source` on an operation that
    /// keeps its source's axes.
    pub up: String,
    /// The axis the asset's front faces, at right angles to `up`; `source`
    /// with `up`.
    pub front: String,
    /// What showed it: the run, sample file or page.
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelSchema {
    pub id: String,
    pub provider: String,
    pub media_type: MediaType,
    /// What the model does: `generate`, or an operation on an existing asset
    /// such as `rig` or `animate`. A row or a runtime model may leave it out
    /// (`generate`), so it stays optional here: this type is also the admin
    /// API's input.
    #[serde(default)]
    pub task: ModelTask,
    /// 3D only: the axes the vendor builds its meshes along, declared once a
    /// run has shown them. Absent means unproven: the mesh is never turned and
    /// its manifest says `front: "unverified"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_orientation: Option<NativeOrientation>,
    /// Image edits: what the vendor promises about pixels outside the edit
    /// area, in its own words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserves_unmasked: Option<PreservesUnmasked>,
    /// When the vendor deprecated the model (`YYYY-MM-DD`); it still serves it
    /// (L14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated_at: Option<String>,
    /// The vendor's announced shutdown date (`YYYY-MM-DD`). From the day
    /// after it, litegen neither lists nor serves the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retires_at: Option<String>,
    /// The catalog model to move to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
    /// Derived from the dates above when the model is served. A served
    /// schema, a stored runtime model or a copy posted back carries it, and
    /// so may yaml: it is read and ignored, never trusted.
    #[serde(default, deserialize_with = "ignore_served_status")]
    pub status: ModelStatus,
    pub display_name: String,
    #[serde(default)] pub description: String,
    /// The vendor's own API reference page for the endpoint this model calls —
    /// the page to re-read when a field mapping is questioned. An absolute
    /// `https://` URL (the loader refuses anything else); every non-mock 3D
    /// model carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    pub pricing: ModelPricing,
    pub capabilities: ModelCapabilityFlags,
    pub prompt: PromptSpec,
    // Same situation as `RefInputSpec.provider_format` — the per-param `ParamSpec`
    // is a discriminated union that openapi-python-client can't model directly.
    // Expose as `dict[str, Any]` at the OpenAPI level; the standalone `ParamSpec`
    // schema is still emitted for TypeScript and for manual Python decoding.
    #[serde(default)]
    #[schema(value_type = HashMap<String, serde_json::Value>)]
    pub params: HashMap<String, ParamSpec>,
    #[serde(default)] pub ref_inputs: Option<RefInputSpec>,
    #[serde(default)] pub extra_allowlist: Vec<String>,
    /// The vendor's type for an `extra_allowlist` key, by key: `bool`,
    /// `int`, `float`, `string` or `string_array`, with its bounds or values
    /// when they are known. A request's value is checked like a param's
    /// (`param: "extra.<key>"`); `null` passes as sent, and an allowlisted key
    /// with no type here passes as any JSON.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    #[schema(value_type = HashMap<String, serde_json::Value>)]
    pub extra_params: std::collections::BTreeMap<String, ParamSpec>,
    /// Request fields litegen applies to this model itself, beyond what its
    /// vendor takes, each with the bounds litegen holds it to and in the same
    /// `ParamSpec` shape as `params`: on every 3D model `ground`, `center` and
    /// `height_m` (unless `params` declares the model's own `height_m`); on an
    /// image model that takes a `mask` reference or a `region`,
    /// `mask_grow_px`, `preserve_unmasked` and `feather_px`. No name is in
    /// both maps. Computed by the server and served by `GET /v1/models/{id}`,
    /// so read-only (`readOnly` in the OpenAPI document): a catalog row that
    /// sets it is refused, and the admin API reads and ignores it, as it
    /// does `status`, so a served schema posted back to `POST`/`PUT
    /// /v1/admin/models` works.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    #[schema(value_type = HashMap<String, serde_json::Value>)]
    pub litegen_params: std::collections::BTreeMap<String, ParamSpec>,
    #[serde(default)] pub tags: Vec<String>,
}

impl ModelSchema {
    /// Whether the model edits part of an image (3D doc §4): an image model
    /// that takes a `mask` reference or a `region`. Such a model takes
    /// `mask_grow_px`, `preserve_unmasked` and `feather_px`, and only such a
    /// model may promise `preserves_unmasked`. The one predicate the loader,
    /// the validator and the listing share.
    pub fn edits_part_of_an_image(&self) -> bool {
        self.media_type == MediaType::Image
            && (self.ref_inputs.as_ref().is_some_and(|r| r.roles.contains_key("mask"))
                || matches!(self.params.get("region"), Some(ParamSpec::Region(_))))
    }

    /// The declared `default` of a `string` or `aspect_ratio` param: what a
    /// request that leaves it out is priced at, so what an adapter sends in
    /// its place (E10), never a default the vendor picks for itself.
    pub fn string_default(&self, param: &str) -> Option<&str> {
        match self.params.get(param)? {
            ParamSpec::String(s) => s.default.as_deref(),
            ParamSpec::AspectRatio(s) => s.default.as_deref(),
            _ => None,
        }
    }

    /// The model's `output_formats` spec, normalising the superseded scalar
    /// `output_format` spelling into the plural shape.
    ///
    /// A legacy scalar spec becomes `max_items: 1` — which is the honest
    /// reading, since a model declaring the scalar could only ever emit one
    /// container per job.
    pub fn output_formats_spec(&self) -> Option<ParamSpecStringArray> {
        // An image model's `output_format` is its image container (L9), not
        // this superseded 3D spelling.
        if self.media_type != MediaType::Model3d {
            return None;
        }
        match self.params.get("output_formats") {
            Some(ParamSpec::StringArray(s)) => return Some(s.clone()),
            // A wrong-kinded `output_formats` is a catalog bug, not a missing
            // param. The loader rejects it at boot; refuse to guess here.
            Some(_) => return None,
            None => {}
        }
        match self.params.get("output_format") {
            Some(ParamSpec::String(s)) => Some(ParamSpecStringArray {
                enum_values: s.enum_values.clone(),
                max_items: Some(1),
                default: s.default.clone().into_iter().collect(),
                derivable_values: Vec::new(),
                label: s.label.clone(),
                description: s.description.clone(),
            }),
            _ => None,
        }
    }

    /// The containers a generation on this model is actually committed to
    /// producing, given what the caller asked for (already validated).
    ///
    /// Never empty: a model that declares no `output_formats` and a caller who
    /// names none still get GLB, which is what makes the delivered-vs-requested
    /// guard meaningful on every 3D model rather than only the ones whose yaml
    /// opted in.
    pub fn resolve_output_formats(&self, requested: Option<&[String]>) -> Vec<String> {
        let lower = |v: &[String]| -> Vec<String> {
            v.iter().map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect()
        };
        if let Some(r) = requested {
            let r = lower(r);
            if !r.is_empty() {
                return r;
            }
        }
        if let Some(spec) = self.output_formats_spec() {
            let d = lower(&spec.default);
            if !d.is_empty() {
                return d;
            }
        }
        vec![crate::types::DEFAULT_MODEL3D_FORMAT.to_string()]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelsFile {
    pub models: Vec<ModelSchema>,
}

/// Vendor fields no caller sets through `extra`, whatever a row allowlists
/// (E4a): where the vendor sends a job's result, where it stores it, and
/// whether it publishes it. litegen polls every job itself and fires the
/// key's own webhook, handing files out in the app's access mode
/// (docs/storage-access.md); a vendor callback would go around both. Each
/// entry is a key with its reason. Keys compare without case, `_` or `-`
/// ([`gateway_owned_extra`]), so `isPublic` and `CallbackURL` count too.
pub const GATEWAY_OWNED_EXTRAS: &[(&str, &str)] = &[
    ("callback_url", CALLBACK_REASON),
    ("webhook_url", CALLBACK_REASON),
    ("notify_url", CALLBACK_REASON),
    ("storage_uri", STORAGE_REASON),
    ("output_data_config", STORAGE_REASON),
    ("is_public", PUBLIC_REASON),
    ("public", PUBLIC_REASON),
];

const CALLBACK_REASON: &str = "litegen polls the vendor and fires your key's webhook";
const STORAGE_REASON: &str = "litegen keeps outputs where your app's storage says";
const PUBLIC_REASON: &str = "outputs stay inside litegen's access rules";

/// Why litegen owns the vendor field `key`, or `None` for a field a row may
/// allowlist: one of [`GATEWAY_OWNED_EXTRAS`], or any key with `callback` or
/// `webhook` in it (`status_callback`, `fal_webhook`), compared in lower case
/// with `_` and `-` removed.
pub fn gateway_owned_extra(key: &str) -> Option<&'static str> {
    let norm = |k: &str| k.chars().filter(|c| !matches!(c, '_' | '-')).collect::<String>().to_lowercase();
    let key = norm(key);
    GATEWAY_OWNED_EXTRAS
        .iter()
        .find(|(owned, _)| norm(owned) == key)
        .map(|(_, why)| *why)
        .or_else(|| (key.contains("callback") || key.contains("webhook")).then_some(CALLBACK_REASON))
}

/// Canonical param names. The loader rejects unknown keys to keep yaml honest.
pub const KNOWN_PARAMS: &[&str] = &[
    "camera",
    "audio",
    "loop",
    "output_format",
    "background",
    "negative_prompt",
    "seed",
    "steps",
    "guidance_scale",
    "strength",
    "quality",
    "style",
    "size",
    "aspect_ratio",
    "duration_seconds",
    "resolution",
    "fps",
    // 3D (model3d). First-class rather than extra_allowlist passthrough so the
    // aipix input panel and the litegen Playground both render real controls.
    //
    // `output_formats` (plural, StringArray) is canonical. `output_format`
    // (scalar String) is the superseded spelling, still accepted so that a
    // deployment pointing at its own models dir — the visgotti instance does —
    // keeps loading across the release that introduces the plural. Resolution
    // order lives in `output_formats_spec`.
    "output_formats",
    "output_format",
    "texture",
    "pbr",
    "target_polycount",
    "symmetry",
    "topology",
    "rig",
    // 3D operations (3D doc §3A): the mesh an operation works on.
    "source_model",
    // Rigging (3D doc §3B).
    "skeleton",
    "joint_naming",
    "height_m",
    // Animation (3D doc §3C); `fps` is shared with video.
    "animations",
    // Partial image edits (3D doc §4).
    "region",
    // Multiview images (3D doc §3E).
    "views",
    // Promptable segmentation (PixelGenie L7).
    "prompts",
    // Texturing (3D doc §3F).
    "texture_resolution",
    "keep_uvs",
    // Expanded canvases (3D doc §4).
    "expand",
    // Per-box prompts and masks named in text (3D doc §4).
    "layout",
    "mask_prompt",
    // Native pixel art (PixelGenie L16).
    "palette",
    "directions",
    // Video operations (3D doc §5).
    "source_video",
    "direction",
    "time_range",
    "replace",
];

/// The `ParamSpec` kind(s) a canonical param may be declared with: exactly the
/// kind its validator reads.
///
/// One table, so a param's typing is decided once. A param declared with any
/// other kind would load, advertise itself through `GET /v1/models`, and then
/// be refused by the validator as unsupported — so the loader rejects it at
/// boot instead. Empty for a name outside [`KNOWN_PARAMS`].
pub fn canonical_param_kinds(name: &str) -> &'static [&'static str] {
    match name {
        "seed" => &["seed"],
        "negative_prompt" | "quality" | "style" | "resolution" => &["string"],
        "steps" | "fps" => &["int"],
        "guidance_scale" | "strength" => &["float"],
        // Most vendors take fractional seconds; Runway takes whole ones.
        "duration_seconds" => &["float", "int"],
        "size" => &["size"],
        "aspect_ratio" => &["aspect_ratio"],
        "output_formats" | "views" => &["string_array"],
        "output_format" | "background" | "symmetry" | "topology" | "skeleton" | "joint_naming" => &["string"],
        "height_m" => &["float"],
        "animations" => &["animation_list"],
        "region" => &["region"],
        "texture" | "pbr" | "rig" | "audio" | "loop" | "keep_uvs" => &["bool"],
        "texture_resolution" => &["int"],
        "camera" => &["string"],
        "target_polycount" => &["int"],
        "source_model" => &["source_model"],
        "prompts" => &["segmentation_prompts"],
        "expand" => &["expand"],
        "layout" => &["layout"],
        "mask_prompt" | "direction" | "replace" => &["string"],
        "source_video" => &["source_video"],
        "time_range" => &["time_range"],
        "palette" => &["palette"],
        "directions" => &["int"],
        _ => &[],
    }
}

/// Why a value does not fit a typed extra (E4b): see [`ParamSpec::extra_misfit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtraMisfit {
    /// Not the JSON type the extra's kind takes, which it names ("a string").
    Type(&'static str),
    /// A number outside the extra's `min`/`max`, which it names.
    Range(String),
    /// Not one of the extra's listed values, which it names.
    NotListed(String),
    /// A string longer than the extra's `max_length`.
    TooLong(usize),
    /// A list longer than the extra's `max_items`.
    TooMany(usize),
    /// A string its `pattern` does not match, which it names (N4).
    Pattern(String),
}

impl std::fmt::Display for ExtraMisfit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtraMisfit::Type(t) => write!(f, "it must be {t}"),
            ExtraMisfit::Range(r) => write!(f, "it must lie in {r}"),
            ExtraMisfit::NotListed(v) => write!(f, "it must be one of {v}"),
            ExtraMisfit::TooLong(n) => write!(f, "it must be at most {n} characters long"),
            ExtraMisfit::TooMany(n) => write!(f, "it may hold at most {n} items"),
            ExtraMisfit::Pattern(p) => write!(f, "it must match the pattern {p}"),
        }
    }
}

impl ParamSpec {
    /// The kinds an `extra_params` entry may have (E4b): a vendor field's
    /// own JSON type. litegen's other kinds mean nothing to a vendor.
    pub const EXTRA_KINDS: &'static [&'static str] = &["bool", "int", "float", "string", "string_array"];

    /// How `v` misfits this spec as a typed extra's value, or `None` when it
    /// fits (E4b). `null` always fits: it is sent as given, as before extras
    /// were typed.
    pub fn extra_misfit(&self, v: &serde_json::Value) -> Option<ExtraMisfit> {
        fn bounds<T: std::fmt::Display>(min: Option<T>, max: Option<T>) -> String {
            match (min, max) {
                (Some(a), Some(b)) => format!("{a}..={b}"),
                (Some(a), None) => format!("{a}.."),
                (None, Some(b)) => format!("..={b}"),
                (None, None) => "any".into(),
            }
        }
        if v.is_null() {
            return None;
        }
        match self {
            ParamSpec::Bool(_) => (!v.is_boolean()).then_some(ExtraMisfit::Type("true or false")),
            ParamSpec::Int(s) => {
                let Some(n) = as_integer(v) else { return Some(ExtraMisfit::Type("an integer")) };
                if s.min.is_some_and(|m| n < m) || s.max.is_some_and(|m| n > m) {
                    return Some(ExtraMisfit::Range(bounds(s.min, s.max)));
                }
                (!s.values.is_empty() && !s.values.contains(&n)).then(|| ExtraMisfit::NotListed(format!("{:?}", s.values)))
            }
            ParamSpec::Float(s) => {
                let Some(x) = v.as_f64() else { return Some(ExtraMisfit::Type("a number")) };
                if s.min.is_some_and(|m| x < m) || s.max.is_some_and(|m| x > m) {
                    return Some(ExtraMisfit::Range(bounds(s.min, s.max)));
                }
                (!s.values.is_empty() && !s.values.iter().any(|a| (a - x).abs() < 1e-9))
                    .then(|| ExtraMisfit::NotListed(format!("{:?}", s.values)))
            }
            ParamSpec::String(s) => {
                let Some(t) = v.as_str() else { return Some(ExtraMisfit::Type("a string")) };
                if let Some(max) = s.max_length.filter(|max| t.chars().count() > *max) {
                    return Some(ExtraMisfit::TooLong(max));
                }
                if !s.enum_values.is_empty() && !s.enum_values.iter().any(|e| e == t) {
                    return Some(ExtraMisfit::NotListed(format!("{:?}", s.enum_values)));
                }
                s.pattern.as_ref().filter(|_| !s.matches_pattern(t)).map(|p| ExtraMisfit::Pattern(p.clone()))
            }
            ParamSpec::StringArray(s) => {
                let Some(items) = v.as_array().filter(|a| a.iter().all(serde_json::Value::is_string)) else {
                    return Some(ExtraMisfit::Type("a list of strings"));
                };
                if let Some(max) = s.max_items.filter(|max| items.len() > *max) {
                    return Some(ExtraMisfit::TooMany(max));
                }
                items
                    .iter()
                    .any(|i| !s.enum_values.is_empty() && !s.enum_values.iter().any(|e| i.as_str() == Some(e.as_str())))
                    .then(|| ExtraMisfit::NotListed(format!("{:?}", s.enum_values)))
            }
            // Never an extra's kind: the loader refuses it.
            _ => None,
        }
    }

    /// A typed extra's value as the vendor is sent it, once it fits: an
    /// `int` extra written as an integral float (`2.0`) goes as the integer
    /// (`2`), which a strict integer field takes. Anything else is unchanged.
    pub fn normalize_extra(&self, v: &mut serde_json::Value) {
        if let ParamSpec::Int(_) = self {
            if v.is_f64() {
                if let Some(n) = as_integer(v) {
                    *v = n.into();
                }
            }
        }
    }

    /// The `kind` tag this spec is written with in yaml and on the wire.
    pub fn kind(&self) -> &'static str {
        match self {
            ParamSpec::Bool(_) => "bool",
            ParamSpec::Int(_) => "int",
            ParamSpec::Float(_) => "float",
            ParamSpec::String(_) => "string",
            ParamSpec::StringArray(_) => "string_array",
            ParamSpec::Size(_) => "size",
            ParamSpec::AspectRatio(_) => "aspect_ratio",
            ParamSpec::Seed(_) => "seed",
            ParamSpec::SourceModel(_) => "source_model",
            ParamSpec::AnimationList(_) => "animation_list",
            ParamSpec::Region(_) => "region",
            ParamSpec::SegmentationPrompts(_) => "segmentation_prompts",
            ParamSpec::Expand(_) => "expand",
            ParamSpec::Layout(_) => "layout",
            ParamSpec::SourceVideo(_) => "source_video",
            ParamSpec::TimeRange(_) => "time_range",
            ParamSpec::Palette(_) => "palette",
        }
    }
}

fn default_true() -> bool { true }

/// `v` as an integer, when it is one: an integer, or a number with no
/// fraction written as a float (`2.0`), which JSON Schema's `integer` takes
/// too (passthrough review #6). `None` for a fraction or for a number outside
/// what an `i64` holds.
fn as_integer(v: &serde_json::Value) -> Option<i64> {
    // [-2^63, 2^63): every integral float here converts exactly.
    const RANGE: std::ops::Range<f64> = -9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0;
    v.as_i64().or_else(|| v.as_f64().filter(|x| x.fract() == 0.0 && RANGE.contains(x)).map(|x| x as i64))
}

/// `ModelSchema.status` as read: any value is accepted and dropped, since the
/// status is derived from the dates whenever the model is served.
fn ignore_served_status<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ModelStatus, D::Error> {
    serde::de::IgnoredAny::deserialize(d).map(|_| ModelStatus::default())
}

#[cfg(test)]
mod pattern_tests {
    use super::*;
    use std::sync::Arc;

    /// Passthrough review #3: a string param's pattern is compiled once, not
    /// on every request that sends the param. The registry hands each request
    /// its own copy of the schema, so the copies share it by the pattern's text.
    #[test]
    fn a_pattern_is_compiled_once_for_every_copy_of_its_spec() {
        let spec = ParamSpecString { pattern: Some("^[a-z]+$".into()), ..Default::default() };
        let copy = spec.clone();
        let (first, again) = (spec.compiled_pattern().expect("it compiles"), copy.compiled_pattern().expect("it compiles"));
        assert!(Arc::ptr_eq(&first, &again), "the pattern was compiled again for the next check");
        assert!(spec.matches_pattern("abc") && !copy.matches_pattern("ABC"));
        assert!(ParamSpecString::default().matches_pattern("anything"), "no pattern matches anything");
        let broken = ParamSpecString { pattern: Some("([a-z".into()), ..Default::default() };
        assert!(broken.compiled_pattern().is_none() && !broken.matches_pattern("a"), "a broken pattern matches nothing");
    }

    /// The cache keeps at most `capacity` patterns: past that, a new one is
    /// compiled for each use, as before, and the ones kept stay kept.
    #[test]
    fn a_full_cache_compiles_new_patterns_without_keeping_them() {
        let cache = PatternCache::new(1);
        let kept = cache.get("^a$").expect("compiles");
        assert!(Arc::ptr_eq(&kept, &cache.get("^a$").expect("compiles")), "the first pattern is kept");
        let (b1, b2) = (cache.get("^b$").expect("compiles"), cache.get("^b$").expect("compiles"));
        assert!(!Arc::ptr_eq(&b1, &b2), "a full cache keeps no more");
        assert!(b1.is_match("b") && !b1.is_match("a"));
        assert!(Arc::ptr_eq(&kept, &cache.get("^a$").expect("compiles")), "and keeps what it has");
    }
}
