use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// The container every 3D model is required to be able to emit, and what a
/// caller gets when nothing narrower was declared or asked for.
///
/// It is the only universal: every vendor with a public 3D API can produce GLB,
/// whereas FBX, USDZ, OBJ and STL are each missing from at least one of them.
/// It is also the only container the dashboard viewer renders.
pub const DEFAULT_MODEL3D_FORMAT: &str = "glb";

// ─── Generation Request / Response ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BaseGenerationRequest {
    /// Whether a prompt is required is decided per model, by its
    /// `prompt.required` in `GET /v1/models/{id}` — never by the wire format.
    /// Omitting it is the same as sending `""`, which an image-to-3D model
    /// with no prompt field (TRELLIS, Hunyuan3D v2) accepts and a text model
    /// answers with `prompt_required`.
    #[serde(default)]
    pub prompt: String,
    pub model: String,
    #[serde(default = "default_n")]
    pub n: u32,
    #[serde(default)]
    pub negative_prompt: Option<String>,
    #[serde(default)]
    pub seed: Option<i64>,
    #[serde(default)]
    pub reference_images: Vec<ReferenceImage>,
    #[serde(default = "default_true")]
    pub strict: bool,
    #[serde(default)]
    pub extra: Option<serde_json::Value>,
    /// Your own labels for the job (PixelGenie L17): a map of string values,
    /// keys of letters, digits, `_`, `.` and `-`, at most 1 KB as JSON. Kept on
    /// the generation, echoed on every answer and webhook, and filters
    /// `GET /v1/logs` and `GET /v1/stats` as `metadata.<key>=<value>`.
    #[serde(default)]
    #[schema(value_type = Option<std::collections::HashMap<String, String>>)]
    pub metadata: Option<serde_json::Value>,
}

/// The caller's `metadata`, once validated: what every answer echoes.
pub type RequestMetadata = std::collections::BTreeMap<String, String>;

impl BaseGenerationRequest {
    /// This request's `metadata` as the echo carries it.
    pub fn metadata_map(&self) -> Option<RequestMetadata> {
        serde_json::from_value(self.metadata.clone()?).ok()
    }
}

impl Generation {
    /// The `metadata` the caller sent with this job, kept on its row.
    pub fn request_metadata(&self) -> Option<RequestMetadata> {
        serde_json::from_value(self.metadata.as_ref()?.get("request")?.get("metadata")?.clone()).ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageGenerationRequest {
    #[serde(flatten)]
    pub base: BaseGenerationRequest,
    /// Edit only this rectangle of the `init` image (3D doc §4). A box-native
    /// model receives it as its own box; a mask-only one receives it drawn as
    /// the mask. With a `mask` too, the edit area is where they overlap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// The views a `multiview` model makes, one image each, in this order.
    /// Omitted means the model's default set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<ImageView>>)]
    pub views: Option<Vec<String>>,
    /// Grow the init image's canvas by this many pixels a side (3D doc §4);
    /// the model's `expand` spec says how much and whether it only centres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expand: Option<Expand>,
    /// Boxes for a text-to-image canvas (3D doc §4): objects and words
    /// placed by fractions of the canvas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Vec<LayoutItem>>,
    /// The area an edit replaces, named in text rather than drawn as a mask
    /// (3D doc §4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_prompt: Option<String>,
    /// Paste the init image's pixels back outside the edit area (3D doc §4),
    /// so they come back byte-identical from any vendor. Needs a mask or a
    /// region, a lossless container and an output the init's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve_unmasked: Option<bool>,
    /// With `preserve_unmasked`: blend from the init at the edit area's edge
    /// to the vendor's pixels this many pixels inside it (0, a hard edge, by
    /// default; at most 256).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feather_px: Option<u32>,
    /// Colours as `#rrggbb` (PixelGenie L16): every pixel one of them on a
    /// model whose palette is forced, a hint elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<Vec<String>>,
    /// A character in 4 or 8 facings, one labelled image each (PixelGenie L16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directions: Option<u32>,
    /// Grow the edit area outward by this many pixels: the vendor's own
    /// setting where it has one (sent explicitly, even as 0), else litegen
    /// dilates the mask itself. Default 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_grow_px: Option<u32>,
    /// The image container to return (PixelGenie L9): one of the model's
    /// `capabilities.output_formats`. Omitted means the vendor's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<ImageOutputFormat>)]
    pub output_format: Option<String>,
    /// `transparent` for a real alpha channel where the model supports it
    /// (`supports_transparent_background`); needs `png` or `webp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<ImageBackground>)]
    pub background: Option<String>,
    #[serde(default)] pub size: Option<String>,
    #[serde(default)] pub aspect_ratio: Option<String>,
    #[serde(default)] pub quality: Option<String>,
    #[serde(default)] pub style: Option<String>,
    #[serde(default)] pub steps: Option<u32>,
    #[serde(default)] pub guidance_scale: Option<f64>,
    #[serde(default)] pub strength: Option<f64>,
    #[serde(default = "default_response_format")]
    pub response_format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VideoGenerationRequest {
    #[serde(flatten)]
    pub base: BaseGenerationRequest,
    /// Absent means the model's own `duration_seconds` default (resolved by
    /// `resolve_duration_seconds`), not a fixed 5s: models whose minimum is
    /// higher (fal/ltx-2.3, bedrock nova-reel) rejected every request that
    /// omitted it.
    #[serde(default)]
    pub duration_seconds: Option<f64>,
    #[serde(default)] pub aspect_ratio: Option<String>,
    #[serde(default)] pub resolution: Option<String>,
    #[serde(default)] pub fps: Option<u32>,
    /// `fixed` for a still camera, where the model takes one
    /// (`supports_fixed_camera`, PixelGenie L10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<VideoCamera>)]
    pub camera: Option<String>,
    /// `false` for a silent clip, where the model can leave audio out
    /// (`supports_audio_toggle`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<bool>,
    /// `true` for a clip whose last frame is its first (`supports_loop`).
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub r#loop: Option<bool>,
    /// The clip an operation (`extend`, `retake`, `video_edit`) works on (3D
    /// doc §5): a litegen generation id, a URL or base64, as `source_model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_video: Option<SourceRef>,
    /// An `extend`: add before (`backward`) or after (`forward`) the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    /// A `retake`'s window, or a `video_edit`'s where the model takes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_range: Option<TimeRange>,
    /// A `retake`'s track: `video`, `audio` or `both`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<String>,
}

/// A video's camera (PixelGenie L10): `fixed` holds the shot still.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VideoCamera {
    Fixed,
    Auto,
}

/// What a finished video is, read from its bytes (PixelGenie L10).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct VideoMedia {
    pub duration_s: Option<f64>,
    pub fps: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub has_audio: Option<bool>,
}

impl VideoMedia {
    /// The first video asset's measurements, from a row's or a poll's
    /// `{"assets": [...]}` metadata.
    pub fn from_assets(metadata: &serde_json::Value) -> Option<Self> {
        let video = metadata["assets"].as_array()?.iter().find(|a| a["kind"] == "video")?;
        let media = serde_json::from_value::<Self>(video.clone()).ok()?;
        (media != Self::default()).then_some(media)
    }
}

/// An image container litegen asks a vendor for (PixelGenie L9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImageOutputFormat {
    Png,
    Webp,
    Jpeg,
}

impl ImageOutputFormat {
    pub const ALL: [ImageOutputFormat; 3] = [ImageOutputFormat::Png, ImageOutputFormat::Webp, ImageOutputFormat::Jpeg];

    pub const fn as_str(self) -> &'static str {
        match self {
            ImageOutputFormat::Png => "png",
            ImageOutputFormat::Webp => "webp",
            ImageOutputFormat::Jpeg => "jpeg",
        }
    }
}

/// An image's background (PixelGenie L9): `transparent` is a real alpha channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImageBackground {
    Transparent,
    Opaque,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReferenceImage {
    #[serde(rename = "type")]
    pub kind: RefImageKind,
    pub value: String,
    #[serde(default)]
    pub role: Option<String>,
    /// Seconds from the start of the clip, on a `keyframe` reference (3D doc
    /// §5): the video shows this image at that time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_s: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefImageKind {
    Base64,
    Url,
    Blob,
    /// A stored litegen image generation's id: litegen reads the image back,
    /// after the same access check `GET /v1/generations/{id}` makes.
    Generation,
}

fn default_n() -> u32 { 1 }
fn default_response_format() -> String { "url".to_string() }
fn default_true() -> bool { true }

/// A single generated image in the response.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageResult {
    /// URL of the generated image (if response_format=url).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Base64-encoded image data (if response_format=b64_json).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b64_json: Option<String>,
    /// Revised prompt (if provider modified the prompt, e.g. DALL-E 3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
    /// The view this image shows, from a `multiview` model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<ImageView>)]
    pub view: Option<String>,
    /// The camera's yaw for `view`, in degrees (see `ImageView`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw_deg: Option<f64>,
    /// The facing this image shows, from a `directions` request: `south`
    /// (towards the viewer), `south-east`, `east`, … (PixelGenie L16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    /// Content type of the image (e.g. "image/png").
    #[serde(default = "default_content_type")]
    pub content_type: String,
    /// The image's size in pixels, read from its bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Index in the batch.
    pub index: u32,
}

