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

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PromptSpec {
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)] pub min_length: Option<usize>,
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ParamSpecStringArray {
    #[serde(default)] pub enum_values: Vec<String>,
    /// Upper bound on how many members one request may ask for. `None` means
    /// "as many as are declared".
    #[serde(default)] pub max_items: Option<usize>,
    /// Applied when the caller omits the param entirely. Empty means the
    /// resolver falls back to `glb`.
    #[serde(default)] pub default: Vec<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelSchema {
    pub id: String,
    pub provider: String,
    pub media_type: MediaType,
    pub display_name: String,
    #[serde(default)] pub description: String,
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
];

fn default_true() -> bool { true }
