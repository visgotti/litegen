# Capability Registry & Per-Model Param Handling — Design

**Status:** approved
**Date:** 2026-05-28
**Scope:** litegen-core

---

## 1. Problem

Litegen normalizes image/video generation requests across many providers, but each provider — and each *model* within a provider — has a different parameter shape. Some accept `negative_prompt`, some don't. Some accept multiple reference images, some accept one, some accept none with specific roles (init / mask / first_frame / last_frame / style / character). Some have strict aspect-ratio enums; others take free width/height. Prompt-length limits vary.

Today, those differences are buried inside each provider's `generate()` function. The consequences:

- **Silent drops.** A user sends `guidance_scale` to DALL-E 3; it's ignored with no error or warning.
- **No discoverability.** `/v1/models` returns boolean flags but no parameter-level schema, so clients can't introspect what a model accepts.
- **Dead escape hatch.** `ImageGenerationRequest.extra: Value` exists ([`src/types/mod.rs:56`](../../../litegen-core/src/types/mod.rs#L56)) but no provider reads it.
- **Singular reference fields.** `image_url`, `mask_url`, `first_frame_url`, `last_frame_url` can't express multi-ref models (Midjourney `--cref`+`--sref`, Luma multi-keyframe, etc.).
- **Inconsistent size handling.** OpenAI uses fixed enums, Stability V2 aspect-ratio enums (via pixel-to-ratio mapping), Replicate raw W×H, Fal nested JSON, Google a different aspect-ratio set.

This design replaces the inline per-provider capability declarations with a **single typed registry** loaded from YAML, adds an **edge validator** that consults the registry before dispatching, and adds a **materializer** that converts a unified reference-image input into whatever each provider needs (uploading to temp storage when a URL is required, fetching when bytes are required, building multipart when expected).

---

## 2. Goals

1. Every model declares its full input contract: prompt limits, accepted params with bounds/enums, accepted ref-image roles + counts + provider format, `extra`-passthrough allowlist.
2. The contract is **data**, not hardcoded Rust — adding a model is a yaml change.
3. Requests are validated against the contract before reaching a provider. Strict by default with a per-request `strict: false` opt-out for power users.
4. Reference images use a uniform tagged-union shape; the proxy adapts to each provider's required form.
5. The schema is published over HTTP so clients (and the dashboard) can introspect.
6. Pre-release: hard break on the legacy singular fields. No deprecation shims.

## 3. Non-goals (deferred to later passes)

- Hot-reloading the yaml at runtime (load at startup; restart to update).
- Per-user / per-key quota or rate limiting.
- Streaming responses or webhooks.
- Per-model variable pricing beyond a single `base_cost_usd` field.
- Migrating to a unified `/v1/generations` endpoint (we keep `/v1/images/generations` and `/v1/videos/generations` for OpenAI-SDK compatibility).

---

## 4. Architecture

```
                          litegen.yaml (deployment config)
                                    │
                                    ▼
   models/*.yaml ─►  ┌──────────────────────────────┐  ◄── provider impls
   (per-provider     │       CapabilityRegistry     │      (consume at runtime;
    model schemas)   │       (typed, in-memory)     │       no more list_models)
                     └──────────────┬───────────────┘
                                    │ queries
                  ┌─────────────────┼──────────────────┐
                  ▼                 ▼                  ▼
         ┌──────────────┐  ┌────────────────┐  ┌─────────────────┐
         │  Validator   │  │  Materializer  │  │ /v1/models      │
         │ (middleware) │  │  (ref-image    │  │ /v1/models/{id} │
         └──────┬───────┘  │   adapter)     │  └─────────────────┘
                │          └────────┬───────┘
                ▼                   ▼
        request rejected     refs converted to
        with 400, OR         provider's required
        params dropped       form (URL/base64/
        (strict=false +      multipart)
        X-Litegen-Dropped-          │
        Params header)              ▼
                              Provider.generate()
```

**Module layout (new and changed):**

| Path | Purpose | Status |
|---|---|---|
| `src/capabilities/mod.rs` | Public re-exports | new |
| `src/capabilities/schema.rs` | `ModelSchema`, `ParamSpec`, `RefInputSpec`, etc. | new |
| `src/capabilities/registry.rs` | In-memory store; lookup API | new |
| `src/capabilities/loader.rs` | YAML parse + error surface | new |
| `models/<provider>.yaml` | Per-provider model declarations | new (data) |
| `src/api/middleware/validator.rs` | Axum layer that validates requests | new |
| `src/proxy/materializer.rs` | Reference-image adapter + temp resource tracking | new |
| `src/types/mod.rs` | New `ReferenceImage`; refactored request structs | changed |
| `src/providers/mod.rs` | `ImageProvider` / `VideoProvider` traits: drop `list_models`, `supported_models`; `generate()` takes `MaterializedRequest` | changed |
| Every `src/providers/{image,video}/*.rs` | Remove inline `list_models`; rewrite `generate()` against materialized request | changed |
| `src/api/handlers.rs` | New `GET /v1/models/{id}`; remove legacy request fields | changed |

---

## 5. Schema types (Rust)

`src/capabilities/schema.rs`:

```rust
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Full schema for one model. Loaded from yaml at startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSchema {
    pub id: String,                            // "stability/sd3-large"
    pub provider: String,                      // "stability"
    pub media_type: MediaType,
    pub display_name: String,
    pub description: String,
    pub pricing: ModelPricing,
    pub capabilities: ModelCapabilityFlags,
    pub prompt: PromptSpec,
    #[serde(default)]
    pub params: HashMap<String, ParamSpec>,    // keyed by canonical param name
    #[serde(default)]
    pub ref_inputs: Option<RefInputSpec>,
    #[serde(default)]
    pub extra_allowlist: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSpec {
    #[serde(default = "default_true")]
    pub required: bool,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType { Image, Video }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCapabilityFlags {
    #[serde(default)] pub text_to_image: bool,
    #[serde(default)] pub image_to_image: bool,
    #[serde(default)] pub inpainting: bool,
    #[serde(default)] pub text_to_video: bool,
    #[serde(default)] pub image_to_video: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub base_cost_usd: f64,
    #[serde(default)]
    pub variable_pricing: Option<serde_json::Value>,
}

/// A parameter accepted by the model. Validation derives directly from the variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamSpec {
    Bool { default: Option<bool> },

    Int {
        min: Option<i64>,
        max: Option<i64>,
        default: Option<i64>,
    },

    Float {
        min: Option<f64>,
        max: Option<f64>,
        default: Option<f64>,
    },

    String {
        max_length: Option<usize>,
        #[serde(default)]
        enum_values: Vec<String>,
        pattern: Option<String>,        // regex
        default: Option<String>,
    },

    Size(SizeSpec),                      // width × height

    AspectRatio {
        allowed: Vec<String>,           // e.g. ["1:1", "16:9"]
        default: Option<String>,
    },

    /// Seed is a special case so we can reject negative or oversize values
    /// without forcing every model to redeclare bounds.
    Seed { min: i64, max: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SizeSpec {
    /// Free width/height within bounds, optionally constrained to multiples.
    Freeform {
        min_width: u32, max_width: u32,
        min_height: u32, max_height: u32,
        #[serde(default)] multiple_of: Option<u32>,
    },
    /// Discrete set of W×H values (e.g. DALL-E 3's three sizes).
    Enum { values: Vec<(u32, u32)> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefInputSpec {
    /// Cap on total reference images across all roles.
    pub max_total: u32,
    /// Role assigned when client provides a ref without a `role` field.
    pub default_role: Option<String>,
    /// What shape the provider needs for these refs.
    pub provider_format: RefProviderFormat,
    /// Per-role specs.
    pub roles: HashMap<String, RefRoleSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefRoleSpec {
    #[serde(default)] pub required: bool,
    pub min_count: u32,
    pub max_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum RefProviderFormat {
    /// Provider needs an https URL per ref.
    Url,
    /// Provider needs base64 in the JSON body.
    Base64,
    /// Provider needs multipart/form-data. Map role → form field name.
    Multipart { field_map: HashMap<String, String> },
}

fn default_true() -> bool { true }
```

**Notes on the type design:**

- `ParamSpec` is a tagged enum so the loader produces a typed value the validator dispatches on with `match`. No generic `Value` blobs leak into the validator.
- `Size` and `AspectRatio` are first-class because they're the most divergent parameters in practice; treating them as plain strings would push enumeration into every provider.
- `Seed` is its own variant because its bounds are model-specific (4-byte vs 8-byte seeds) and we want range validation for free.
- `RefInputSpec.default_role` is what makes single-ref ergonomic: clients can send `{type, value}` without a role and we'll assign `init` (or whatever the model declares).
- `extra_allowlist` is a flat list — only top-level keys are validated. Nested objects pass through opaquely. This is intentional: the allowlist is a safety rail, not a deep schema.

## 6. YAML format

`models/stability.yaml`:

```yaml
models:
  - id: stability/sd3-large
    provider: stability
    media_type: image
    display_name: Stable Diffusion 3 Large
    description: Latest Stability AI model with excellent prompt adherence.
    pricing:
      base_cost_usd: 0.065
    capabilities:
      text_to_image: true
      image_to_image: true
    prompt:
      required: true
      max_length: 10000
    params:
      negative_prompt:
        kind: string
        max_length: 10000
      seed:
        kind: seed
        min: 0
        max: 4294967294
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1", "16:9", "9:16", "3:2", "2:3", "4:5", "5:4", "21:9", "9:21"]
        default: "1:1"
      strength:
        kind: float
        min: 0.0
        max: 1.0
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format:
        form: multipart
        field_map:
          init: image
      roles:
        init:
          required: false
          min_count: 0
          max_count: 1
    extra_allowlist: [output_format, style_preset]
    tags: [text-to-image, image-to-image]

  - id: stability/sd3-turbo
    provider: stability
    media_type: image
    display_name: Stable Diffusion 3 Turbo
    description: Faster, cheaper SD3 variant.
    pricing:
      base_cost_usd: 0.04
    capabilities:
      text_to_image: true
    prompt:
      required: true
      max_length: 10000
    params:
      seed:
        kind: seed
        min: 0
        max: 4294967294
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1"]
        default: "1:1"
    extra_allowlist: []
    tags: [text-to-image, fast]
```

`models/openai.yaml` (covers DALL-E 3 fixed-enum sizes):

```yaml
models:
  - id: openai/dall-e-3
    provider: openai
    media_type: image
    display_name: DALL-E 3
    description: OpenAI's latest text-to-image model.
    pricing:
      base_cost_usd: 0.04
    capabilities:
      text_to_image: true
    prompt:
      required: true
      max_length: 4000
    params:
      size:
        kind: size
        mode: enum
        values:
          - [1024, 1024]
          - [1792, 1024]
          - [1024, 1792]
      quality:
        kind: string
        enum_values: [standard, hd]
        default: standard
      style:
        kind: string
        enum_values: [vivid, natural]
        default: vivid
    extra_allowlist: []
    tags: [text-to-image]
```

**Files (12 total):**
- `models/openai.yaml` — DALL-E 2, DALL-E 3, Sora
- `models/stability.yaml` — SD3-Large, SD3-Turbo, Core, Ultra, SDXL
- `models/replicate.yaml` — Flux Pro/Dev/Schnell, SDXL, SD3
- `models/google.yaml` — Imagen 3, Gemini 2.5 Flash, Gemini 3 Pro
- `models/fal.yaml` — Flux family, SDXL, SD3.5-M, Recraft V3, AuraFlow + Fal video
- `models/runway.yaml` — Gen-3, Gen-3 Turbo
- `models/luma.yaml` — Dream Machine, Ray 2/3, Ray Flash 2, Ray HDR 3
- `models/mock.yaml` — mock/image-gen, mock/video-gen (for tests)

Loader globs `models/*.yaml`, parses each, builds the registry. Duplicate IDs are a hard load error. A missing file just means no models for that provider.

## 7. Loader behavior

- Discovers files via `models/*.yaml` (path configurable; defaults to `./models` relative to working dir, then `<binary_dir>/models` as fallback for packaged installs).
- Parses each file with `serde_yaml`.
- Validates per-model invariants:
  - `id` must contain `/` and the part before `/` must equal `provider`
  - `pricing.base_cost_usd >= 0`
  - For `SizeSpec::Freeform`: `min_width <= max_width`, same for height
  - For `RefInputSpec`: every key in `field_map` must appear in `roles` (and vice versa for Multipart)
  - Param keys (in `params:`) must match the canonical names declared in `KNOWN_PARAMS` (`negative_prompt`, `seed`, `steps`, `guidance_scale`, `strength`, `quality`, `style`, `size`, `aspect_ratio`, `duration_seconds`, `resolution`, `fps`)
- Aggregates errors so one bad file reports all problems, not just the first.
- Returns `Result<CapabilityRegistry, LoadError>` with file path + line number context on failures (`serde_yaml` location info).

## 8. Registry API

`src/capabilities/registry.rs`:

```rust
pub struct CapabilityRegistry {
    models: HashMap<String, ModelSchema>,           // by id
    by_provider: HashMap<String, Vec<String>>,      // provider → ids
}

impl CapabilityRegistry {
    pub fn from_dir(path: &Path) -> Result<Self, LoadError> { ... }
    pub fn from_yaml_str(yaml: &str) -> Result<Self, LoadError> { ... } // for tests
    pub fn get(&self, id: &str) -> Option<&ModelSchema>;
    pub fn all(&self) -> impl Iterator<Item = &ModelSchema>;
    pub fn for_provider(&self, provider: &str) -> impl Iterator<Item = &ModelSchema>;
    pub fn len(&self) -> usize;
}
```

Shared across the app via `Arc<CapabilityRegistry>` in `AppState`.

## 9. Request shapes

New, in `src/types/mod.rs`:

```rust
/// Fields every generation request shares.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BaseGenerationRequest {
    pub prompt: String,
    pub model: String,
    #[serde(default = "default_n")]
    pub n: u32,
    #[serde(default)] pub negative_prompt: Option<String>,
    #[serde(default)] pub seed: Option<i64>,
    #[serde(default)] pub reference_images: Vec<ReferenceImage>,
    /// Strict-mode toggle. Default true. When false, unknown/unsupported params
    /// are dropped instead of rejected; clients receive an X-Litegen-Dropped-Params
    /// header listing what was dropped.
    #[serde(default = "default_true")]
    pub strict: bool,
    /// Free-form provider-specific params. In strict mode (default), only keys
    /// in the model's extra_allowlist are accepted. In lax mode, all keys pass
    /// through to the provider verbatim.
    #[serde(default)]
    pub extra: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageGenerationRequest {
    #[serde(flatten)] pub base: BaseGenerationRequest,
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
    #[serde(flatten)] pub base: BaseGenerationRequest,
    #[serde(default = "default_duration")]
    pub duration_seconds: f64,
    #[serde(default)] pub aspect_ratio: Option<String>,
    #[serde(default)] pub resolution: Option<String>,
    #[serde(default)] pub fps: Option<u32>,
}

/// A single reference image. The proxy adapts this to whatever the target
/// model's provider needs.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReferenceImage {
    #[serde(rename = "type")]
    pub kind: RefImageKind,
    /// For Base64: the base64 string (data: prefix stripped if present).
    /// For Url: an https URL.
    /// For Blob: the multipart form field name. The actual bytes live in the
    /// multipart request's file part with that name.
    pub value: String,
    /// Role assigned to this image. If omitted, the registry's default_role
    /// for the target model is used. Strict mode: must be a declared role.
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefImageKind { Base64, Url, Blob }
```

**Deleted fields:**
- `ImageGenerationRequest`: `image_url`, `image_base64`, `mask_url`
- `VideoGenerationRequest`: `image_url`, `image_base64`, `first_frame_url`, `last_frame_url`

Clients must use `reference_images`. Roles map old fields like so (documented in the changelog):

| Old field | New ref entry |
|---|---|
| `image_url` | `{type: "url", value: "...", role: "init"}` |
| `image_base64` | `{type: "base64", value: "...", role: "init"}` |
| `mask_url` | `{type: "url", value: "...", role: "mask"}` |
| `first_frame_url` | `{type: "url", value: "...", role: "first_frame"}` |
| `last_frame_url` | `{type: "url", value: "...", role: "last_frame"}` |

## 10. Multipart upload format

When a client wants to use `Blob` refs, the request is `multipart/form-data`:

- Part `request` (Content-Type: `application/json`) — the `ImageGenerationRequest` / `VideoGenerationRequest` JSON. Refs of type `blob` have `value: "<part_name>"`.
- One part per blob ref, named to match.

Example:

```
POST /v1/images/generations HTTP/1.1
Content-Type: multipart/form-data; boundary=---X

---X
Content-Disposition: form-data; name="request"
Content-Type: application/json

{"model":"stability/sd3-large","prompt":"...","reference_images":[
  {"type":"blob","value":"init_img","role":"init"}
]}
---X
Content-Disposition: form-data; name="init_img"; filename="cat.png"
Content-Type: image/png

<bytes>
---X--
```

Application/JSON requests (no blobs) keep working unchanged.

## 11. Validator middleware

`src/api/middleware/validator.rs` — implemented as a **custom Axum extractor** `ValidatedRequest<T>` that wraps either `Json<T>` or `Multipart` depending on Content-Type. The extractor runs after deserialization and before the handler body; it holds an `Arc<CapabilityRegistry>` pulled from `AppState`. We use an extractor (not a tower middleware layer) because we need the typed, deserialized `T` to validate against, and extractors are the idiomatic axum 0.7+ way to do that.

The extractor:

1. Deserializes the request from JSON or multipart (resolving blob refs to their byte parts as part of the validated value).
2. Looks up the model in the registry. Missing → `404 model_not_found`.
3. For each declared field on the request struct that's `Some`, dispatches on the param's `ParamSpec` in the schema:
   - `size: "1024x1024"` against `SizeSpec::Enum` — must match an entry verbatim.
   - `size: "768x768"` against `SizeSpec::Freeform` — within min/max and (if set) divisible by `multiple_of`.
   - `aspect_ratio: "16:9"` against `AspectRatio.allowed` — must be in the list.
   - `seed` against `Seed { min, max }` — within range.
   - `guidance_scale` against `Float` — within range.
   - `negative_prompt` against `String { max_length }` — within length.
4. If the model's schema *doesn't* declare a param the request includes (e.g. `guidance_scale` on DALL-E 3):
   - Strict (default): `400 unsupported_parameter`.
   - Lax: drop the value (replace with `None`), record to `dropped_params: Vec<String>`.
5. Validates `reference_images`:
   - Total count ≤ `max_total`.
   - Each ref's role (or `default_role` if absent) must be a key in `roles`.
   - Per-role counts within `[min_count, max_count]`.
   - Required roles must be present.
6. Validates `extra`:
   - Strict: every top-level key must be in `extra_allowlist`. Unknown → 400.
   - Lax: pass through unchanged.
7. On lax-mode drops, sets response header `X-Litegen-Dropped-Params: <comma list>`.
8. On any violation in strict mode, returns:

```json
{
  "error": {
    "type": "validation_error",
    "code": "param_unsupported",        // or param_out_of_range, ref_role_unknown, etc.
    "message": "Parameter 'guidance_scale' is not supported by model 'openai/dall-e-3'.",
    "param": "guidance_scale",
    "model": "openai/dall-e-3"
  }
}
```

Error codes (canonical set): `param_unsupported`, `param_out_of_range`, `param_enum_mismatch`, `param_too_long`, `param_pattern_mismatch`, `prompt_too_long`, `prompt_too_short`, `prompt_required`, `ref_total_exceeded`, `ref_role_unknown`, `ref_role_count_out_of_range`, `ref_role_required`, `extra_key_unsupported`, `model_not_found`.

## 12. Materializer

`src/proxy/materializer.rs` — consumes a validated request + the model's `RefInputSpec` and returns a `MaterializedRequest` ready for the provider.

```rust
pub struct MaterializedRequest {
    pub base: BaseGenerationRequest,
    pub refs: Vec<MaterializedRef>,
    pub cleanup: Cleanup,        // drop guard runs after the request finishes
}

pub struct MaterializedRef {
    pub role: String,
    pub form: MaterializedRefForm,
}

pub enum MaterializedRefForm {
    Url(String),
    Base64(String),
    /// For multipart: pre-built form field with bytes.
    MultipartField { field_name: String, bytes: Bytes, content_type: String },
}
```

**Conversions:**

| Input → Provider needs | Action |
|---|---|
| URL → URL | passthrough |
| URL → Base64 | fetch bytes, base64 encode |
| URL → Multipart | fetch bytes, build form field |
| Base64 → URL | decode, upload to `storage` backend (same one used for outputs), record temp object key, return public URL |
| Base64 → Base64 | passthrough (strip data: prefix) |
| Base64 → Multipart | decode, build form field |
| Blob → URL | upload bytes to storage, return public URL |
| Blob → Base64 | base64 encode the bytes |
| Blob → Multipart | passthrough |

**Cleanup:**

`Cleanup` holds a list of storage object keys created during materialization. Its `Drop` impl spawns a tokio task to delete them. For tests this is observable via a counter.

The materializer reuses the existing `proxy::storage` backend (S3 or local) — no new storage code. Temp objects go under a `tmp/` prefix so a cron / lifecycle policy can sweep stragglers if the cleanup task fails.

## 13. Provider trait changes

`ImageProvider` and `VideoProvider` are slimmed down:

```rust
#[async_trait]
pub trait ImageProvider: Send + Sync {
    fn name(&self) -> &str;
    fn configure(&mut self, config: ProviderInstanceConfig);
    fn is_configured(&self) -> bool;

    async fn generate(
        &self,
        model: &ModelSchema,
        request: MaterializedRequest,
        image_extras: ImageExtras,
    ) -> Result<GenerationOutput, ProviderError>;

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProviderError>;

    async fn health_check(&self) -> HealthCheckResult;
}

pub struct ImageExtras {
    pub size: Option<String>,
    pub aspect_ratio: Option<String>,
    pub quality: Option<String>,
    pub style: Option<String>,
    pub steps: Option<u32>,
    pub guidance_scale: Option<f64>,
    pub strength: Option<f64>,
    pub response_format: String,
    /// Extra params post-allowlist filtering.
    pub extra: Option<serde_json::Value>,
}
```

**Removed methods:** `supported_models`, `supports_model`, `list_models`.

These responsibilities now belong to the registry. The registry knows which provider owns which model via `ModelSchema.provider`. The router asks the registry for the model, then dispatches to that provider.

The provider gets the `ModelSchema` reference so it can read declared things like `extra_allowlist` if it wants to do its own logging.

## 14. API endpoint changes

| Endpoint | Change |
|---|---|
| `GET /v1/models` | Now sourced from registry. Returns paginated `ModelInfo` (slim summary). |
| `GET /v1/models/{id}` | **NEW.** Returns the full `ModelSchema` for one model — including `params`, `ref_inputs`, `extra_allowlist`. This is what powers dashboard introspection and client SDKs. |
| `POST /v1/images/generations` | Validator middleware in front. Accepts new `reference_images` shape. Legacy singular fields are gone (deserialization will fail if present). Accepts `multipart/form-data` for blob refs. |
| `POST /v1/videos/generations` | Same. |
| `POST /v1/images/cost` and `POST /v1/videos/cost` | Same validation runs (estimate paths benefit from rejection too). |

## 15. Error path compatibility

Existing error response shape ([`src/types/mod.rs:517`](../../../litegen-core/src/types/mod.rs#L517)) supports our needs already (`ErrorDetail { message, type, code, provider_error }`). We add a small extension for validation errors:

```rust
pub struct ErrorDetail {
    pub message: String,
    pub r#type: String,                       // "validation_error" for new ones
    pub code: Option<String>,                 // canonical code from §11
    pub provider_error: Option<Value>,
    pub param: Option<String>,                // NEW — which param failed
    pub model: Option<String>,                // NEW — which model
}
```

`param` and `model` are `Option` so existing error paths are unaffected.

## 16. Dashboard impact

The dashboard's Models page can render parameter schemas (a "what params does this model take" panel) by hitting `GET /v1/models/{id}`. Out of scope for this design's *implementation* (frontend follow-up), but the API contract is locked.

## 17. Testing strategy

TDD throughout — write tests first per the test-driven-development skill.

**Unit tests (per module):**

- `loader_tests.rs`
  - Round-trips: each shipped `models/*.yaml` parses without error.
  - Negative: duplicate IDs across files → load error.
  - Negative: provider/id mismatch → load error.
  - Negative: ref `field_map` references undeclared role → load error.
  - Negative: `SizeSpec::Freeform` with min > max → load error.
  - Negative: param key not in `KNOWN_PARAMS` → load error.

- `registry_tests.rs`
  - `get`, `for_provider`, `all` correctness.
  - Lookup hit + miss.
  - From-string constructor for inline yaml fixtures.

- `validator_tests.rs`
  - Each `ParamSpec` variant: one happy-path test + at least one rejection test.
  - Strict vs lax behavior for unknown params.
  - `reference_images` cardinality (under, at, over).
  - Required role missing → 400.
  - Unknown role in strict → 400; in lax → dropped + header.
  - `extra` allowlist enforcement.
  - Prompt length min/max + required.
  - Error codes match the canonical set.

- `materializer_tests.rs`
  - Each conversion in the matrix in §12 — happy path.
  - Cleanup invoked exactly once per materialization.
  - Failed upload bubbles a `ProviderError::InvalidRequest`.
  - URL fetch with non-200 → error (no temp upload performed).
  - Mock storage backend used; assert no real network.

- `multipart_tests.rs`
  - Request JSON + N file parts deserialized correctly.
  - Blob ref whose name has no matching part → 400.
  - Extra file parts → ignored (warning logged, no failure).

**Integration tests (per provider):**

For each of the 12 providers, a test using `wiremock` (or `mockito`) to assert the outbound request body matches what we expect for a representative model. These tests catch the silent-drop bug at provider granularity — if we previously dropped `negative_prompt`, the integration test now fails because the mock server sees the parameter.

- `tests/providers/openai_dalle3.rs` — DALL-E 3 with each supported size, quality, style.
- `tests/providers/stability_sd3.rs` — SD3 with aspect ratio, seed, negative prompt; ref image as init.
- `tests/providers/replicate_flux.rs`
- `tests/providers/google_gemini.rs`
- `tests/providers/fal_flux.rs`
- `tests/providers/runway_gen3.rs` — duration enum enforcement.
- `tests/providers/luma_ray.rs` — first_frame + last_frame refs.
- Plus mock providers for sanity.

**E2E tests:**

- `tests/e2e/validator_strict.rs` — POST with unsupported param → 400 with canonical error shape.
- `tests/e2e/validator_lax.rs` — POST with `strict: false` and unsupported param → 200 + `X-Litegen-Dropped-Params` header lists the dropped one.
- `tests/e2e/multipart_roundtrip.rs` — blob ref → URL provider → storage upload happens → cleanup runs.

**Coverage target:** every line of `capabilities/*`, `materializer.rs`, `validator.rs` must be covered. Provider modules: every `generate()` branch (each ParamSpec consumer) exercised by at least one integration test.

## 18. Implementation order

1. `capabilities/schema.rs` types + unit tests (no behavior yet).
2. `capabilities/loader.rs` + `registry.rs` + tests. Shipping `models/*.yaml` for current 12 providers (transcribe from each provider's existing `list_models`).
3. New request types (`BaseGenerationRequest`, `ReferenceImage`, refactored `Image/VideoGenerationRequest`) + tests.
4. `materializer.rs` + tests, using existing `proxy::storage`.
5. `validator.rs` middleware + tests.
6. Wire registry into `AppState`. Wire middleware into router.
7. Rewrite provider trait. Rewrite each provider's `generate()` to consume the materialized request. Drop `list_models` / `supported_models`.
8. Add `GET /v1/models/{id}` handler. Update `GET /v1/models` to source from registry.
9. Multipart handling: extractor that supports both JSON-only and multipart.
10. Integration tests per provider.
11. E2E tests.
12. Update `litegen.example.yaml` if model paths need to be configurable.

Each step ends with `cargo test -p litegen-core` green before moving on.

## 19. Risks & mitigations

| Risk | Mitigation |
|---|---|
| `models/*.yaml` drift from real provider APIs as providers iterate. | Integration tests against mocked provider servers catch the moment a schema disagrees with provider behavior. Schema yaml lives next to code, owned by the same PRs that touch providers. |
| Loader errors are confusing without good locations. | `serde_yaml` returns line/col; we surface file + line in `LoadError`. |
| Multipart parsing is the new biggest attack surface for bad input. | Use Axum's `Multipart` extractor (battle-tested). Reject parts > 25 MB by default; configurable. |
| Storage upload latency adds to perceived request latency. | Already a concern for output storage; materializer uses the same backend with the same characteristics. |
| Removing `list_models` from provider trait could leak — anywhere still calling it breaks. | Compiler catches this; we delete callsites as part of the same change. |

## 20. Open follow-ups (outside this design's scope)

- `GET /v1/generations/{id}` (the missing video-polling endpoint from the original audit).
- Streaming responses for providers that support them.
- Per-key billing/quota endpoints.
- Schema hot-reload (file watcher).
- Dashboard panel rendering the per-model schema introspectively.