fn default_content_type() -> String {
    "image/png".to_string()
}

/// Response for an image generation request.
/// Follows OpenAI's images response format for compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageGenerationResponse {
    /// Unix timestamp of when the request was created.
    pub created: i64,
    /// Array of generated images.
    pub data: Vec<ImageResult>,
    /// The model that was used.
    pub model: String,
    /// The provider that handled the request.
    pub provider: String,
    /// Cost information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
    /// Unique request ID for tracking.
    pub id: String,
    /// The request's `metadata`, as sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<std::collections::HashMap<String, String>>)]
    pub metadata: Option<RequestMetadata>,
}

// ─── Video Generation ─────────────────────────────────────────────────────

/// Video generation can be async — this is the initial response.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VideoGenerationResponse {
    /// Unique generation ID for polling.
    pub id: String,
    /// Current status.
    pub status: GenerationStatus,
    /// The model used.
    pub model: String,
    /// The provider.
    pub provider: String,
    /// Video URL when completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_url: Option<String>,
    /// Progress percentage (0-100).
    pub progress: u8,
    /// Error message if failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Cost information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
    /// Unix timestamp.
    pub created: i64,
    /// What came back, read from the finished video (PixelGenie L10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<VideoMedia>,
    /// An operation's clip, stretch by stretch: from the source or generated
    /// (3D doc §5). Absent on a plain generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<VideoSegment>>,
    /// The request's `metadata`, as sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<std::collections::HashMap<String, String>>)]
    pub metadata: Option<RequestMetadata>,
}

// ─── 3D Model Generation ──────────────────────────────────────────────────

/// Every mesh container a 3D request may name in `output_formats`.
///
/// The closed vocabulary across all of litegen's 3D models. Which subset one
/// model can deliver is per model: `GET /v1/models/{id}` lists it under
/// `params.output_formats.enum_values`, and anything outside it is refused
/// with `param_enum_mismatch` before any vendor is called.
///
/// A container reaches a caller one of two ways. Either litegen derives it
/// locally from whatever readable container the vendor emitted — the set
/// `proxy::model3d_convert::derivable_formats` returns, served per model as
/// `params.output_formats.derivable_values` — and any number of those cost one
/// vendor job together; or the vendor emits it itself, one job each. Which is
/// which grows with `litegen-core/src/mesh/`, so this list is deliberately a
/// superset of every container any vendor or the converter produces.
///
/// Requests carry these as plain, case-insensitive strings — validation, not
/// deserialisation, owns the error, so it can name what IS deliverable. This
/// type pins the vocabulary for OpenAPI and for the catalog loader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Model3dFormat {
    /// Binary glTF 2.0 — the one container every 3D vendor emits.
    Glb,
    /// JSON glTF 2.0, buffers embedded so it is a single file.
    Gltf,
    Obj,
    Stl,
    Ply,
    Fbx,
    /// Zipped USD — the format AR Quick Look opens.
    Usdz,
    /// Text USD.
    Usda,
    #[serde(rename = "3mf")]
    ThreeMf,
    /// Collada 1.4.1.
    Dae,
}

impl Model3dFormat {
    pub const ALL: [Model3dFormat; 10] = [
        Model3dFormat::Glb,
        Model3dFormat::Gltf,
        Model3dFormat::Obj,
        Model3dFormat::Stl,
        Model3dFormat::Ply,
        Model3dFormat::Fbx,
        Model3dFormat::Usdz,
        Model3dFormat::Usda,
        Model3dFormat::ThreeMf,
        Model3dFormat::Dae,
    ];

    /// The wire spelling: lowercase, no dot — also the storage-key extension.
    pub const fn as_str(self) -> &'static str {
        match self {
            Model3dFormat::Glb => "glb",
            Model3dFormat::Gltf => "gltf",
            Model3dFormat::Obj => "obj",
            Model3dFormat::Stl => "stl",
            Model3dFormat::Ply => "ply",
            Model3dFormat::Fbx => "fbx",
            Model3dFormat::Usdz => "usdz",
            Model3dFormat::Usda => "usda",
            Model3dFormat::ThreeMf => "3mf",
            Model3dFormat::Dae => "dae",
        }
    }

    /// Case-insensitive, whitespace-tolerant — the same folding the validator
    /// applies to `output_formats`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        Self::ALL.into_iter().find(|f| f.as_str().eq_ignore_ascii_case(s))
    }
}

/// Face topology a 3D request may ask for (`topology`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Model3dTopology {
    /// Triangulated output — what every vendor emits by default.
    Triangle,
    /// Quad-dominant remesh, for DCC tools and subdivision.
    Quad,
}

impl Model3dTopology {
    pub const ALL: [Model3dTopology; 2] = [Model3dTopology::Triangle, Model3dTopology::Quad];

    pub const fn as_str(self) -> &'static str {
        match self {
            Model3dTopology::Triangle => "triangle",
            Model3dTopology::Quad => "quad",
        }
    }
}

/// Symmetry handling a 3D request may ask for (`symmetry`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Model3dSymmetry {
    Off,
    /// Let the model decide from the input.
    Auto,
    On,
}

impl Model3dSymmetry {
    pub const ALL: [Model3dSymmetry; 3] =
        [Model3dSymmetry::Off, Model3dSymmetry::Auto, Model3dSymmetry::On];

    pub const fn as_str(self) -> &'static str {
        match self {
            Model3dSymmetry::Off => "off",
            Model3dSymmetry::Auto => "auto",
            Model3dSymmetry::On => "on",
        }
    }
}

/// The skeleton a rig is built for (`skeleton`, 3D doc §3B): Tripo's
/// `rig_type` values, with its `biped` named `humanoid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Model3dSkeleton {
    Humanoid,
    Quadruped,
    Hexapod,
    Octopod,
    Avian,
    Serpentine,
    Aquatic,
    /// A skeleton matching none of the named topologies.
    Other,
}

impl Model3dSkeleton {
    pub const ALL: [Model3dSkeleton; 8] = [
        Model3dSkeleton::Humanoid,
        Model3dSkeleton::Quadruped,
        Model3dSkeleton::Hexapod,
        Model3dSkeleton::Octopod,
        Model3dSkeleton::Avian,
        Model3dSkeleton::Serpentine,
        Model3dSkeleton::Aquatic,
        Model3dSkeleton::Other,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Model3dSkeleton::Humanoid => "humanoid",
            Model3dSkeleton::Quadruped => "quadruped",
            Model3dSkeleton::Hexapod => "hexapod",
            Model3dSkeleton::Octopod => "octopod",
            Model3dSkeleton::Avian => "avian",
            Model3dSkeleton::Serpentine => "serpentine",
            Model3dSkeleton::Aquatic => "aquatic",
            Model3dSkeleton::Other => "other",
        }
    }
}

/// The joint names a rig is delivered with (`joint_naming`, 3D doc §3B).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Model3dJointNaming {
    /// Whatever the vendor names them.
    Vendor,
    /// VRM 1.0 humanoid bone names.
    Vrm,
    /// Mixamo's `mixamorig:` names.
    Mixamo,
    /// Unity's `HumanBodyBones`.
    Unity,
    /// Unreal Engine's mannequin.
    Unreal,
}

impl Model3dJointNaming {
    pub const ALL: [Model3dJointNaming; 5] = [
        Model3dJointNaming::Vendor,
        Model3dJointNaming::Vrm,
        Model3dJointNaming::Mixamo,
        Model3dJointNaming::Unity,
        Model3dJointNaming::Unreal,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Model3dJointNaming::Vendor => "vendor",
            Model3dJointNaming::Vrm => "vrm",
            Model3dJointNaming::Mixamo => "mixamo",
            Model3dJointNaming::Unity => "unity",
            Model3dJointNaming::Unreal => "unreal",
        }
    }
}

/// The canonical animation presets (`animations[].preset`, 3D doc §3C). A
/// model maps the ones it plays onto its own preset ids; a request may name
/// either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Model3dAnimationPreset {
    Idle,
    Walk,
    Run,
    Jump,
    Fall,
    AttackMelee,
    AttackRanged,
    Cast,
    Hurt,
    Death,
    Crouch,
    Climb,
    Turn,
}

