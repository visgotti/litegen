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
    /// 3D: split `source_model` into named parts.
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
            MediaType::Image => matches!(self, Generate | Multiview | BackgroundSwap),
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
pub struct ModelCapabilityFlags {
    #[serde(default)] pub text_to_image: bool,
    #[serde(default)] pub image_to_image: bool,
    #[serde(default)] pub inpainting: bool,
    #[serde(default)] pub text_to_video: bool,
    #[serde(default)] pub image_to_video: bool,
    #[serde(default)] pub text_to_3d: bool,
    #[serde(default)] pub image_to_3d: bool,
    #[serde(default)] pub multiview_to_3d: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[schema(as = CapabilityModelPricing)]
pub struct ModelPricing {
    pub base_cost_usd: f64,
    #[serde(default)]
    pub variable_pricing: Option<serde_json::Value>,
}

/// How a model treats the request's `prompt`. Lengths count characters.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
}

// ─── ParamSpec ──────────────────────────────────────────────────────────────
//
// Each variant payload lives in its own named struct so utoipa emits a real
// component schema for it (instead of an anonymous oneOf entry). The wire
// format is unchanged thanks to `#[serde(tag = "kind")]` on the enum and
// serde's flattening of newtype variants.

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ParamSpecBool {
    #[serde(default)] pub default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ParamSpecInt {
    #[serde(default)] pub min: Option<i64>,
    #[serde(default)] pub max: Option<i64>,
    #[serde(default)] pub default: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ParamSpecFloat {
    #[serde(default)] pub min: Option<f64>,
    #[serde(default)] pub max: Option<f64>,
    #[serde(default)] pub default: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ParamSpecString {
    #[serde(default)] pub max_length: Option<usize>,
    #[serde(default)] pub enum_values: Vec<String>,
    #[serde(default)] pub pattern: Option<String>,
    #[serde(default)] pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

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
    /// Applied when the caller omits the param entirely. Empty means the
    /// resolver falls back to `glb`.
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
pub struct ParamSpecAspectRatio {
    pub allowed: Vec<String>,
    #[serde(default)] pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
}

/// The clips an `animate` model makes (`animations`, 3D doc §3C).
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
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
pub struct SizeSpecFreeform {
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
    #[serde(default)] pub multiple_of: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RefRoleSpec {
    #[serde(default)] pub required: bool,
    pub min_count: u32,
    pub max_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RefProviderFormatMultipart {
    pub field_map: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum RefProviderFormat {
    Url,
    Base64,
    Multipart(RefProviderFormatMultipart),
}

/// The axes a 3D model's vendor builds its meshes along (3D doc §2 rule 3).
/// Where they differ from glTF's (+Y up, +Z front), litegen adds a root
/// rotation and the manifest says `oriented_by: "litegen"`. An operation that
/// hands back its source's own geometry says `source` for both: its output
/// faces wherever the source did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
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
pub struct ModelSchema {
    pub id: String,
    pub provider: String,
    pub media_type: MediaType,
    /// What the model does: `generate`, or an operation on an existing asset
    /// such as `rig` or `animate`.
    #[serde(default)]
    pub task: ModelTask,
    /// 3D only: the axes the vendor builds its meshes along, declared once a
    /// run has shown them. Absent means unproven: the mesh is never turned and
    /// its manifest says `front: "unverified"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_orientation: Option<NativeOrientation>,
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
    #[serde(default)] pub tags: Vec<String>,
}

impl ModelSchema {
    /// The model's `output_formats` spec, normalising the superseded scalar
    /// `output_format` spelling into the plural shape.
    ///
    /// A legacy scalar spec becomes `max_items: 1` — which is the honest
    /// reading, since a model declaring the scalar could only ever emit one
    /// container per job.
    pub fn output_formats_spec(&self) -> Option<ParamSpecStringArray> {
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
pub struct ModelsFile {
    pub models: Vec<ModelSchema>,
}

/// Canonical param names. The loader rejects unknown keys to keep yaml honest.
pub const KNOWN_PARAMS: &[&str] = &[
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
        "output_formats" => &["string_array"],
        "output_format" | "symmetry" | "topology" | "skeleton" | "joint_naming" => &["string"],
        "height_m" => &["float"],
        "animations" => &["animation_list"],
        "texture" | "pbr" | "rig" => &["bool"],
        "target_polycount" => &["int"],
        "source_model" => &["source_model"],
        _ => &[],
    }
}

impl ParamSpec {
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
        }
    }
}

fn default_true() -> bool { true }
