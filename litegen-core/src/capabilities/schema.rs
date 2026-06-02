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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ModelCapabilityFlags {
    #[serde(default)] pub text_to_image: bool,
    #[serde(default)] pub image_to_image: bool,
    #[serde(default)] pub inpainting: bool,
    #[serde(default)] pub text_to_video: bool,
    #[serde(default)] pub image_to_video: bool,
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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ParamSpecInt {
    #[serde(default)] pub min: Option<i64>,
    #[serde(default)] pub max: Option<i64>,
    #[serde(default)] pub default: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ParamSpecFloat {
    #[serde(default)] pub min: Option<f64>,
    #[serde(default)] pub max: Option<f64>,
    #[serde(default)] pub default: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ParamSpecString {
    #[serde(default)] pub max_length: Option<usize>,
    #[serde(default)] pub enum_values: Vec<String>,
    #[serde(default)] pub pattern: Option<String>,
    #[serde(default)] pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ParamSpecAspectRatio {
    pub allowed: Vec<String>,
    #[serde(default)] pub default: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub struct ParamSpecSeed {
    pub min: i64,
    pub max: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamSpec {
    Bool(ParamSpecBool),
    Int(ParamSpecInt),
    Float(ParamSpecFloat),
    String(ParamSpecString),
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
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SizeSpecEnum {
    // JSON shape is `[[w, h], ...]`; represented in OpenAPI as `Vec<Vec<u32>>`
    // (OpenAPI 3.1's prefixItems is not portable across codegen tools yet).
    #[schema(value_type = Vec<Vec<u32>>)]
    pub values: Vec<(u32, u32)>,
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
];

fn default_true() -> bool { true }