impl Model3dAnimationPreset {
    pub const ALL: [Model3dAnimationPreset; 13] = [
        Model3dAnimationPreset::Idle,
        Model3dAnimationPreset::Walk,
        Model3dAnimationPreset::Run,
        Model3dAnimationPreset::Jump,
        Model3dAnimationPreset::Fall,
        Model3dAnimationPreset::AttackMelee,
        Model3dAnimationPreset::AttackRanged,
        Model3dAnimationPreset::Cast,
        Model3dAnimationPreset::Hurt,
        Model3dAnimationPreset::Death,
        Model3dAnimationPreset::Crouch,
        Model3dAnimationPreset::Climb,
        Model3dAnimationPreset::Turn,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Model3dAnimationPreset::Idle => "idle",
            Model3dAnimationPreset::Walk => "walk",
            Model3dAnimationPreset::Run => "run",
            Model3dAnimationPreset::Jump => "jump",
            Model3dAnimationPreset::Fall => "fall",
            Model3dAnimationPreset::AttackMelee => "attack_melee",
            Model3dAnimationPreset::AttackRanged => "attack_ranged",
            Model3dAnimationPreset::Cast => "cast",
            Model3dAnimationPreset::Hurt => "hurt",
            Model3dAnimationPreset::Death => "death",
            Model3dAnimationPreset::Crouch => "crouch",
            Model3dAnimationPreset::Climb => "climb",
            Model3dAnimationPreset::Turn => "turn",
        }
    }
}

/// A side of the subject a view shows (3D doc §3E). Its `yaw_deg` is the
/// camera's position around +Y, measured from the subject's front (+Z)
/// towards +X — and glTF puts the subject's left at +X — so `front` is 0,
/// `left` 90, `back` 180 and `right` 270. `top` and `bottom` have no yaw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ImageView {
    Front,
    FrontLeft,
    Left,
    BackLeft,
    Back,
    BackRight,
    Right,
    FrontRight,
    Top,
    Bottom,
}

impl ImageView {
    pub const ALL: [ImageView; 10] = [
        ImageView::Front,
        ImageView::FrontLeft,
        ImageView::Left,
        ImageView::BackLeft,
        ImageView::Back,
        ImageView::BackRight,
        ImageView::Right,
        ImageView::FrontRight,
        ImageView::Top,
        ImageView::Bottom,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ImageView::Front => "front",
            ImageView::FrontLeft => "front-left",
            ImageView::Left => "left",
            ImageView::BackLeft => "back-left",
            ImageView::Back => "back",
            ImageView::BackRight => "back-right",
            ImageView::Right => "right",
            ImageView::FrontRight => "front-right",
            ImageView::Top => "top",
            ImageView::Bottom => "bottom",
        }
    }

    /// The camera's yaw for this view; `None` for `top` and `bottom`.
    pub const fn yaw_deg(self) -> Option<f64> {
        match self {
            ImageView::Front => Some(0.0),
            ImageView::FrontLeft => Some(45.0),
            ImageView::Left => Some(90.0),
            ImageView::BackLeft => Some(135.0),
            ImageView::Back => Some(180.0),
            ImageView::BackRight => Some(225.0),
            ImageView::Right => Some(270.0),
            ImageView::FrontRight => Some(315.0),
            ImageView::Top | ImageView::Bottom => None,
        }
    }

    pub fn parse(s: &str) -> Option<ImageView> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

/// The smallest and largest output edge, in pixels (PixelGenie L16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PixelSize {
    pub min: u32,
    pub max: u32,
}

/// A window of a clip, in seconds from its start (3D doc §5).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TimeRange {
    pub start_s: f64,
    pub end_s: f64,
}

/// Where a stretch of a finished clip came from (3D doc §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SegmentOrigin {
    Source,
    Generated,
}

/// One stretch of a finished operation's clip, in its own seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct VideoSegment {
    pub start_s: f64,
    pub end_s: f64,
    pub origin: SegmentOrigin,
}

/// Where a layout box sits, as fractions of the canvas (0 to 1, top-left
/// origin): the output's size may not be known yet.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LayoutBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// One box of a text-to-image layout (3D doc §4). Boxes guide the layout;
/// no vendor clips to them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LayoutItem {
    #[serde(rename = "box")]
    pub bounds: LayoutBox,
    pub kind: crate::capabilities::LayoutKind,
    /// What goes there: required for an `object` box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The words to render: required for a `text` box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Pixels to add to each side of the init image's canvas (3D doc §4): the
/// result is the whole canvas, with the source at (`left`, `top`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
pub struct Expand {
    #[serde(default)]
    pub left: u32,
    #[serde(default)]
    pub top: u32,
    #[serde(default)]
    pub right: u32,
    #[serde(default)]
    pub bottom: u32,
}

/// A rectangle of an image, in its pixels: top-left origin, ends exclusive
/// (it covers `x..x+w` by `y..y+h`). Named keys, never arrays: vendors
/// disagree on axis order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

// ─── Segmentation (PixelGenie L7) ─────────────────────────────────────────

/// One thing to select, in the image's pixels (top-left origin). Point and
/// box prompts together select ONE object; a text prompt selects every
/// instance of what it names and cannot be mixed with them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SegmentationPrompt {
    /// A spot on the object (`label: 1`, the default) or one to leave out
    /// of it (`label: 0`).
    Point {
        x: u32,
        y: u32,
        #[serde(default = "point_on")]
        label: u8,
    },
    /// A box around the object; ends exclusive, as `Region`.
    Box { x: u32, y: u32, w: u32, h: u32 },
    /// What to select, named.
    Text { text: String },
}

fn point_on() -> u8 {
    1
}

impl SegmentationPrompt {
    pub fn kind(&self) -> crate::capabilities::SegmentationPromptType {
        use crate::capabilities::SegmentationPromptType as T;
        match self {
            SegmentationPrompt::Point { .. } => T::Point,
            SegmentationPrompt::Box { .. } => T::Box,
            SegmentationPrompt::Text { .. } => T::Text,
        }
    }
}

/// `POST /v1/images/segmentations` (PixelGenie L7). Answered at once.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageSegmentationRequest {
    /// A model whose `task` is `segment`.
    pub model: String,
    /// The image: base64, or a `data:` URI. PNG, JPEG, WebP or GIF.
    pub image: String,
    /// What to select. Point and box prompts together select one object;
    /// a text prompt selects every instance of what it names.
    pub prompts: Vec<SegmentationPrompt>,
    /// Your own labels for the call, as on a generation (L17).
    #[serde(default)]
    #[schema(value_type = Option<std::collections::HashMap<String, String>>)]
    pub metadata: Option<serde_json::Value>,
}

/// What the prompts selected.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageSegmentationResponse {
    /// Unique request ID for tracking.
    pub id: String,
    /// Unix timestamp of when the request was created.
    pub created: i64,
    pub model: String,
    pub provider: String,
    /// One per object selected, best score first. Empty when nothing was
    /// found.
    pub masks: Vec<SegmentationMask>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
    /// The request's `metadata`, as sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<std::collections::HashMap<String, String>>)]
    pub metadata: Option<RequestMetadata>,
}

/// One selected object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SegmentationMask {
    /// A grayscale PNG the size of the image: white (255) where selected,
    /// black (0) elsewhere.
    pub png_base64: String,
    /// The vendor's confidence, 0–1; null when it gives none.
    #[schema(required = true)]
    pub score: Option<f64>,
    /// The smallest box around the white pixels, measured by litegen.
    #[serde(rename = "box")]
    pub bounds: Region,
}

/// One clip an `animate` request asks for (3D doc §3C).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AnimationRequest {
    /// The clip's name in the delivered file: unique within the request, 1–64
    /// characters of `[A-Za-z0-9_.-]`.
    pub name: String,
    /// A canonical preset (`Model3dAnimationPreset`) or one of the model's
    /// own preset ids. Exactly one of `preset` and `prompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Text-to-motion, where the model offers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// A prompt clip's length in seconds, within the model's
    /// `text_to_motion` range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f64>,
    /// Keep the root in place horizontally (vertical movement stays).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_place: Option<bool>,
    /// A loop hint, recorded in the manifest; nothing is resampled to force a
    /// loop.
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_requested: Option<bool>,
}

/// OpenAPI schema for the superseded `output_format`: the format vocabulary,
/// marked deprecated.
///
/// Hand-built because utoipa drops `deprecated` from every shape it derives for
/// this field — a `$ref`, the `oneOf: [null, …]` an `Option` becomes, and an
/// inline enum alike — and a deprecation no client generator can see is not
/// one.
fn deprecated_output_format_schema() -> utoipa::openapi::schema::Object {
    utoipa::openapi::schema::ObjectBuilder::new()
        .schema_type(utoipa::openapi::schema::Type::String)
        .enum_values(Some(Model3dFormat::ALL.iter().map(|f| f.as_str())))
        .deprecated(Some(utoipa::openapi::Deprecated::True))
        .description(Some(
            "Superseded by `output_formats`. A lone value is folded into it; when both \
             are sent, `output_formats` wins and this one is reported dropped.",
        ))
        .build()
}

/// Request body for `POST /v1/models3d/generations` and `POST /v1/models3d/cost`.
///
/// The mode is inferred, never passed: no `reference_images` is text-to-3D, an
/// `init` image is image-to-3D, and `view-*` images are multiview-to-3D.
///
/// Every 3D param is optional and model-dependent; `GET /v1/models/{id}` →
/// `params` says which a model takes. One it does not take is refused with
/// `param_unsupported` when `strict` is true (the default), and dropped — and
/// named in the `X-Litegen-Dropped-Params` response header — when `strict` is
/// false. A supported param with a bad VALUE is refused in both modes.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Model3dGenerationRequest {
    #[serde(flatten)]
    pub base: BaseGenerationRequest,
    /// Mesh containers to deliver, e.g. `["glb", "fbx"]`. A promise, not a
    /// hint: every format named here is present on a `completed` generation,
    /// or the generation ends `failed`. Omitted means the model's declared
    /// default, and `["glb"]` when it declares none. Case-insensitive;
    /// duplicates collapse.
    ///
    /// Any set of the containers litegen derives (the model's served
    /// `derivable_values`) costs one vendor job between them; every other
    /// container costs one job of its own. A request needing more vendor jobs
    /// than the model's `max_items` is refused with `param_too_many`.
    #[serde(default)]
    #[schema(value_type = Option<Vec<Model3dFormat>>)]
    pub output_formats: Option<Vec<String>>,
    /// Superseded by `output_formats`. Its OpenAPI schema is hand-built by
    /// `deprecated_output_format_schema`.
    #[serde(default)]
    #[schema(schema_with = deprecated_output_format_schema, required = false)]
    pub output_format: Option<String>,
    /// Texture the mesh, rather than returning bare geometry.
    #[serde(default)] pub texture: Option<bool>,
    /// PBR materials. `false` asks for flat shading, not "no material".
    #[serde(default)] pub pbr: Option<bool>,
    /// Approximate triangle budget for the mesh, within the model's declared
    /// `target_polycount` range.
    #[serde(default)] pub target_polycount: Option<u32>,
    /// Symmetry handling.
    #[serde(default)]
    #[schema(value_type = Option<Model3dSymmetry>)]
    pub symmetry: Option<String>,
    /// Face topology of the output mesh.
    #[serde(default)]
    #[schema(value_type = Option<Model3dTopology>)]
    pub topology: Option<String>,
    /// Auto-rig: deliver the mesh with a skeleton.
    #[serde(default)] pub rig: Option<bool>,
    /// The skeleton to rig for, from the model's `skeleton` enum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Model3dSkeleton>)]
    pub skeleton: Option<String>,
    /// The joint names to deliver the rig with. `vendor` (the default) keeps
    /// the vendor's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Model3dJointNaming>)]
    pub joint_naming: Option<String>,
    /// The delivered mesh's height in metres (3D doc §3D): litegen scales
    /// any mesh it delivers to it, and a rig model whose vendor takes a
    /// height gets it too. At most 10,000 m, or within a model's own
    /// `height_m` range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_m: Option<f64>,
    /// Put the mesh's lowest point at y = 0 (3D doc §3D). litegen applies it
    /// on delivery, to any model's mesh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground: Option<bool>,
    /// Centre the mesh's bounds on x and z (3D doc §3D). litegen applies it
    /// on delivery, to any model's mesh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<bool>,
    /// The clips to make, in order: one glTF animation per entry, named
    /// exactly its `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animations: Option<Vec<AnimationRequest>>,
    /// Frames per second of the delivered clips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    /// A `texture` model's map size in pixels, one of its
    /// `texture_resolutions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture_resolution: Option<u32>,
    /// Paint the source's own UV layout rather than a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_uvs: Option<bool>,
    /// The mesh an operation (`rig`, `animate`, `texture`, `remesh`,
    /// `segment`) works on, required by every model whose task is not
    /// `generate`. A litegen generation id chains a step onto the last; a URL
    /// or base64 bytes bring a mesh of your own and name its `format`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_model: Option<SourceRef>,
}

/// Where an operation's input comes from: a stored litegen generation, or a
/// file of your own.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SourceRef {
    #[serde(rename = "type")]
    pub kind: SourceKind,
    /// The generation id, the URL, or the base64 bytes (a `data:` URI prefix
    /// is allowed).
    pub value: String,
    /// The container, from the `Model3dFormat` vocabulary. Required for `url`
    /// and `base64`; a generation's is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Model3dFormat>)]
    pub format: Option<String>,
}

/// How a `SourceRef`'s `value` is to be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A litegen generation id the caller can read (`GET /v1/generations/{id}`).
    Generation,
    /// A public http(s) URL litegen downloads.
    Url,
    /// The file's bytes, base64-encoded.
    Base64,
}

/// What a `Model3dAsset` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Model3dAssetKind {
    /// A mesh container: one per format the request was promised.
    Mesh,
    /// A texture map the vendor returned beside the mesh (e.g. Rodin's PBR maps).
    Texture,
    /// A 2D rendered preview of the result.
    Preview,
    /// The vendor's own file, delivered whenever litegen changed anything in
    /// the meshes it delivers (a rotation, a transcode, a rename), so nothing
    /// is lost by normalising.
    Original,
}

/// One file produced by a 3D generation, re-hosted on litegen storage (the
/// app's own bucket when it configured one).
///
/// A `completed` generation carries one `mesh` asset for every format in its
/// `output_formats` — never fewer; a missing one fails the generation instead —
/// plus whatever textures and previews the vendor returned.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Model3dAsset {
    pub kind: Model3dAssetKind,
    /// Always absolute: a root-relative path is indistinguishable from an
    /// outage to a client fetching it from a worker. Meshes live at
    /// `…/{generation_id}/model.{format}`.
    pub url: String,
    /// Lowercase container without a dot. Meshes use the `Model3dFormat`
    /// vocabulary; textures and previews their image format (`png`, `jpg`,
    /// `webp`). A plain string so a client tolerates a format added later.
    pub format: String,
    /// Size of the stored file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Triangle count, on meshes whose vendor reports one. A container litegen
    /// derived carries its source's count — conversion keeps every triangle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polycount: Option<u32>,
    /// Pixel width, on textures and previews whose vendor reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Pixel height, on textures and previews whose vendor reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Lowercase hex SHA-256 of the stored file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// On meshes: what litegen's own reader found in this very file. Never the
    /// vendor's claims.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneManifest>,
}

/// What litegen measured in a delivered mesh (3D doc §3D). Every delivered
/// mesh is in glTF's conventions: metres, +Y up, the subject facing +Z with
/// its right side at −X.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SceneManifest {
    /// Always `m`.
    pub units: String,
    /// Always `+y`.
    pub up: String,
    /// `+z`, or `unverified` when the model's orientation has not been proven
    /// (litegen never reports a guess).
    pub front: String,
    /// `vendor` (it arrived so) or `litegen` (litegen turned it).
    pub oriented_by: String,
    /// World-space bounds at rest, in metres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<SceneBounds>,
    pub vertex_count: u64,
    pub triangle_count: u64,
    pub uv_sets: u32,
    pub vertex_colors: bool,
    pub materials: Vec<SceneMaterial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin: Option<SceneSkin>,
    pub animations: Vec<SceneAnimation>,
    /// glTF extensions the file uses (none is ever required).
    pub extensions_used: Vec<String>,
    /// A `segment` model's parts: one glTF node each, named with the
    /// vendor's label or `part_<n>`. Absent on every other model's mesh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<ScenePart>>,
    /// What delivery could not keep or do for this file: a texture litegen
    /// could not decode, a joint naming the rig could not take, what this
    /// container cannot hold of the scene. Empty when nothing was lost.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// One part of a segmented mesh, measured by litegen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScenePart {
    pub name: String,
    /// The part's node: its index in the delivered glTF's `nodes`.
    pub node: u32,
    pub triangle_count: u64,
    /// World-space bounds at rest, in metres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<SceneBounds>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SceneBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SceneMaterial {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `base_color`, `metallic_roughness`, `normal`, `occlusion`, `emissive`.
    pub maps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SceneSkin {
    /// `humanoid` when a known naming scheme matched, else what the request
    /// asked the rig to be, else absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skeleton: Option<String>,
    pub joint_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// `t`, `a` or `unknown`, from the upper-arm angle at rest.
    pub bind_pose: String,
    /// Joint names, unique, in skin order.
    pub joints: Vec<String>,
    /// VRM 1.0 humanoid bone → joint name, for a rig litegen recognises;
    /// `null` for one it does not (never a guess).
    pub humanoid: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SceneAnimation {
    pub name: String,
    /// Measured from the keys.
    pub duration_s: f32,
    /// Measured: the root ends within 1 cm of where it started, horizontally.
    pub in_place: bool,
    /// `vendor` (arrived in place), `litegen` (litegen removed the root's
    /// horizontal travel), or absent when not in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_place_by: Option<String>,
    /// The root's world displacement, first key to last, in metres.
    pub root_travel_m: [f32; 3],
    /// Whether the request asked for a loop (a hint; glTF has no loop flag).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_requested: Option<bool>,
}

/// One stored file of a generation of any kind, as
/// `GET /v1/generations/{id}/assets` lists it. litegen keeps every result for
/// at least 7 days; `GET /v1/generations/{id}/assets/{index}` streams it with
/// a litegen key alone, whatever the vendor's own URL did.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GenerationAsset {
    /// Position in the list; the `{index}` that streams this file.
    pub index: u32,
    /// `image`, `video`, `mesh`, `texture`, `preview` or `original`. A plain
    /// string so a client tolerates a kind added later.
    pub kind: String,
    /// Lowercase container or image format, without a dot.
    pub format: String,
    /// Where the file is served from: absolute, and litegen's own storage.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Lowercase hex SHA-256 of the stored file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Videos: length in seconds, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f64>,
    /// Videos: frames per second, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    /// Whether a video carries an audio track (PixelGenie L10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_audio: Option<bool>,
}

/// `GET /v1/generations/{id}/assets`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GenerationAssetList {
    pub generation_id: String,
    pub data: Vec<GenerationAsset>,
}

/// A 3D generation's state — both the submit response and every poll
/// response, since 3D generation is always async. Mirrors
/// `VideoGenerationResponse`, with `video_url` replaced by `assets`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Model3dGenerationResponse {
    /// `litegen-3d-…`. Poll it at `GET /v1/models3d/{id}`; it is also the
    /// generation's id under `/v1/generations`.
    pub id: String,
    pub status: GenerationStatus,
    pub model: String,
    pub provider: String,
    /// Populated on `completed`; omitted while in flight and on `failed`, so a
    /// failed generation never looks half-usable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<Model3dAsset>,
    /// 0–100. Vendors that only report queue state (fal) move in coarse steps.
    pub progress: u8,
    /// Why the generation failed, when it did. Vendor failures carry the
    /// vendor's message, with its machine-readable type in parentheses when it
    /// sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The cost charged: quoted at submit, and 0 once a failed or cancelled
    /// generation has been refunded (L18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
    /// Unix seconds when the generation was submitted.
    pub created: i64,
    /// The request's `metadata`, as sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<std::collections::HashMap<String, String>>)]
    pub metadata: Option<RequestMetadata>,
}

impl Model3dGenerationResponse {
    /// The canonical mesh asset, if present.
    ///
    /// GLB wins whenever it is there. A generation may now carry one mesh per
    /// requested container, and this backs the `result_url` column — without
    /// the preference, whichever file the adapter happened to push first would
    /// become the canonical URL, which for a Meshy-shaped bundle is arbitrary.
    /// GLB is the one container every vendor emits and the only one the
    /// dashboard viewer can render.
    pub fn mesh(&self) -> Option<&Model3dAsset> {
        let meshes = || self.assets.iter().filter(|a| a.kind == Model3dAssetKind::Mesh);
        meshes()
            .find(|a| a.format.eq_ignore_ascii_case(DEFAULT_MODEL3D_FORMAT))
            .or_else(|| meshes().next())
    }

    /// Distinct mesh containers present, lowercased — the delivered half of the
    /// requested-vs-delivered guard.
    pub fn delivered_formats(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for a in self.assets.iter().filter(|a| a.kind == Model3dAssetKind::Mesh) {
            let f = a.format.to_ascii_lowercase();
            if !out.contains(&f) {
                out.push(f);
            }
        }
        out
    }
}

/// Formats that were asked for but never arrived.
///
/// Comparison is case-insensitive because `format` is a free-form string filled
/// in by each adapter, and a vendor answering `"GLB"` is a spelling difference,
/// not a missing file.
pub fn missing_model3d_formats(requested: &[String], delivered: &[String]) -> Vec<String> {
    requested
        .iter()
        .filter(|r| !delivered.iter().any(|d| d.eq_ignore_ascii_case(r)))
        .cloned()
        .collect()
}

// ─── Shared Types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GenerationStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    Cancelled,
}

impl GenerationStatus {
    /// Whether this status is final: the generation will never change again.
    ///
    /// Callers use this to decide whether polling the provider is still
    /// meaningful. It must stay exhaustive-matched (no `_` arm) so a new status
    /// forces an explicit decision here rather than silently counting as
    /// in-flight.
    pub fn is_terminal(&self) -> bool {
        match self {
            Self::Completed | Self::Failed | Self::Cancelled => true,
            Self::Pending | Self::Processing => false,
        }
    }
}

impl std::fmt::Display for GenerationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Processing => write!(f, "processing"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Cost / usage information for a generation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UsageInfo {
    /// Provider cost in USD.
    pub cost_usd: f64,
    /// Internal token cost.
    pub tokens: u64,
    /// Cost source: "dynamic" (from API) or "estimated" (from pricing table).
    pub cost_source: CostSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    Dynamic,
    Estimated,
}

// ─── Provider / Model Types ─────────────────────────────────────────────────

/// A model available through LiteGen.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelInfo {
    /// Unique model ID (e.g. "openai/dall-e-3").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Provider name.
    pub provider: String,
    /// Media type this model produces.
    pub media_type: MediaType,
    /// Whether the model's provider can serve this caller (L14).
    pub is_available: bool,
    /// Where the model is in its vendor's lifecycle (L14).
    pub status: crate::capabilities::ModelStatus,
    /// When the vendor deprecated it (`YYYY-MM-DD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated_at: Option<String>,
    /// The vendor's shutdown date (`YYYY-MM-DD`): litegen stops serving it the
    /// day after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retires_at: Option<String>,
    /// The catalog model to move to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
    /// Model capabilities.
    pub capabilities: ModelCapabilities,
    /// Pricing information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing: Option<ModelPricing>,
    /// Tags for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Image,
    Video,
    Model3d,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelCapabilities {
    /// What the model does: `generate`, or an operation on an existing asset
    /// such as `rig` or `animate`. The same value `GET /v1/models/{id}`
    /// serves as `task`.
    #[serde(default)]
    pub task: crate::capabilities::ModelTask,
    pub supports_text_to_image: bool,
    pub supports_image_to_image: bool,
    #[serde(default)]
    pub supports_inpainting: bool,
    #[serde(default)]
    pub supported_sizes: Vec<String>,
    /// The most reference images one request may carry. Kept under its old
    /// name too: `max_images` is this same number (L14).
    #[serde(default = "default_max_images")]
    pub max_reference_images: u32,
    #[serde(default = "default_max_images")]
    pub max_images: u32,
    // Video-specific
    #[serde(default)]
    pub supports_text_to_video: bool,
    #[serde(default)]
    pub supports_image_to_video: bool,
    #[serde(default)]
    pub supports_first_frame: bool,
    #[serde(default)]
    pub supports_last_frame: bool,
    /// The longest clip the model makes, from its `duration_seconds` param.
    #[serde(default)]
    pub max_duration_seconds: Option<f64>,
    // 3D-specific
    /// Accepts a prompt with no reference images (text-to-3D).
    #[serde(default)]
    pub supports_text_to_3d: bool,
    /// Accepts one `init` reference image (image-to-3D).
    #[serde(default)]
    pub supports_image_to_3d: bool,
    /// Accepts several `view-*` reference images (multiview-to-3D).
    #[serde(default)]
    pub supports_multiview_to_3d: bool,
    /// Takes the `pbr` param.
    #[serde(default)]
    pub supports_pbr: bool,
    /// Takes the `rig` param.
    #[serde(default)]
    pub supports_rig: bool,
    /// Takes `animations`: one clip per entry, whatever the task (3D doc
    /// §3C) — a rig or generate model that returns clips in the same call
    /// as well as an `animate` one.
    #[serde(default)]
    pub supports_animation: bool,
    /// Takes `skeleton`; `params.skeleton.enum_values` lists what it rigs
    /// (3D doc §3B).
    #[serde(default)]
    pub supports_skeleton: bool,
    /// Takes `joint_naming` (3D doc §3B).
    #[serde(default)]
    pub supports_joint_naming: bool,
    /// Takes the `texture` param.
    #[serde(default)]
    pub supports_texture: bool,
    /// Takes `source_model`: an operation on an existing mesh.
    #[serde(default)]
    pub supports_source_model: bool,
    /// Every mesh container a caller may request, including those litegen
    /// derives locally — the same list, in the same order, as
    /// `GET /v1/models/{id}` serves under `params.output_formats.enum_values`.
    /// Empty for image and video models.
    #[serde(default)]
    pub output_formats: Vec<String>,
    /// Upper bound of the model's `target_polycount` param, when it takes one.
    #[serde(default)]
    pub max_polycount: Option<u32>,
    /// Takes `keyframe` references, each pinned at its `time_s` (3D doc §5).
    #[serde(default)]
    pub supports_keyframes: bool,
    /// The most `keyframe` references one request may carry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_keyframes: Option<u32>,
    /// `any`: each keyframe at its own time; `even`: evenly spaced over the
    /// clip, start and end included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyframe_timing: Option<crate::capabilities::KeyframeTiming>,
    /// Takes `region` (3D doc §4).
    #[serde(default)]
    pub supports_region_edit: bool,
    /// How the model receives a `region`: `box` (its own box) or `mask`
    /// (drawn as its mask).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region_input: Option<crate::capabilities::RegionInput>,
    /// What the vendor promises about pixels outside the edit, in its own
    /// words: `exact`, `similar` or `unspecified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserves_unmasked: Option<crate::capabilities::PreservesUnmasked>,
    /// The mask convention a caller sends: `white_edits` on every model that
    /// takes a mask, whatever its vendor reads — litegen converts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_convention: Option<crate::capabilities::MaskConvention>,
    /// Takes `camera: fixed` (PixelGenie L10).
    #[serde(default)]
    pub supports_fixed_camera: bool,
    /// Takes `audio: false` (or `true`) to leave audio out (or in).
    #[serde(default)]
    pub supports_audio_toggle: bool,
    /// Takes `loop: true`.
    #[serde(default)]
    pub supports_loop: bool,
    /// The only `duration_seconds` values the model takes; empty when it
    /// takes its range.
    #[serde(default)]
    pub duration_values: Vec<f64>,
    /// Takes `background: transparent` (PixelGenie L9).
    #[serde(default)]
    pub supports_transparent_background: bool,
    /// Returns only lossy containers (no PNG).
    #[serde(default)]
    pub lossy_only: bool,
    /// Takes one `init` image and returns it as an RGBA PNG with its
    /// background removed (PixelGenie L8).
    #[serde(default)]
    pub supports_background_removal: bool,
    /// The map sizes a `texture` model paints at (`texture_resolution`);
    /// empty when it takes none.
    #[serde(default)]
    pub texture_resolutions: Vec<u32>,
    /// Takes `keep_uvs`.
    #[serde(default)]
    pub supports_keep_uvs: bool,
    /// Takes `ground`, `center` and `height_m`, which litegen applies to every
    /// mesh it delivers: true on every 3D model.
    #[serde(default)]
    pub supports_placement: bool,
    /// Takes `expand` (3D doc §4).
    #[serde(default)]
    pub supports_expand: bool,
    /// Takes `layout` (3D doc §4).
    #[serde(default)]
    pub supports_layout: bool,
    /// What its layout boxes may hold; empty when it takes no layout.
    #[serde(default)]
    pub layout_kinds: Vec<crate::capabilities::LayoutKind>,
    /// Takes `mask_prompt` (3D doc §4).
    #[serde(default)]
    pub supports_mask_prompt: bool,
    /// Takes `preserve_unmasked`, which litegen applies to any model taking a
    /// mask or a region (3D doc §4).
    #[serde(default)]
    pub supports_preserve_unmasked: bool,
    /// Takes `mask_grow_px`: litegen widens the mask or region of any model
    /// taking one, or asks a vendor that grows its own (3D doc §4).
    #[serde(default)]
    pub supports_mask_grow: bool,
    /// Works on a `source_video` (3D doc §5).
    #[serde(default)]
    pub supports_source_video: bool,
    /// Draws native pixel art (PixelGenie L16).
    #[serde(default)]
    pub native_pixel_art: bool,
    /// A native pixel-art model's output edge, in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_size: Option<PixelSize>,
    /// Takes `palette`, as a hint or forced.
    #[serde(default)]
    pub supports_palette: bool,
    /// Every pixel of its output is one of the palette's colours.
    #[serde(default)]
    pub supports_forced_palette: bool,
    /// The `directions` counts it takes; empty when none.
    #[serde(default)]
    pub directions: Vec<u32>,
    /// An `extend` model's answer: the whole clip or only the added part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extend_output: Option<crate::capabilities::ExtendOutput>,
    /// Takes `time_range`.
    #[serde(default)]
    pub supports_time_range: bool,
    /// Serves `POST /v1/images/segmentations` (PixelGenie L7).
    #[serde(default)]
    pub supports_segmentation: bool,
    /// The prompt types it segments by: `point`, `box`, `text`. Empty when it
    /// does not segment.
    #[serde(default)]
    pub segmentation_prompts: Vec<crate::capabilities::SegmentationPromptType>,
}

fn default_max_images() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelPricing {
    /// Base cost per image/video in USD.
    pub base_cost_usd: f64,
    /// How the price follows the request: ordered rules over a running
    /// price that starts at `base_cost_usd` (see `PriceRule`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variable_pricing: Vec<crate::capabilities::PriceRule>,
}

// ─── Cost Estimation ────────────────────────────────────────────────────────

/// Cost estimate returned before generation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CostEstimate {
    /// Base cost from the provider in USD.
    pub base_cost_usd: f64,
    /// Markup applied (configurable, default 0%).
    pub markup_usd: f64,
    /// Total cost including markup.
    pub total_cost_usd: f64,
    /// Equivalent token cost.
    pub tokens_required: u64,
    /// Where the cost data came from.
    pub cost_source: CostSource,
    /// Breakdown details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakdown: Option<serde_json::Value>,
}

// ─── Routing / Config Types ─────────────────────────────────────────────────

/// Configuration for a provider deployment.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderConfig {
    /// Provider name (e.g. "openai", "stability", "replicate").
    pub provider: String,
    /// API key(s) with optional weights.
    pub api_keys: Vec<ApiKeyEntry>,
    /// Base URL override.
    #[serde(default)]
    pub api_base: Option<String>,
    /// Model ID mapping: internal ID → provider's model ID.
    #[serde(default)]
    pub model_mapping: std::collections::HashMap<String, String>,
    /// Extra headers.
    #[serde(default)]
    pub extra_headers: std::collections::HashMap<String, String>,
    /// Provider-specific options.
    #[serde(default)]
    pub options: Option<serde_json::Value>,
    /// Whether this provider config is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// An API key with weight for round-robin distribution.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyEntry {
    /// The API key value. Stored encrypted at rest.
    pub key: String,
    /// Weight for round-robin (higher = more traffic). Default: 1.
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// Optional label for the key.
    #[serde(default)]
    pub label: Option<String>,
}

fn default_weight() -> u32 {
    1
}

/// A signing credential set (key_id + key_secret, plus optional region for
/// SigV4/TC3) with a weight for round-robin distribution. The signing-scheme
/// analogue of [`ApiKeyEntry`], used by providers like Bedrock, Hunyuan, and
/// Kling that authenticate with a credential *pair* rather than a bearer key.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CredentialEntry {
    /// Access key id (SigV4) / secret id (TC3) / access key (Kling).
    pub key_id: String,
    /// Secret access key (SigV4) / secret key (TC3, Kling). Stored encrypted.
    pub key_secret: String,
    /// Region (SigV4 / TC3). Omitted for Kling; when absent the provider falls
    /// back to its configured/default region.
    #[serde(default)]
    pub region: Option<String>,
    /// Weight for round-robin (higher = more traffic). Default: 1.
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// Optional label for the credential set.
    #[serde(default)]
    pub label: Option<String>,
}

/// One field the dashboard must collect for a provider credential, per pool
/// entry. Returned as part of [`ProviderCatalogEntry`].
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CredentialFieldSpec {
    /// JSON key within each pool entry (e.g. "key", "key_id", "key_secret", "region").
    pub key: String,
    /// Human-readable label for the input.
    pub label: String,
    /// Whether the value is secret and should be rendered masked.
    pub secret: bool,
    /// Whether the field may be left empty.
    #[serde(default)]
    pub optional: bool,
}

/// Describes a provider and how the dashboard should render its credential
/// form. Returned by `GET /v1/providers` so the UI never hard-codes per-provider
/// field knowledge.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderCatalogEntry {
    /// Provider id (e.g. "openai", "bedrock").
    pub name: String,
    /// Which media this provider serves — any of "image", "video".
    pub modalities: Vec<String>,
    /// Coarse auth scheme: "api_key" | "aws_sigv4" | "tencent_tc3" | "kling_jwt".
    pub auth_scheme: String,
    /// Which credentials array to submit: "api_keys" (bearer) or
    /// "credential_sets" (signing). Each array element is `{<fields…>, weight?, label?}`.
    pub pool_field: String,
    /// Fields to collect per pool entry, besides the universal `weight`/`label`.
    pub fields: Vec<CredentialFieldSpec>,
}

/// Routing configuration for a model, with fallbacks and weights.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelRoute {
    /// The model ID pattern (e.g. "dall-e-3", "openai/*").
    pub model: String,
    /// Ordered list of provider deployments to try.
    pub deployments: Vec<Deployment>,
    /// Routing strategy.
    #[serde(default)]
    pub strategy: RoutingStrategy,
    /// Cache settings for this model.
    #[serde(default)]
    pub cache: Option<CacheConfig>,
}

/// A single deployment in a routing chain.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Deployment {
    /// Provider config name/id.
    pub provider: String,
    /// Weight for weighted routing.
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// Max retries before falling to next deployment.
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    /// Timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    /// Rate limit (requests per minute). 0 = unlimited.
    #[serde(default)]
    pub rpm_limit: u32,
    /// Whether to skip this deployment on health check failure.
    #[serde(default = "default_true")]
    pub respect_health: bool,
}

fn default_retries() -> u32 {
    2
}
fn default_timeout() -> u64 {
    120
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    /// Try deployments in order, fall back on failure.
    #[default]
    Fallback,
    /// Distribute by weight.
    WeightedRoundRobin,
    /// Pick the cheapest provider.
    LowestCost,
    /// Pick the fastest provider (by latency history).
    LowestLatency,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CacheConfig {
    /// Enable caching for this model.
    #[serde(default)]
    pub enabled: bool,
    /// Cache TTL in seconds.
    #[serde(default = "default_cache_ttl")]
    pub ttl_seconds: u64,
    /// Max cached items.
    #[serde(default = "default_cache_max")]
    pub max_items: u64,
}

fn default_cache_ttl() -> u64 {
    3600
}
fn default_cache_max() -> u64 {
    1000
}

// ─── Health & Stats ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderHealth {
    pub provider: String,
    pub healthy: bool,
    pub message: Option<String>,
    pub latency_ms: Option<u64>,
    pub last_checked: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RequestLog {
    pub id: String,
    pub model: String,
    pub provider: String,
    pub status: GenerationStatus,
    pub media_type: MediaType,
    pub cost_usd: f64,
    pub latency_ms: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Latency percentiles for the last N minutes.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LatencyPercentiles {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub sample_count: u64,
    pub window_minutes: i64,
}

/// Aggregate stats for the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProxyStats {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
    pub requests_per_minute: f64,
    pub models_used: Vec<ModelUsageStat>,
    pub providers_used: Vec<ProviderUsageStat>,
    pub latency_percentiles: LatencyPercentiles,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelUsageStat {
    pub model: String,
    pub requests: u64,
    pub cost_usd: f64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderUsageStat {
    pub provider: String,
    pub requests: u64,
    pub failures: u64,
    pub cost_usd: f64,
    pub avg_latency_ms: f64,
}

// ─── Log Filters ─────────────────────────────────────────────────────────────

/// Optional filters for `get_request_logs_filtered`.
#[derive(Debug, Clone, Default)]
pub struct LogFilters {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub status: Option<String>,
    /// ISO 8601 lower bound (inclusive).
    pub from: Option<String>,
    /// ISO 8601 upper bound (inclusive).
    pub to: Option<String>,
}

// ─── Pagination ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PaginatedResponse<T: Serialize> {
    pub data: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
    pub total_pages: u32,
}

// ─── Error Response ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorDetail {
    /// Human-readable; not for matching on — use `code` and `param`.
    pub message: String,
    /// The error's family, e.g. `validation_error`, `authentication_error`,
    /// `quota_exceeded`, `provider_not_configured`.
    pub r#type: String,
    /// Machine-readable reason within the family, e.g. `param_unsupported`,
    /// `param_enum_mismatch`, `prompt_required`, `ref_role_required`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The vendor's own answer, on `vendor_rejected` and `vendor_error`
    /// (PixelGenie L15).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_error: Option<VendorError>,
    /// Whether the same request may work later: a vendor's 5xx, 429 or
    /// timeout, but not its refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    /// The request field that failed validation — `output_formats`, `prompt`,
    /// `reference_images`, `extra.<key>` — so a caller can tell which one to fix
    /// without parsing `message`. Validation errors only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    /// The model the rejected request named. Validation errors only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The vendor's own job id, on `vendor_timeout` (the job the vendor had
    /// accepted when litegen stopped waiting, if it had; not re-sent) and on
    /// `generation_record_failed` (a job the vendor took that litegen could
    /// not record).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_job_id: Option<String>,
}

/// What a vendor said when it refused or failed a request (PixelGenie L15):
/// its HTTP status (none when it never answered), its own error code and its
/// message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct VendorError {
    pub status: Option<u16>,
    pub code: Option<String>,
    pub message: String,
}

// ─── Generations (DB-backed polling) ───────────────────────────────────────

/// `PATCH /v1/generations/{id}`'s answer: the cancelled generation as
/// settled, and whether its vendor took the cancel (L18).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CancelledGeneration {
    #[serde(flatten)]
    pub generation: Generation,
    /// True when the vendor accepted the cancel, so a queued job will not run;
    /// false when it has no cancel, had already finished, or did not answer
    /// within 5 seconds. The job is `cancelled` and refunded either way.
    pub vendor_cancelled: bool,
}

/// A DB-backed generation row (video or image, currently used for video).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Generation {
    /// Locally-minted ID (e.g. "litegen-vid-<uuid>").
    pub id: String,
    /// API key that submitted this generation; None when master key was used.
    pub key_id: Option<uuid::Uuid>,
    pub model: String,
    pub provider: String,
    pub media_type: String,
    pub status: GenerationStatus,
    pub progress: i32,
    /// Provider-assigned job ID used for polling.
    pub provider_job_id: Option<String>,
    /// Final result URL when completed.
    pub result_url: Option<String>,
    pub error_message: Option<String>,
    pub cost_usd: f64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Arbitrary JSON metadata.
    pub metadata: Option<serde_json::Value>,
    /// Owning tenant (organization). None for pre-tenancy / untenanted rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// Owning application. None for pre-tenancy / untenanted rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
}

// ─── Tenancy (Organizations / Applications / Members / Credentials) ─────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Organization {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub plan: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Application {
    pub id: String,
    pub org_id: String,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OrganizationMember {
    pub org_id: String,
    pub user_id: String,
    pub email: String, // joined from users for list views
    pub role: Role,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Public view of a stored BYO provider credential — NEVER the plaintext secret.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderCredentialInfo {
    pub provider: String,
    pub display_hint: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Internal row for per-app BYO storage config (DB → resolution/handlers).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AppStorageRow {
    pub backend: String,
    pub bucket_name: String,
    pub region: String,
    pub endpoint_url: Option<String>,
    pub custom_public_url: Option<String>,
    pub path_prefix: Option<String>,
    pub access_key_id_hint: Option<String>,
    pub secret_ciphertext: String,
    pub secret_nonce: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Owned input for upserting per-app BYO storage config.
#[derive(Debug, Clone)]
pub struct AppStorageUpsert {
    pub app_id: String,
    pub backend: String,
    pub bucket_name: String,
    pub region: String,
    pub endpoint_url: Option<String>,
    pub custom_public_url: Option<String>,
    pub path_prefix: Option<String>,
    pub access_key_id_hint: Option<String>,
    pub secret_ciphertext: String,
    pub secret_nonce: String,
}

/// Public view of per-app BYO storage config — NEVER includes the secret.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AppStorageInfo {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ─── Model Allowlists ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OrgAllowedModels {
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AppModelAccess {
    /// "all" = inherit everything in the org pool. "select" = only `models`.
    pub mode: String,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetOrgAllowedModelsRequest {
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetAppModelAccessRequest {
    /// Must be "all" or "select".
    pub mode: String,
    #[serde(default)]
    pub models: Vec<String>,
}

// ─── API Key Auth ───────────────────────────────────────────────────────────

/// API key for authenticating with the LiteGen proxy.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKey {
    pub id: Uuid,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
    /// USD budget cap; None = unlimited.
    pub token_quota: Option<f64>,
    /// Running USD spent under this key.
    pub tokens_used: f64,
    /// Requests-per-minute cap; None = unlimited.
    pub rpm_limit: Option<u32>,
    /// CSV of scopes: "generate,read,admin".
    pub scopes: String,
    /// Webhook URL for async notifications (future use).
    pub webhook_url: Option<String>,
    /// The user who owns this key (None for master-key-created keys).
    pub owner_user_id: Option<String>,
    pub org_id: Option<String>,
    pub app_id: Option<String>,
    /// Public key id shown to customers, e.g. "pk_live_…". None for legacy lg- keys.
    pub public_id: Option<String>,
}

/// Full key detail for GET /v1/keys/{id} and PATCH /v1/keys/{id} responses.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyDetail {
    pub id: Uuid,
    pub name: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
    pub token_quota: Option<f64>,
    pub tokens_used: f64,
    pub rpm_limit: Option<u32>,
    pub scopes: String,
    pub webhook_url: Option<String>,
}

/// Request body for PATCH /v1/keys/{id}.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateApiKeyRequest {
    pub name: Option<String>,
    pub token_quota: Option<f64>,
    pub rpm_limit: Option<u32>,
    /// Replace the key's scopes: the same values and the same privilege check
    /// as on create (an unknown scope is a 400, one the caller may not grant a
    /// 403).
    pub scopes: Option<String>,
    pub webhook_url: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: Option<bool>,
}

/// Response for `POST /v1/keys/{id}/rotate`. Carries the freshly-minted secret,
/// which is returned exactly once (it is never persisted or shown again).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RotatedKeyResponse {
    pub id: Uuid,
    /// Public key id, e.g. "pk_live_…".
    pub public_id: String,
    /// The new secret, e.g. "sk_live_…". Shown only at rotation time.
    pub key: String,
    pub prefix: String,
    pub name: String,
    /// CSV of scopes carried over from the rotated key.
    pub scopes: String,
    pub token_quota: Option<f64>,
    pub rpm_limit: Option<u32>,
    pub webhook_url: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ─── Typed API Response Wrappers ────────────────────────────────────────────

/// Response for `GET /health/live`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LivenessResponse {
    pub status: String,
}

/// Response for `GET /health/ready`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReadinessResponse {
    /// "ready" or "not_ready"
    pub status: String,
    pub checks: ReadinessChecks,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReadinessChecks {
    pub db: bool,
    pub providers: Vec<String>,
}

/// Cache state included in the health response.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CacheStatus {
    pub enabled: bool,
    pub entries: u64,
}

/// Response for `GET /health`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub providers: Vec<ProviderHealth>,
    pub cache: CacheStatus,
}

/// Response for `GET /v1/models`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelListResponse {
    pub object: String,
    pub data: Vec<ModelInfo>,
    /// Changes whenever any served model's field does (L14).
    pub catalog_version: String,
}

/// Public view of an API key (no hash).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyInfo {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
    pub token_quota: Option<f64>,
    pub tokens_used: f64,
    pub rpm_limit: Option<u32>,
    pub scopes: String,
    pub webhook_url: Option<String>,
    pub public_id: Option<String>,
    pub app_id: Option<String>,
}

/// Response for `GET /v1/keys`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyListResponse {
    pub data: Vec<ApiKeyInfo>,
}

/// Response for `POST /v1/keys`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyCreatedResponse {
    pub id: Uuid,
    pub key: String,
    pub prefix: String,
    pub public_id: String,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub token_quota: Option<f64>,
    pub rpm_limit: Option<u32>,
    pub scopes: String,
}

/// Response for `DELETE /v1/keys/{id}` and similar revocation endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevokeKeyResponse {
    pub revoked: bool,
}

/// Response for `DELETE /v1/cache`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CacheClearedResponse {
    pub cleared: bool,
}

// ─── Audit Log ───────────────────────────────────────────────────────────────

/// One audit log entry recording an admin action.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AuditLogEntry {
    pub id: String,
    /// The API key ID of the actor. None when the master key was used.
    pub actor_key_id: Option<String>,
    /// Human-readable actor label: `"master-key"` or the key's name.
    pub actor_label: String,
    /// Action identifier, e.g. `"key.create"`, `"generation.cancel"`.
    pub action: String,
    /// Target entity type: `"api_key"` or `"generation"`.
    pub target_type: String,
    /// Target entity ID.
    pub target_id: String,
    /// JSON snapshot of the entity state *before* the action (None on create).
    pub before_json: Option<String>,
    /// JSON snapshot of the entity state *after* the action (None on delete/revoke).
    pub after_json: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Tenant (organization) this audit entry belongs to. None falls back to the
    /// default org at the DB layer (single-tenant); set from the caller's active org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
}

/// Filters for `list_audit_log`.
#[derive(Debug, Clone, Default)]
pub struct AuditLogFilter {
    pub actor_key_id: Option<String>,
    pub action: Option<String>,
    /// ISO 8601 lower bound (inclusive).
    pub from: Option<String>,
    /// ISO 8601 upper bound (inclusive).
    pub to: Option<String>,
}

// ─── Webhook Delivery Log ────────────────────────────────────────────────────

// ─── Request Artifacts ──────────────────────────────────────────────────────

/// Stores the input/output snapshot of a generation request for drill-down.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RequestArtifact {
    pub request_id: String,
    pub media_type: String,
    pub prompt: Option<String>,
    pub negative_prompt: Option<String>,
    pub params_json: Option<serde_json::Value>,
    pub refs_meta_json: Option<serde_json::Value>,
    /// "b64" | "url" | "error"
    pub output_kind: String,
    pub output_value: Option<String>,
    pub output_mime: Option<String>,
    pub output_truncated: bool,
    pub error_message: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub org_id: Option<String>,
    pub app_id: Option<String>,
}

/// One row in the `webhook_deliveries` table.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WebhookDelivery {
    pub id: String,
    pub key_id: String,
    pub generation_id: String,
    pub url: String,
    pub attempt_number: i32,
    pub status_code: Option<i32>,
    /// Whether this attempt was successful (HTTP 2xx).
    pub success: bool,
    pub response_body: Option<String>,
    pub error_message: Option<String>,
    pub payload_json: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ─── User / Session / Invitation / PasswordReset ────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct User {
    pub id: String,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    pub role: Role,
    pub oauth_github_id: Option<String>,
    pub oauth_google_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_login_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub csrf_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Invitation {
    pub id: String,
    pub email: String,
    /// The role the invited user will receive in `org_id` upon accepting.
    pub role: Role,
    pub token: String,
    pub invited_by: Option<String>,
    /// Organization the invitee will join on accept. Defaults to the
    /// single-tenant default org for legacy/global invites.
    pub org_id: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PasswordReset {
    pub token: String,
    pub user_id: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_and_app_serde_roundtrip() {
        let org = Organization { id: "o1".into(), name: "Acme".into(), slug: "acme".into(),
            plan: "free".into(), status: "active".into(),
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now() };
        let j = serde_json::to_string(&org).unwrap();
        let back: Organization = serde_json::from_str(&j).unwrap();
        assert_eq!(back.slug, "acme");
    }
}

#[cfg(test)]
mod ref_image_tests {
    use super::*;

    #[test]
    fn deserialize_url_ref() {
        let j = r#"{"type":"url","value":"https://example.com/x.png"}"#;
        let r: ReferenceImage = serde_json::from_str(j).unwrap();
        assert!(matches!(r.kind, RefImageKind::Url));
        assert_eq!(r.value, "https://example.com/x.png");
        assert!(r.role.is_none());
    }

    #[test]
    fn deserialize_base64_ref_with_role() {
        let j = r#"{"type":"base64","value":"abc==","role":"mask"}"#;
        let r: ReferenceImage = serde_json::from_str(j).unwrap();
        assert!(matches!(r.kind, RefImageKind::Base64));
        assert_eq!(r.role.as_deref(), Some("mask"));
    }

    #[test]
    fn deserialize_blob_ref() {
        let j = r#"{"type":"blob","value":"field_init","role":"init"}"#;
        let r: ReferenceImage = serde_json::from_str(j).unwrap();
        assert!(matches!(r.kind, RefImageKind::Blob));
        assert_eq!(r.value, "field_init");
    }

    #[test]
    fn flatten_image_request() {
        let j = r#"{
          "prompt":"hi","model":"x/y","reference_images":[
            {"type":"url","value":"u"}
          ],
          "size":"1024x1024","strict":false
        }"#;
        let r: ImageGenerationRequest = serde_json::from_str(j).unwrap();
        assert_eq!(r.base.prompt, "hi");
        assert_eq!(r.base.reference_images.len(), 1);
        assert!(!r.base.strict);
        assert_eq!(r.size.as_deref(), Some("1024x1024"));
    }
}

#[cfg(test)]
mod model3d_vocab_tests {
    use super::*;

    #[test]
    fn every_format_round_trips_through_its_wire_spelling() {
        for f in Model3dFormat::ALL {
            let v = serde_json::to_value(f).unwrap();
            assert_eq!(v, f.as_str(), "serde and as_str disagree for {f:?}");
            assert_eq!(Model3dFormat::parse(f.as_str()), Some(f));
            assert_eq!(serde_json::from_value::<Model3dFormat>(v).unwrap(), f);
        }
        assert_eq!(Model3dFormat::ThreeMf.as_str(), "3mf", "the one spelling serde cannot derive");
    }

    #[test]
    fn format_parsing_folds_case_and_whitespace_like_the_validator() {
        assert_eq!(Model3dFormat::parse(" GLB "), Some(Model3dFormat::Glb));
        assert_eq!(Model3dFormat::parse("3MF"), Some(Model3dFormat::ThreeMf));
        assert_eq!(Model3dFormat::parse("GLTF"), Some(Model3dFormat::Gltf));
        assert_eq!(Model3dFormat::parse("gltf2"), None);
        assert_eq!(Model3dFormat::parse(""), None);
    }

    #[test]
    fn every_locally_derivable_container_can_be_named_in_a_request() {
        for f in crate::mesh::MeshFormat::ALL {
            assert!(Model3dFormat::parse(f.as_str()).is_some(), "{f:?} is derivable but not in the vocabulary");
        }
        assert_eq!(Model3dFormat::parse(DEFAULT_MODEL3D_FORMAT), Some(Model3dFormat::Glb));
    }

    #[test]
    fn topology_and_symmetry_round_trip_through_their_wire_spellings() {
        for t in Model3dTopology::ALL {
            assert_eq!(serde_json::to_value(t).unwrap(), t.as_str());
        }
        for s in Model3dSymmetry::ALL {
            assert_eq!(serde_json::to_value(s).unwrap(), s.as_str());
        }
    }

    #[test]
    fn an_omitted_prompt_deserialises_as_empty_for_the_validator_to_judge() {
        // Whether a prompt is required is the model's call — `PromptSpec`,
        // applied by the validator with a precise `prompt_required` — never a
        // JSON shape error. An image-to-3D model with no prompt field takes none.
        let r: Model3dGenerationRequest =
            serde_json::from_str(r#"{"model":"fal/trellis","reference_images":[{"type":"url","value":"https://x/a.png"}]}"#)
                .unwrap();
        assert_eq!(r.base.prompt, "");
    }
}
