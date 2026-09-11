# 3D Model Generation (`model3d`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `model3d` as a third first-class media family in litegen — API, storage re-host, async poller, mock provider with a real GLB, SDK namespace, dashboard viewer + Playground — conforming to the wire contract aipix is already coded against.

**Architecture:** `Model3dProvider` mirrors `VideoProvider` (async submit → poll), but poll results carry **bytes**, not provider URLs: litegen re-hosts every mesh/texture/preview file on its own storage (S3 when configured, per-app BYO storage when the app has it, an in-process local store serving absolute URLs otherwise) so BYO buckets work identically across all three modalities. Completed generations return an `assets: Model3dAsset[]` array; exactly one asset has `kind: "mesh"`.

**Tech Stack:** Rust (axum 0.8, utoipa 5, sqlx 0.8, async-trait, wiremock), TypeScript SDK (openapi-typescript generated), React dashboard (Vite + Playwright), Node landing generators.

**Spec:** [`docs/superpowers/specs/2026-08-19-3d-model-generation-design.md`](../specs/2026-08-19-3d-model-generation-design.md) — litegen-side architecture. **Contract of record:** `~/source/repos/aipix/docs/litegen-3d-specs.md` — the consumer contract, which **overrides** the design doc wherever they disagree (see Global Constraints).

## Global Constraints

- **Wire media type is `"model3d"`, never `"model_3d"`.** The Rust enum variant `MediaType::Model3d` under `#[serde(rename_all = "snake_case")]` already serializes to exactly `"model3d"` (digits do not introduce underscores). Do **not** add a `#[serde(rename)]`. A test locks this.
- **Routes are `/v1/models3d/...`**, not the design doc's `/v1/3d/...`:
  `POST /v1/models3d/generations`, `POST /v1/models3d/cost`, `GET /v1/models3d/{id}`.
- **Completed responses carry `assets: Model3dAsset[]`**, not `model_url`/`thumbnail_url`.
- **A completed generation MUST carry exactly one `kind: "mesh"` asset.** Never report `completed` without it — aipix refunds the slot and treats it as a provider failure.
- **Every asset `url` MUST be absolute** (`http://…` / `https://…`). A root-relative path like `/mock/video/{id}` (what the video mock returns today) is a contract violation here.
- **`strict: false` must drop unsupported params and report them in `X-Litegen-Dropped-Params`**, never error. This is the single most important behaviour in the contract.
- **3D params are first-class `ParamSpec` entries**, not `extra_allowlist` passthrough (this reverses design-doc decision 6, at aipix's §4 request). New `KNOWN_PARAMS`: `output_format`, `texture`, `pbr`, `target_polycount`, `symmetry`, `topology`, `rig`.
- **Storage uses the `ImageStorage` trait (`put(key, bytes, content_type)`), never `ImageStore`.** `S3Store::store` infers the extension from a hardcoded image content-type switch (`storage.rs:104-109`) and would silently save a `.glb` as `.png`.
- **Scope of this plan:** foundation + mock provider only. Meshy / Tripo3D / Stability / Rodin adapters are explicitly out of scope (spec §11 phase 2).
- **Git:** commit directly to `master` after each task; push to `origin master`. No branches, no PRs, no `Co-Authored-By: Claude` trailer.
- **Acceptance:** `cargo test -p litegen` green, `cargo clippy --all-targets -- -D warnings` clean, `cd dashboard && npm run build && npx playwright test` clean, `cd sdks/typescript && npm run build && npm test` clean.

## File Structure

**Create**
| Path | Responsibility |
|---|---|
| `litegen-core/src/providers/model3d/mod.rs` | Module root; `pub mod glb; pub mod mock;` |
| `litegen-core/src/providers/model3d/glb.rs` | Pure glTF-2.0-binary writer — builds a valid, prompt-varied cube GLB |
| `litegen-core/src/providers/model3d/mock.rs` | `MockModel3dProvider` — progress ramp, real GLB bytes, `mock/fail-3d` |
| `dashboard/src/components/ModelViewer.tsx` | Lazy-loading `<model-viewer>` wrapper used by every 3D surface |
| `dashboard/src/playground/ResultTile3D.tsx` | Playground tile rendering a mesh instead of an image |
| `dashboard/tests/model3d.spec.ts` | Playwright coverage for the gallery + Playground 3D paths |

**Modify** — `capabilities/schema.rs`, `capabilities/loader.rs`, `types/mod.rs`, `providers/mod.rs`, `proxy/{registry,router,poller,storage}.rs`, `api/handlers/mod.rs`, `api/middleware/validator.rs`, `api/openapi.rs`, `config/mod.rs`, `db/{trait_def,sqlite,postgres}.rs`, `models/mock.yaml`, `sdks/typescript/src/{polling,client,index}.ts`, `dashboard/src/pages/{Generations,Models}.tsx`, `dashboard/src/components/{ModelDetail,TracePanel}.tsx`, `dashboard/src/playground/{types.ts,useFanOut.ts,SingleMode.tsx,ResultGrid.tsx}`, `apps/landing/scripts/derive-{models,capabilities}.mjs`.

---

### Task 1: `MediaType::Model3d` + 3D capability flags + `ParamSpec` labels

**Status:** ✅ DONE (2026-08-20, d5011ec) — task review clean

**Files:**
- Modify: `litegen-core/src/capabilities/schema.rs`
- Modify: `litegen-core/src/types/mod.rs:217-220` (`MediaType`), `:227-243` (`ModelCapabilities`)
- Modify: `litegen-core/src/api/handlers/mod.rs:640-670` (`project_model_info`)
- Test: `litegen-core/tests/unit_tests.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `capabilities::MediaType::Model3d`; `types::MediaType::Model3d`; `ModelCapabilityFlags { text_to_3d, image_to_3d, multiview_to_3d }`; `ModelCapabilities { supports_text_to_3d, supports_image_to_3d, supports_multiview_to_3d, supports_pbr, supports_rig, supports_texture, output_formats: Vec<String>, max_polycount: Option<u32> }`; `label: Option<String>` / `description: Option<String>` on every `ParamSpec*` payload struct; extended `KNOWN_PARAMS`.

- [x] DONE 2026-08-20 **Step 1: Write the failing wire-format test**

Append inside `mod tests` in `litegen-core/tests/unit_tests.rs`:

```rust
// ─── model3d wire format ────────────────────────────────────────────────

#[test]
fn media_type_model3d_serializes_without_underscore() {
    // aipix discriminates on the literal string "model3d" (ai-model.entity.ts
    // stores it in a varchar(10)). serde's snake_case does not insert an
    // underscore before a digit, so this is the natural output — but it is
    // load-bearing enough to lock down.
    let t = serde_json::to_string(&MediaType::Model3d).unwrap();
    assert_eq!(t, "\"model3d\"");
    let back: MediaType = serde_json::from_str("\"model3d\"").unwrap();
    assert_eq!(back, MediaType::Model3d);

    let c = serde_json::to_string(&litegen::capabilities::MediaType::Model3d).unwrap();
    assert_eq!(c, "\"model3d\"");
}

#[test]
fn model3d_capability_flags_default_false_and_round_trip() {
    let yaml = "text_to_3d: true\nimage_to_3d: true\nmultiview_to_3d: false\n";
    let flags: litegen::capabilities::ModelCapabilityFlags =
        serde_yaml::from_str(yaml).unwrap();
    assert!(flags.text_to_3d);
    assert!(flags.image_to_3d);
    assert!(!flags.multiview_to_3d);
    // Image/video flags stay defaulted — the new fields are purely additive.
    assert!(!flags.text_to_image);
    assert!(!flags.text_to_video);
}

#[test]
fn param_spec_carries_optional_label_and_description() {
    let yaml = "kind: int\nmin: 100\nmax: 300000\nlabel: Target polycount\ndescription: Approximate triangle budget\n";
    let spec: litegen::capabilities::ParamSpec = serde_yaml::from_str(yaml).unwrap();
    match spec {
        litegen::capabilities::ParamSpec::Int(i) => {
            assert_eq!(i.label.as_deref(), Some("Target polycount"));
            assert_eq!(i.description.as_deref(), Some("Approximate triangle budget"));
            assert_eq!(i.min, Some(100));
        }
        other => panic!("expected Int, got {other:?}"),
    }
    // Absent label/description stay None and are omitted from JSON.
    let bare: litegen::capabilities::ParamSpec = serde_yaml::from_str("kind: bool\n").unwrap();
    let json = serde_json::to_string(&bare).unwrap();
    assert!(!json.contains("label"), "label must be skipped when None: {json}");
}

#[test]
fn known_params_include_the_3d_knobs() {
    for k in ["output_format", "texture", "pbr", "target_polycount", "symmetry", "topology", "rig"] {
        assert!(
            litegen::capabilities::KNOWN_PARAMS.contains(&k),
            "'{k}' must be a first-class param — aipix renders one control per params entry"
        );
    }
}
```

- [x] DONE 2026-08-20 **Step 2: Run the tests to verify they fail**

Run: `cd litegen-core && cargo test --test unit_tests model3d 2>&1 | tail -20`
Expected: FAIL — `no variant named Model3d`, `no field text_to_3d`, `no field label`.

- [x] DONE 2026-08-20 **Step 3: Add the enum variant and flags in `capabilities/schema.rs`**

```rust
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
```

- [x] DONE 2026-08-20 **Step 4: Add `label`/`description` to every `ParamSpec` payload struct**

Add these two lines to **each** of `ParamSpecBool`, `ParamSpecInt`, `ParamSpecFloat`, `ParamSpecString`, `ParamSpecAspectRatio`, `ParamSpecSeed`, `SizeSpecFreeform`, `SizeSpecEnum` in `capabilities/schema.rs`:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")] pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
```

`ParamSpecSeed` currently derives `Copy` — remove `Copy` from its derive list (a `String` field is not `Copy`) and fix any resulting move errors by cloning. `ParamSpecAspectRatio`, `SizeSpecFreeform`, `SizeSpecEnum` have no `Default` derive; leave their derives otherwise unchanged.

- [x] DONE 2026-08-20 **Step 5: Extend `KNOWN_PARAMS`**

```rust
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
    "output_format",
    "texture",
    "pbr",
    "target_polycount",
    "symmetry",
    "topology",
    "rig",
];
```

- [x] DONE 2026-08-20 **Step 6: Add the `types::MediaType` variant and the 3D `ModelCapabilities` fields**

In `litegen-core/src/types/mod.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Image,
    Video,
    Model3d,
}
```

and append to `ModelCapabilities` (all additive, all defaulted so existing image/video JSON still deserializes):

```rust
    // 3D-specific
    #[serde(default)]
    pub supports_text_to_3d: bool,
    #[serde(default)]
    pub supports_image_to_3d: bool,
    #[serde(default)]
    pub supports_multiview_to_3d: bool,
    #[serde(default)]
    pub supports_pbr: bool,
    #[serde(default)]
    pub supports_rig: bool,
    #[serde(default)]
    pub supports_texture: bool,
    #[serde(default)]
    pub output_formats: Vec<String>,
    #[serde(default)]
    pub max_polycount: Option<u32>,
```

- [x] DONE 2026-08-20 **Step 7: Let the compiler find every match arm and struct literal**

Run: `cd litegen-core && cargo build 2>&1 | grep -E "^error" -A 8 | head -60`

Fix each site. The known ones:

`api/handlers/mod.rs` `project_model_info` — the `media_type` match and the `ModelCapabilities` literal:

```rust
        media_type: match s.media_type {
            crate::capabilities::MediaType::Image => MediaType::Image,
            crate::capabilities::MediaType::Video => MediaType::Video,
            crate::capabilities::MediaType::Model3d => MediaType::Model3d,
        },
        is_available: true,
        capabilities: ModelCapabilities {
            supports_text_to_image: s.capabilities.text_to_image,
            supports_image_to_image: s.capabilities.image_to_image,
            supports_inpainting: s.capabilities.inpainting,
            supports_text_to_video: s.capabilities.text_to_video,
            supports_image_to_video: s.capabilities.image_to_video,
            supports_first_frame: s.ref_inputs.as_ref().is_some_and(|ri| ri.roles.contains_key("first_frame")),
            supports_last_frame: s.ref_inputs.as_ref().is_some_and(|ri| ri.roles.contains_key("last_frame")),
            supported_sizes: extract_sizes(s),
            max_images: s.ref_inputs.as_ref().map(|ri| ri.max_total).unwrap_or(1),
            max_duration_seconds: None,
            supports_text_to_3d: s.capabilities.text_to_3d,
            supports_image_to_3d: s.capabilities.image_to_3d,
            supports_multiview_to_3d: s.capabilities.multiview_to_3d,
            supports_pbr: s.params.contains_key("pbr"),
            supports_rig: s.params.contains_key("rig"),
            supports_texture: s.params.contains_key("texture"),
            output_formats: extract_output_formats(s),
            max_polycount: extract_max_polycount(s),
        },
```

and add these two helpers next to `extract_sizes`:

```rust
/// `output_formats` is derived from the model's declared `output_format`
/// enum_values, so a model advertises exactly the formats it can emit.
fn extract_output_formats(s: &crate::capabilities::ModelSchema) -> Vec<String> {
    match s.params.get("output_format") {
        Some(crate::capabilities::ParamSpec::String(sp)) => sp.enum_values.clone(),
        _ => Vec::new(),
    }
}

/// `max_polycount` mirrors the upper bound of the `target_polycount` param.
fn extract_max_polycount(s: &crate::capabilities::ModelSchema) -> Option<u32> {
    match s.params.get("target_polycount") {
        Some(crate::capabilities::ParamSpec::Int(i)) => i.max.map(|m| m.max(0) as u32),
        _ => None,
    }
}
```

`stability.rs:620` builds a `ModelCapabilities` literal — add `..Default::default()` only if the struct derives `Default`; it does not, so spell out the eight new fields as `false` / `Vec::new()` / `None`.

- [x] DONE 2026-08-20 **Step 8: Run the tests to verify they pass**

Run: `cd litegen-core && cargo test --test unit_tests model3d 2>&1 | tail -20`
Expected: PASS (4 tests).

Run: `cd litegen-core && cargo test 2>&1 | tail -20`
Expected: no regressions.

- [x] DONE 2026-08-20 **Step 9: Commit**

```bash
git add litegen-core/src/capabilities/schema.rs litegen-core/src/types/mod.rs \
        litegen-core/src/api/handlers/mod.rs litegen-core/src/providers/image/stability.rs \
        litegen-core/tests/unit_tests.rs
git commit -m "feat(3d): model3d media type, capability flags, and labelled params"
git push origin master
```

---

### Task 2: `Model3dAsset` + request/response types + OpenAPI registration

**Status:** ✅ DONE (2026-08-20, 5a85adb) — task review clean

**Files:**
- Modify: `litegen-core/src/types/mod.rs` (after `VideoGenerationResponse`, ~line 143)
- Modify: `litegen-core/src/api/openapi.rs` (`components(schemas(...))`)
- Test: `litegen-core/tests/unit_tests.rs`

**Interfaces:**
- Consumes: `MediaType::Model3d`, `GenerationStatus`, `UsageInfo` (Task 1 / existing).
- Produces: `types::Model3dAssetKind`, `types::Model3dAsset`, `types::Model3dGenerationRequest`, `types::Model3dGenerationResponse`.

- [x] DONE 2026-08-20 **Step 1: Write the failing test**

Append to `mod tests` in `litegen-core/tests/unit_tests.rs`:

```rust
#[test]
fn model3d_request_deserializes_with_flattened_base() {
    let json = serde_json::json!({
        "model": "mock/mesh-3d",
        "prompt": "a low-poly fox",
        "output_format": "glb",
        "texture": true,
        "target_polycount": 5000,
        "topology": "triangle",
        "strict": false
    });
    let req: Model3dGenerationRequest = serde_json::from_value(json).unwrap();
    assert_eq!(req.base.model, "mock/mesh-3d");
    assert_eq!(req.base.prompt, "a low-poly fox");
    assert!(!req.base.strict);
    assert_eq!(req.output_format.as_deref(), Some("glb"));
    assert_eq!(req.texture, Some(true));
    assert_eq!(req.target_polycount, Some(5000));
    assert_eq!(req.topology.as_deref(), Some("triangle"));
    // Unset optionals stay None rather than defaulting to a vendor opinion.
    assert_eq!(req.pbr, None);
    assert_eq!(req.rig, None);
    assert_eq!(req.symmetry, None);
}

#[test]
fn model3d_asset_kind_serializes_lowercase() {
    assert_eq!(serde_json::to_string(&Model3dAssetKind::Mesh).unwrap(), "\"mesh\"");
    assert_eq!(serde_json::to_string(&Model3dAssetKind::Texture).unwrap(), "\"texture\"");
    assert_eq!(serde_json::to_string(&Model3dAssetKind::Preview).unwrap(), "\"preview\"");
}

#[test]
fn model3d_response_omits_empty_assets_and_keeps_progress() {
    let pending = Model3dGenerationResponse {
        id: "litegen-3d-1".into(),
        status: GenerationStatus::Pending,
        model: "mock/mesh-3d".into(),
        provider: "mock".into(),
        assets: Vec::new(),
        progress: 0,
        error: None,
        usage: None,
        created: 1_760_000_000,
    };
    let v = serde_json::to_value(&pending).unwrap();
    assert!(v.get("assets").is_none(), "empty assets must be omitted, not sent as []");
    assert_eq!(v["progress"], 0);
    assert_eq!(v["status"], "pending");

    let done = Model3dGenerationResponse {
        assets: vec![Model3dAsset {
            kind: Model3dAssetKind::Mesh,
            url: "https://cdn.example.com/litegen/3d/litegen-3d-1/model.glb".into(),
            format: "glb".into(),
            size_bytes: Some(780),
            polycount: Some(12),
            width: None,
            height: None,
        }],
        status: GenerationStatus::Completed,
        progress: 100,
        ..pending
    };
    let v = serde_json::to_value(&done).unwrap();
    assert_eq!(v["assets"][0]["kind"], "mesh");
    assert_eq!(v["assets"][0]["format"], "glb");
    assert!(v["assets"][0]["url"].as_str().unwrap().starts_with("https://"));
    assert!(v["assets"][0].get("width").is_none(), "None dimensions must be omitted");
}
```

- [x] DONE 2026-08-20 **Step 2: Run to verify it fails**

Run: `cd litegen-core && cargo test --test unit_tests model3d_ 2>&1 | tail -20`
Expected: FAIL — `cannot find type Model3dGenerationRequest`.

- [x] DONE 2026-08-20 **Step 3: Add the types**

Insert into `litegen-core/src/types/mod.rs` immediately after the `VideoGenerationResponse` block (before `// ─── Shared Types ───`):

```rust
// ─── 3D Model Generation ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Model3dGenerationRequest {
    #[serde(flatten)]
    pub base: BaseGenerationRequest,
    /// Desired mesh container: "glb" | "obj" | "fbx" | "usdz". Default "glb".
    #[serde(default)] pub output_format: Option<String>,
    /// Generate textures (vs. bare geometry).
    #[serde(default)] pub texture: Option<bool>,
    /// PBR materials, where the model supports them.
    #[serde(default)] pub pbr: Option<bool>,
    #[serde(default)] pub target_polycount: Option<u32>,
    /// "off" | "auto" | "on".
    #[serde(default)] pub symmetry: Option<String>,
    /// "triangle" | "quad".
    #[serde(default)] pub topology: Option<String>,
    /// Auto-rig / emit a skeleton, where supported.
    #[serde(default)] pub rig: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Model3dAssetKind {
    Mesh,
    Texture,
    Preview,
}

/// One file produced by a 3D generation. A completed generation always carries
/// exactly one `Mesh` asset; consumers treat its absence as a provider failure.
/// `url` is always absolute — a root-relative path is indistinguishable from an
/// outage to a client fetching it from a worker.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Model3dAsset {
    pub kind: Model3dAssetKind,
    pub url: String,
    /// "glb" | "obj" | "fbx" | "usdz" | "png" | "jpg".
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Meshes only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polycount: Option<u32>,
    /// Preview / texture assets only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// 3D generation is always async — this is both the submit response and the
/// poll response, mirroring `VideoGenerationResponse` with `video_url` replaced
/// by the richer `assets` list.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Model3dGenerationResponse {
    pub id: String,
    pub status: GenerationStatus,
    pub model: String,
    pub provider: String,
    /// Populated on `completed`. Omitted entirely while the job is in flight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<Model3dAsset>,
    /// Unified 0–100 progress.
    pub progress: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageInfo>,
    pub created: i64,
}

impl Model3dGenerationResponse {
    /// The single required mesh asset, if present.
    pub fn mesh(&self) -> Option<&Model3dAsset> {
        self.assets.iter().find(|a| a.kind == Model3dAssetKind::Mesh)
    }
}
```

- [x] DONE 2026-08-20 **Step 4: Register the schemas with utoipa**

In `litegen-core/src/api/openapi.rs`, inside `components(schemas(`, after `crate::types::VideoGenerationResponse,`:

```rust
        crate::types::Model3dGenerationRequest,
        crate::types::Model3dGenerationResponse,
        crate::types::Model3dAsset,
        crate::types::Model3dAssetKind,
```

- [x] DONE 2026-08-20 **Step 5: Run to verify it passes**

Run: `cd litegen-core && cargo test --test unit_tests model3d 2>&1 | tail -20`
Expected: PASS (7 tests total across Tasks 1–2).

- [x] DONE 2026-08-20 **Step 6: Commit**

```bash
git add litegen-core/src/types/mod.rs litegen-core/src/api/openapi.rs litegen-core/tests/unit_tests.rs
git commit -m "feat(3d): Model3dAsset, request, and response types"
git push origin master
```

---

### Task 3: `Model3dProvider` trait, extras, handle, and poll result

**Status:** ✅ DONE (2026-08-20, 65e4c0d) — task review clean

**Files:**
- Modify: `litegen-core/src/providers/mod.rs` (after `VideoGenerationPollResult`, ~line 340)
- Create: `litegen-core/src/providers/model3d/mod.rs`
- Modify: `litegen-core/src/providers/mod.rs` (add `pub mod model3d;` next to `pub mod video;`)
- Test: covered by Task 5 (the mock is the first implementor); this task only has to compile.

**Interfaces:**
- Consumes: `types::{Model3dGenerationRequest, Model3dAsset, GenerationStatus}`, `capabilities::ModelSchema`, `proxy::materializer::MaterializedRequest`, `ProviderInstanceConfig`, `ProviderError`, `CostEstimate`, `HealthCheckResult`.
- Produces: `providers::{Model3dProvider, Model3dExtras, Model3dGenerationHandle, Model3dGenerationPollResult, Model3dFile}`.

- [x] DONE 2026-08-20 **Step 1: Add the trait and its data types**

Append to `litegen-core/src/providers/mod.rs`, immediately after the `VideoGenerationPollResult` struct:

```rust
/// Extra 3D-specific parameters extracted from a validated request.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Model3dExtras {
    pub output_format: Option<String>,
    pub texture: Option<bool>,
    pub pbr: Option<bool>,
    pub target_polycount: Option<u32>,
    pub symmetry: Option<String>,
    pub topology: Option<String>,
    pub rig: Option<bool>,
    pub extra: Option<serde_json::Value>,
}

/// One file a provider produced, already downloaded. litegen re-hosts every one
/// of these on its own storage, so providers hand back BYTES, not vendor URLs —
/// several vendors expire their download URLs within minutes of task success.
#[derive(Debug, Clone)]
pub struct Model3dFile {
    pub kind: crate::types::Model3dAssetKind,
    /// Container/extension without the dot: "glb", "png", ….
    pub format: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    /// Meshes only; `None` when the provider does not report it.
    pub polycount: Option<u32>,
    /// Preview/texture assets only.
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Handle returned when a 3D generation is started (for polling).
#[derive(Debug, Clone)]
pub struct Model3dGenerationHandle {
    pub provider_job_id: String,
    pub provider: String,
    pub model: String,
    /// Opaque per-provider continuation state for vendors whose flow has an
    /// intermediate stage (e.g. Meshy's preview→refine text-to-3D pipeline).
    /// Only that provider's own `poll_status` ever reads it; the generic poller,
    /// the DB row, and the API contract all stay single-stage.
    pub stage_context: Option<serde_json::Value>,
}

/// Status from polling a 3D generation. On `Completed`, `files` MUST contain
/// exactly one `Mesh` entry — a completed generation without a mesh is a
/// contract violation, not a degraded success.
#[derive(Debug, Clone)]
pub struct Model3dGenerationPollResult {
    pub status: crate::types::GenerationStatus,
    pub progress: u8,
    pub files: Vec<Model3dFile>,
    pub error: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// The core trait for 3D model (mesh) generation providers.
///
/// Async submit + poll, mirroring [`VideoProvider`], with one deliberate
/// divergence: `poll_status` returns downloaded bytes rather than provider URLs,
/// because litegen re-hosts every 3D output so that per-app BYO buckets work
/// identically across images, video, and meshes.
#[async_trait]
pub trait Model3dProvider: Send + Sync {
    fn name(&self) -> &str;
    fn configure(&mut self, config: ProviderInstanceConfig);
    fn is_configured(&self) -> bool;

    /// Start a 3D generation. Mode is inferred from `materialized.refs`: no refs
    /// → text-to-3D, an `init` ref → image-to-3D, `view-*` refs → multiview.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        materialized: &MaterializedRequest,
    ) -> Result<Model3dGenerationHandle, ProviderError>;

    /// Poll an in-flight generation. On `Completed`, every returned file must
    /// already be downloaded — the caller re-hosts within the same tick.
    async fn poll_status(
        &self,
        handle: &Model3dGenerationHandle,
    ) -> Result<Model3dGenerationPollResult, ProviderError>;

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        request: &Model3dGenerationRequest,
    ) -> Result<CostEstimate, ProviderError>;

    async fn health_check(&self) -> HealthCheckResult;
}
```

- [x] DONE 2026-08-20 **Step 2: Create the module root**

`litegen-core/src/providers/model3d/mod.rs`:

```rust
//! 3D model (mesh) generation providers.
//!
//! Every provider here is async submit + poll, and returns downloaded bytes so
//! the caller can re-host them on litegen storage (see `Model3dProvider`).

pub mod glb;
pub mod mock;
```

Add `pub mod model3d;` to `litegen-core/src/providers/mod.rs` alongside `pub mod video;`.

- [x] DONE 2026-08-20 **Step 3: Verify it compiles**

Because `glb` and `mock` do not exist yet, create them as empty placeholders for this step only:

```bash
cd litegen-core && : > src/providers/model3d/glb.rs && : > src/providers/model3d/mock.rs
cargo build 2>&1 | tail -20
```
Expected: builds clean (warnings about unused types are fine).

- [x] DONE 2026-08-20 **Step 4: Commit**

```bash
git add litegen-core/src/providers/mod.rs litegen-core/src/providers/model3d/
git commit -m "feat(3d): Model3dProvider trait, extras, handle, and poll result"
git push origin master
```

---

### Task 4: `glb.rs` — a real glTF 2.0 binary writer

**Status:** ✅ DONE (2026-08-27, 80099ce + ad56a1f) — task review clean

**Files:**
- Create: `litegen-core/src/providers/model3d/glb.rs` (replacing the Task 3 placeholder)

**Interfaces:**
- Consumes: nothing (pure function, no deps beyond `std`).
- Produces: `pub fn generate_cube_glb(prompt: &str) -> Vec<u8>`, `pub const CUBE_TRIANGLE_COUNT: u32 = 12`.

**Why it must be real bytes:** aipix's contract §6 calls out that the video mock's `https://example.com/...` placeholder is non-downloadable and forced a bundled fallback on their side. A 3D mock that returns a URL nothing can load repeats that mistake, and the acceptance checklist requires the mesh to load in three.js `GLTFLoader`.

- [x] DONE 2026-08-27 **Step 1: Write the failing test**

Create `litegen-core/src/providers/model3d/glb.rs` containing only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn u32_at(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }

    #[test]
    fn emits_a_well_formed_glb_container() {
        let glb = generate_cube_glb("a low-poly fox");

        // 12-byte header: magic "glTF", version 2, total length == buffer len.
        assert_eq!(&glb[0..4], b"glTF", "GLB magic");
        assert_eq!(u32_at(&glb, 4), 2, "glTF version");
        assert_eq!(u32_at(&glb, 8) as usize, glb.len(), "declared length matches actual");

        // Chunk 0 must be JSON, 4-byte aligned.
        let json_len = u32_at(&glb, 12) as usize;
        assert_eq!(&glb[16..20], b"JSON", "first chunk type");
        assert_eq!(json_len % 4, 0, "JSON chunk must be 4-byte aligned");

        // Chunk 1 must be BIN\0, 4-byte aligned.
        let bin_off = 20 + json_len;
        let bin_len = u32_at(&glb, bin_off) as usize;
        assert_eq!(&glb[bin_off + 4..bin_off + 8], b"BIN\0", "second chunk type");
        assert_eq!(bin_len % 4, 0, "BIN chunk must be 4-byte aligned");
        assert_eq!(bin_off + 8 + bin_len, glb.len(), "no trailing bytes");
    }

    #[test]
    fn json_chunk_describes_one_indexed_mesh() {
        let glb = generate_cube_glb("cube");
        let json_len = u32_at(&glb, 12) as usize;
        let json: serde_json::Value =
            serde_json::from_slice(&glb[20..20 + json_len]).expect("JSON chunk parses");

        assert_eq!(json["asset"]["version"], "2.0");
        assert_eq!(json["scenes"].as_array().unwrap().len(), 1);
        assert_eq!(json["meshes"].as_array().unwrap().len(), 1);
        // GLB stores its buffer in the BIN chunk, so buffers[0] has no uri.
        assert!(json["buffers"][0].get("uri").is_none(), "GLB buffer must be chunk-backed");
        let prim = &json["meshes"][0]["primitives"][0];
        assert!(prim["attributes"]["POSITION"].is_number());
        assert!(prim["indices"].is_number(), "mesh must be indexed");
        assert!(prim["material"].is_number());
    }

    #[test]
    fn distinct_prompts_produce_distinct_bytes() {
        // Parity with the video mock's "keyframe blend must differ from the
        // prompt-only fallback" assertion: the mock must prove the prompt
        // actually reached the generator.
        let a = generate_cube_glb("prompt one");
        let b = generate_cube_glb("prompt two");
        assert_ne!(a, b, "prompt must vary the output");
        assert_eq!(a.len(), b.len(), "variation is in values, not structure");
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(generate_cube_glb("same"), generate_cube_glb("same"));
    }

    #[test]
    fn triangle_count_matches_the_declared_constant() {
        let glb = generate_cube_glb("cube");
        let json_len = u32_at(&glb, 12) as usize;
        let json: serde_json::Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        let idx_accessor = json["meshes"][0]["primitives"][0]["indices"].as_u64().unwrap() as usize;
        let count = json["accessors"][idx_accessor]["count"].as_u64().unwrap() as u32;
        assert_eq!(count, CUBE_TRIANGLE_COUNT * 3);
    }
}
```

- [x] DONE 2026-08-27 **Step 2: Run to verify it fails**

Run: `cd litegen-core && cargo test glb:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function generate_cube_glb`.

- [x] DONE 2026-08-27 **Step 3: Write the implementation**

Prepend to `litegen-core/src/providers/model3d/glb.rs`:

```rust
//! Minimal glTF 2.0 binary (`.glb`) writer.
//!
//! Produces a real, loadable unit cube rather than a placeholder, so the mock
//! 3D provider hands back bytes `three.js`'s `GLTFLoader` actually accepts.
//! The prompt seeds the base colour and a small scale jitter, so two prompts
//! never yield identical bytes — that is what proves the prompt reached the
//! provider.
//!
//! @see <https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#binary-gltf-layout>

/// A cube is 12 triangles (2 per face).
pub const CUBE_TRIANGLE_COUNT: u32 = 12;

const POSITION_COUNT: usize = 8;
const INDEX_COUNT: usize = (CUBE_TRIANGLE_COUNT * 3) as usize;

/// FNV-1a — a stable, dependency-free hash so output is reproducible across
/// runs and platforms (`DefaultHasher` is explicitly not stable).
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Build a valid GLB containing a single indexed, materialised cube.
pub fn generate_cube_glb(prompt: &str) -> Vec<u8> {
    let h = fnv1a(prompt);

    // Prompt-seeded colour (kept away from 0.0/1.0 so it is visibly shaded) and
    // a ±10% scale jitter. Both live in the JSON/BIN payload, so distinct
    // prompts differ byte-wise at identical length.
    let r = 0.15 + ((h & 0xff) as f32 / 255.0) * 0.8;
    let g = 0.15 + (((h >> 8) & 0xff) as f32 / 255.0) * 0.8;
    let b = 0.15 + (((h >> 16) & 0xff) as f32 / 255.0) * 0.8;
    let scale = 0.45 + (((h >> 24) & 0xff) as f32 / 255.0) * 0.10;

    // ─── BIN chunk: 8 positions (f32x3) then 36 indices (u16) ───────────────
    let corners: [[f32; 3]; POSITION_COUNT] = [
        [-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0],
        [-1.0, -1.0,  1.0], [1.0, -1.0,  1.0], [1.0, 1.0,  1.0], [-1.0, 1.0,  1.0],
    ];
    let mut bin: Vec<u8> = Vec::with_capacity(POSITION_COUNT * 12 + INDEX_COUNT * 2);
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for c in corners {
        for axis in 0..3 {
            let v = c[axis] * scale;
            min[axis] = min[axis].min(v);
            max[axis] = max[axis].max(v);
            bin.extend_from_slice(&v.to_le_bytes());
        }
    }
    let positions_len = bin.len(); // 96

    #[rustfmt::skip]
    const INDICES: [u16; INDEX_COUNT] = [
        0, 1, 2,  0, 2, 3, // -Z
        4, 6, 5,  4, 7, 6, // +Z
        0, 4, 5,  0, 5, 1, // -Y
        3, 2, 6,  3, 6, 7, // +Y
        0, 3, 7,  0, 7, 4, // -X
        1, 5, 6,  1, 6, 2, // +X
    ];
    for i in INDICES {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    let indices_len = bin.len() - positions_len; // 72
    pad_to_4(&mut bin, 0x00);

    // ─── JSON chunk ─────────────────────────────────────────────────────────
    let doc = serde_json::json!({
        "asset": { "version": "2.0", "generator": "litegen mock 3d" },
        "scene": 0,
        "scenes": [ { "nodes": [0] } ],
        "nodes": [ { "mesh": 0, "name": "cube" } ],
        "meshes": [ {
            "name": "cube",
            "primitives": [ { "attributes": { "POSITION": 0 }, "indices": 1, "material": 0 } ]
        } ],
        "materials": [ {
            "name": "generated",
            "pbrMetallicRoughness": {
                "baseColorFactor": [r, g, b, 1.0],
                "metallicFactor": 0.1,
                "roughnessFactor": 0.8
            }
        } ],
        "accessors": [
            {
                "bufferView": 0, "componentType": 5126, "count": POSITION_COUNT,
                "type": "VEC3", "min": min, "max": max
            },
            {
                "bufferView": 1, "componentType": 5123, "count": INDEX_COUNT,
                "type": "SCALAR"
            }
        ],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": positions_len, "target": 34962 },
            { "buffer": 0, "byteOffset": positions_len, "byteLength": indices_len, "target": 34963 }
        ],
        "buffers": [ { "byteLength": bin.len() } ]
    });
    let mut json = serde_json::to_vec(&doc).expect("glb json is always serializable");
    pad_to_4(&mut json, b' '); // JSON chunks pad with spaces, BIN with zeros.

    // ─── Container ──────────────────────────────────────────────────────────
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

fn pad_to_4(buf: &mut Vec<u8>, filler: u8) {
    while buf.len() % 4 != 0 {
        buf.push(filler);
    }
}
```

- [x] DONE 2026-08-27 **Step 4: Run to verify it passes**

Run: `cd litegen-core && cargo test glb:: 2>&1 | tail -20`
Expected: PASS (5 tests).

- [x] DONE 2026-08-27 **Step 5: Verify the bytes load in a real glTF parser**

Run:
```bash
cd litegen-core && cargo test glb::tests::emits_a_well_formed_glb_container -- --nocapture
node -e '
const {execSync}=require("child_process");
' 2>/dev/null || true
```
Then confirm visually in Task 15's Playwright run, which loads the mock GLB through `<model-viewer>` (three.js `GLTFLoader` under the hood). Note in the commit message that byte-level validity is unit-tested and loader acceptance is covered by the Playwright spec.

- [x] DONE 2026-08-27 **Step 6: Commit**

```bash
git add litegen-core/src/providers/model3d/glb.rs
git commit -m "feat(3d): glTF 2.0 binary writer for the mock mesh"
git push origin master
```

---

### Task 5: Mock 3D provider + `models/mock.yaml` entries

**Status:** ✅ DONE (2026-08-27, b73e504) — task review clean

**Files:**
- Create: `litegen-core/src/providers/model3d/mock.rs` (replacing the Task 3 placeholder)
- Modify: `models/mock.yaml`

**Interfaces:**
- Consumes: `glb::generate_cube_glb`, `glb::CUBE_TRIANGLE_COUNT`, `Model3dProvider` and friends (Tasks 3–4), `providers::image::visual_mock::generate_visual_image_png` (existing).
- Produces: `MockModel3dProvider::new()`; model ids `mock/mesh-3d`, `mock/all-params-3d`, `mock/fail-3d`.

**Model id note:** `mock/mesh-3d` deliberately matches the id aipix already seeds locally (`apps/api/src/modules/seed/seed.service.ts`), so aipix's `mock/*` short-circuit can be deleted rather than re-pointed.

- [x] DONE 2026-08-27 **Step 1: Write the failing tests**

Create `litegen-core/src/providers/model3d/mock.rs` with only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{MediaType, ModelCapabilityFlags, ModelPricing, PromptSpec};
    use std::collections::HashMap;

    fn schema(id: &str) -> ModelSchema {
        ModelSchema {
            id: id.into(),
            provider: "mock".into(),
            media_type: MediaType::Model3d,
            display_name: id.into(),
            description: String::new(),
            pricing: ModelPricing { base_cost_usd: 0.0, variable_pricing: None },
            capabilities: ModelCapabilityFlags { text_to_3d: true, ..Default::default() },
            prompt: PromptSpec { required: true, min_length: None, max_length: None },
            params: HashMap::new(),
            ref_inputs: None,
            extra_allowlist: vec![],
            tags: vec![],
        }
    }

    fn base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.into(),
            model: model.into(),
            n: 1,
            negative_prompt: None,
            seed: None,
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn extras() -> Model3dExtras {
        Model3dExtras {
            output_format: None, texture: Some(true), pbr: None, target_polycount: None,
            symmetry: None, topology: None, rig: None, extra: None,
        }
    }

    fn provider() -> MockModel3dProvider {
        let mut p = MockModel3dProvider::new();
        p.configure(ProviderInstanceConfig::default());
        p
    }

    async fn poll_to_terminal(
        p: &MockModel3dProvider,
        h: &Model3dGenerationHandle,
    ) -> (Model3dGenerationPollResult, usize) {
        let mut polls = 0;
        loop {
            polls += 1;
            let r = p.poll_status(h).await.unwrap();
            if !matches!(r.status, GenerationStatus::Pending | GenerationStatus::Processing) {
                return (r, polls);
            }
            assert!(polls < 20, "mock must terminate quickly");
        }
    }

    #[tokio::test]
    async fn reports_monotonic_progress_across_several_polls() {
        // aipix's contract §6 asks the mock to actually exercise the polling
        // path, not snap straight to completed.
        let p = provider();
        let s = schema("mock/mesh-3d");
        let h = p.generate(&s, &base("a fox", "mock/mesh-3d"), &extras(), &MaterializedRequest::default())
            .await.unwrap();

        let mut seen = Vec::new();
        loop {
            let r = p.poll_status(&h).await.unwrap();
            seen.push(r.progress);
            if !matches!(r.status, GenerationStatus::Pending | GenerationStatus::Processing) { break; }
            assert!(seen.len() < 20);
        }
        assert!(seen.len() >= 3, "expected several polls, got {seen:?}");
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "progress must be monotonic: {seen:?}");
        assert_eq!(*seen.last().unwrap(), 100);
    }

    #[tokio::test]
    async fn completed_poll_carries_exactly_one_mesh_with_valid_glb_bytes() {
        let p = provider();
        let s = schema("mock/mesh-3d");
        let h = p.generate(&s, &base("a fox", "mock/mesh-3d"), &extras(), &MaterializedRequest::default())
            .await.unwrap();
        let (r, _) = poll_to_terminal(&p, &h).await;

        assert_eq!(r.status, GenerationStatus::Completed);
        let meshes: Vec<_> = r.files.iter().filter(|f| f.kind == Model3dAssetKind::Mesh).collect();
        assert_eq!(meshes.len(), 1, "exactly one mesh, always");
        let mesh = meshes[0];
        assert_eq!(&mesh.bytes[0..4], b"glTF", "mesh bytes must be a real GLB");
        assert_eq!(mesh.format, "glb");
        assert_eq!(mesh.content_type, "model/gltf-binary");
        assert_eq!(mesh.polycount, Some(crate::providers::model3d::glb::CUBE_TRIANGLE_COUNT));

        // A preview image rides along so the gallery has a 2D thumb.
        let preview = r.files.iter().find(|f| f.kind == Model3dAssetKind::Preview).unwrap();
        assert_eq!(&preview.bytes[0..4], b"\x89PNG");
        assert_eq!(preview.format, "png");
    }

    #[tokio::test]
    async fn distinct_prompts_yield_distinct_mesh_bytes() {
        let p = provider();
        let s = schema("mock/mesh-3d");
        let mut out = Vec::new();
        for prompt in ["prompt one", "prompt two"] {
            let h = p.generate(&s, &base(prompt, "mock/mesh-3d"), &extras(), &MaterializedRequest::default())
                .await.unwrap();
            let (r, _) = poll_to_terminal(&p, &h).await;
            out.push(r.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh).unwrap().bytes.clone());
        }
        assert_ne!(out[0], out[1], "the prompt must reach the generator");
    }

    #[tokio::test]
    async fn fail_model_terminates_failed_with_an_error_string() {
        // aipix needs a deterministic failure to test per-slot refunds.
        let p = provider();
        let s = schema("mock/fail-3d");
        let h = p.generate(&s, &base("anything", "mock/fail-3d"), &extras(), &MaterializedRequest::default())
            .await.unwrap();
        let (r, _) = poll_to_terminal(&p, &h).await;
        assert_eq!(r.status, GenerationStatus::Failed);
        assert!(r.error.as_deref().is_some_and(|e| !e.is_empty()));
        assert!(r.files.is_empty(), "a failed generation must not carry assets");
    }

    #[tokio::test]
    async fn unknown_job_id_is_a_non_retryable_error() {
        let p = provider();
        let h = Model3dGenerationHandle {
            provider_job_id: "does-not-exist".into(),
            provider: "mock".into(),
            model: "mock/mesh-3d".into(),
            stage_context: None,
        };
        let err = p.poll_status(&h).await.unwrap_err();
        assert!(!err.is_retryable(), "a purged job cannot be recovered by retrying");
    }
}
```

- [x] DONE 2026-08-27 **Step 2: Run to verify it fails**

Run: `cd litegen-core && cargo test model3d::mock 2>&1 | tail -20`
Expected: FAIL — `cannot find type MockModel3dProvider`.

- [x] DONE 2026-08-27 **Step 3: Write the implementation**

Prepend to `litegen-core/src/providers/model3d/mock.rs`:

```rust
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::capabilities::ModelSchema;
use crate::providers::model3d::glb::{generate_cube_glb, CUBE_TRIANGLE_COUNT};
use crate::providers::image::visual_mock::generate_visual_image_png;
use crate::proxy::materializer::MaterializedRequest;
use crate::providers::{
    build_cost_estimate, BaseGenerationRequest, HealthCheckResult, Model3dExtras, Model3dFile,
    Model3dGenerationHandle, Model3dGenerationPollResult, Model3dProvider, ProviderError,
    ProviderInstanceConfig,
};
use crate::types::*;

/// Polls before the job reports `completed`. Three is enough for a client to
/// observe a real pending → processing → completed transition without making
/// tests slow.
const POLLS_BEFORE_DONE: u32 = 3;

/// The model id that terminates in `failed`, for exercising refund paths.
const FAIL_MODEL: &str = "mock/fail-3d";

struct Job {
    model: String,
    mesh: Vec<u8>,
    preview: Vec<u8>,
    polls: AtomicU32,
}

/// Process-global job store. The mock has no backing service, so submitted work
/// lives here between `generate` and `poll_status` — the same technique
/// `video::visual_mock::global_store` uses for its GIF bytes.
fn jobs() -> &'static tokio::sync::RwLock<HashMap<String, Arc<Job>>> {
    static JOBS: std::sync::OnceLock<tokio::sync::RwLock<HashMap<String, Arc<Job>>>> =
        std::sync::OnceLock::new();
    JOBS.get_or_init(|| tokio::sync::RwLock::new(HashMap::new()))
}

/// Mock 3D provider for tests, CI, and the Playground.
///
/// No external API is called: `generate` renders a real glTF 2.0 cube locally
/// and `poll_status` walks a short progress ramp before handing back the bytes.
/// The only external contract honoured is the GLB byte format.
///
/// @see <https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html>
pub struct MockModel3dProvider {
    configured: bool,
}

impl MockModel3dProvider {
    pub fn new() -> Self {
        Self { configured: false }
    }
}

impl Default for MockModel3dProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Model3dProvider for MockModel3dProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn configure(&mut self, _config: ProviderInstanceConfig) {
        self.configured = true;
    }

    fn is_configured(&self) -> bool {
        self.configured
    }

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        _extras: &Model3dExtras,
        _materialized: &MaterializedRequest,
    ) -> Result<Model3dGenerationHandle, ProviderError> {
        let job_id = uuid::Uuid::new_v4().to_string();
        // Bytes are rendered at submit time so the poll ramp measures polling,
        // not generation, and so a prompt that reached us is provable later.
        let job = Arc::new(Job {
            model: model.id.clone(),
            mesh: generate_cube_glb(&base.prompt),
            preview: generate_visual_image_png(&base.prompt),
            polls: AtomicU32::new(0),
        });
        jobs().write().await.insert(job_id.clone(), job);
        Ok(Model3dGenerationHandle {
            provider_job_id: job_id,
            provider: "mock".to_string(),
            model: model.id.clone(),
            stage_context: None,
        })
    }

    async fn poll_status(
        &self,
        handle: &Model3dGenerationHandle,
    ) -> Result<Model3dGenerationPollResult, ProviderError> {
        let job = jobs()
            .read()
            .await
            .get(&handle.provider_job_id)
            .cloned()
            .ok_or_else(|| {
                // Not retryable: a job we have no record of will never appear.
                ProviderError::InvalidRequest(format!(
                    "unknown mock 3d job '{}'",
                    handle.provider_job_id
                ))
            })?;

        let n = job.polls.fetch_add(1, Ordering::SeqCst) + 1;
        let meta = HashMap::from([
            ("mock".to_string(), serde_json::json!(true)),
            ("job_id".to_string(), serde_json::json!(handle.provider_job_id)),
        ]);

        if n < POLLS_BEFORE_DONE {
            return Ok(Model3dGenerationPollResult {
                status: if n == 1 { GenerationStatus::Pending } else { GenerationStatus::Processing },
                progress: ((n * 100) / POLLS_BEFORE_DONE) as u8,
                files: Vec::new(),
                error: None,
                metadata: meta,
            });
        }

        jobs().write().await.remove(&handle.provider_job_id);

        if job.model == FAIL_MODEL {
            return Ok(Model3dGenerationPollResult {
                status: GenerationStatus::Failed,
                progress: 100,
                files: Vec::new(),
                error: Some("mock 3d generation failed (mock/fail-3d always fails)".into()),
                metadata: meta,
            });
        }

        Ok(Model3dGenerationPollResult {
            status: GenerationStatus::Completed,
            progress: 100,
            files: vec![
                Model3dFile {
                    kind: Model3dAssetKind::Mesh,
                    format: "glb".into(),
                    content_type: "model/gltf-binary".into(),
                    bytes: job.mesh.clone(),
                    polycount: Some(CUBE_TRIANGLE_COUNT),
                    width: None,
                    height: None,
                },
                Model3dFile {
                    kind: Model3dAssetKind::Preview,
                    format: "png".into(),
                    content_type: "image/png".into(),
                    bytes: job.preview.clone(),
                    polycount: None,
                    width: Some(512),
                    height: Some(512),
                },
            ],
            error: None,
            metadata: meta,
        })
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &Model3dGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
    }

    async fn health_check(&self) -> HealthCheckResult {
        HealthCheckResult {
            healthy: true,
            message: "Mock 3D provider always healthy".into(),
            latency_ms: None,
        }
    }
}
```

If `generate_visual_image_png` reports dimensions other than 512×512, correct the `width`/`height` above to match — read `src/providers/image/visual_mock.rs` rather than guessing.

If `MaterializedRequest` has no `Default` impl, add `#[derive(Default)]` to it in `proxy/materializer.rs` (all its fields are collections/options) rather than hand-building one in each test.

- [x] DONE 2026-08-27 **Step 4: Run to verify it passes**

Run: `cd litegen-core && cargo test model3d:: 2>&1 | tail -20`
Expected: PASS (5 mock tests + 5 glb tests).

- [x] DONE 2026-08-27 **Step 5: Add the mock model YAML entries**

Append to the `models:` list in `models/mock.yaml`:

```yaml
  # ─── 3D model (mesh) mocks ───────────────────────────────────────────────────

  - id: mock/mesh-3d
    provider: mock
    media_type: model3d
    display_name: Mock Mesh 3D
    description: Real glTF 2.0 binary cube, prompt-tinted. Reports progress across three polls so the async path is genuinely exercised.
    pricing: { base_cost_usd: 0.0 }
    capabilities: { text_to_3d: true, image_to_3d: true }
    prompt: { required: true, max_length: 4000 }
    params:
      output_format:
        kind: string
        enum_values: [glb]
        default: glb
        label: Output format
        description: Mesh container. The mock only emits glb.
      texture: { kind: bool, default: true, label: Textures }
      target_polycount:
        kind: int
        min: 100
        max: 300000
        label: Target polycount
        description: Approximate triangle budget.
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: base64 }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [mock, test, visual]

  - id: mock/all-params-3d
    provider: mock
    media_type: model3d
    display_name: Mock All-Params 3D
    description: Every 3D param kind exposed — output_format, texture, pbr, rig, symmetry, topology, target_polycount, seed. Use to test the full 3D validation surface.
    pricing: { base_cost_usd: 0.0 }
    capabilities: { text_to_3d: true, image_to_3d: true, multiview_to_3d: true }
    prompt: { required: true, max_length: 4000 }
    params:
      output_format:
        kind: string
        enum_values: [glb, obj, fbx, usdz]
        default: glb
        label: Output format
      texture: { kind: bool, default: true, label: Textures }
      pbr: { kind: bool, default: false, label: PBR materials }
      rig: { kind: bool, default: false, label: Auto-rig }
      symmetry:
        kind: string
        enum_values: [off, auto, on]
        default: auto
        label: Symmetry
      topology:
        kind: string
        enum_values: [triangle, quad]
        default: triangle
        label: Topology
      target_polycount: { kind: int, min: 100, max: 300000, label: Target polycount }
      seed: { kind: seed, min: 0, max: 2147483647 }
    ref_inputs:
      max_total: 6
      default_role: init
      provider_format: { form: base64 }
      roles:
        init:        { required: false, min_count: 0, max_count: 1 }
        view-front:  { required: false, min_count: 0, max_count: 1 }
        view-back:   { required: false, min_count: 0, max_count: 1 }
        view-left:   { required: false, min_count: 0, max_count: 1 }
        view-right:  { required: false, min_count: 0, max_count: 1 }
        view-top:    { required: false, min_count: 0, max_count: 1 }
    extra_allowlist: []
    tags: [mock, test]

  - id: mock/fail-3d
    provider: mock
    media_type: model3d
    display_name: Mock Failing 3D
    description: Always terminates in `failed` with an error string. Use to test refund / error-surface handling.
    pricing: { base_cost_usd: 0.0 }
    capabilities: { text_to_3d: true }
    prompt: { required: true, max_length: 1000 }
    params: {}
    extra_allowlist: []
    tags: [mock, test]
```

- [x] DONE 2026-08-27 **Step 6: Verify the YAML loads**

Run: `cd litegen-core && cargo test capabilities 2>&1 | tail -20`
Expected: PASS — the loader accepts every param key (they are in `KNOWN_PARAMS` from Task 1) and `media_type: model3d` deserializes.

If the loader rejects `label`/`description` inside a param, the `ParamSpec*` structs from Task 1 Step 4 are missing them — fix there, not here.

- [x] DONE 2026-08-27 **Step 7: Commit**

```bash
git add litegen-core/src/providers/model3d/mock.rs models/mock.yaml
git commit -m "feat(3d): mock provider with a real GLB, progress ramp, and a failure model"
git push origin master
```

---

### Task 6: Registry wiring

**Status:** ✅ DONE (2026-08-27, 331831f) — task review clean

**Files:**
- Modify: `litegen-core/src/proxy/registry.rs` — struct field, `register_provider`, `build_model3d_provider`, `MODEL3D_PROVIDERS`, `provider_catalog`, accessors, test helper
- Test: `litegen-core/src/proxy/registry.rs` (`mod tests`, extend `provider_catalog_covers_registry` + a new test)

**Interfaces:**
- Consumes: `Model3dProvider`, `MockModel3dProvider` (Tasks 3, 5).
- Produces: `ProviderRegistry::{model3d_provider_for, model3d_provider_for_request, get_model3d_provider, register_mock_model3d}`; `registry::MODEL3D_PROVIDERS`.

- [x] DONE 2026-08-27 **Step 1: Write the failing tests**

Append inside `mod tests` in `litegen-core/src/proxy/registry.rs`:

```rust
    #[test]
    fn model3d_providers_const_matches_the_build_arms() {
        // Same drift guard IMAGE_PROVIDERS/VIDEO_PROVIDERS get: every name in the
        // const must actually build, and nothing may build that isn't listed.
        let default = ProviderInstanceConfig::default();
        for name in MODEL3D_PROVIDERS {
            assert!(
                build_model3d_provider(name, &default).is_some(),
                "MODEL3D_PROVIDERS lists '{name}' but it does not build"
            );
        }
        assert!(build_model3d_provider("definitely-not-a-vendor", &default).is_none());
    }

    #[test]
    fn catalog_reports_the_model3d_modality() {
        let mock = provider_catalog().into_iter().find(|e| e.name == "mock").unwrap();
        assert!(
            mock.modalities.contains(&"model3d".to_string()),
            "mock serves 3D; the dashboard credential form reads modalities: {:?}",
            mock.modalities
        );
        // A vendor with no 3D implementation must not claim the modality.
        let stability = provider_catalog().into_iter().find(|e| e.name == "stability").unwrap();
        assert!(!stability.modalities.contains(&"model3d".to_string()));
    }

    #[tokio::test]
    async fn registers_and_resolves_a_model3d_provider() {
        let reg = ProviderRegistry::new();
        reg.register_provider("mock", ProviderInstanceConfig::default()).await;
        assert!(reg.model3d_provider_for("mock").await.is_some());
        assert!(reg.model3d_provider_for("stability").await.is_none());
    }
```

- [x] DONE 2026-08-27 **Step 2: Run to verify it fails**

Run: `cd litegen-core && cargo test proxy::registry 2>&1 | tail -20`
Expected: FAIL — `cannot find value MODEL3D_PROVIDERS`.

- [x] DONE 2026-08-27 **Step 3: Add the field, factory, and const**

In `litegen-core/src/proxy/registry.rs`:

Add the import next to the video ones:
```rust
use crate::providers::model3d::mock::MockModel3dProvider;
use crate::providers::Model3dProvider;
```

Add the field to `ProviderRegistry` (after `video_providers`):
```rust
    model3d_providers: RwLock<HashMap<String, Arc<dyn Model3dProvider>>>,
```
and initialise it in `ProviderRegistry::new()` the same way as the others.

Add the factory next to `build_video_provider`:
```rust
/// Pure factory: build the 3D-provider implementation for `name`. Returns
/// `None` for vendors without a 3D implementation (or an unknown name). Mirror
/// of [`build_image_provider`].
fn build_model3d_provider(
    name: &str,
    config: &ProviderInstanceConfig,
) -> Option<Box<dyn Model3dProvider>> {
    macro_rules! configured {
        ($ty:ty) => {{
            let mut p = <$ty>::new();
            p.configure(config.clone());
            Some(Box::new(p) as Box<dyn Model3dProvider>)
        }};
    }
    match name {
        "mock" => configured!(MockModel3dProvider),
        _ => None,
    }
}

/// Canonical list of providers with a 3D implementation. Mirrors
/// [`build_model3d_provider`]. Real vendors (Meshy, Tripo3D, Stability, Rodin)
/// land here in phase 2.
pub const MODEL3D_PROVIDERS: &[&str] = &["mock"];
```

- [x] DONE 2026-08-27 **Step 4: Wire `register_provider` and the accessors**

Extend `register_provider`:
```rust
    async fn register_provider(&self, name: &str, config: ProviderInstanceConfig) {
        let image = build_image_provider(name, &config);
        let video = build_video_provider(name, &config);
        let model3d = build_model3d_provider(name, &config);

        if image.is_none() && video.is_none() && model3d.is_none() {
            warn!(provider = %name, "Unknown provider, skipping");
            return;
        }

        let has_image = image.is_some();
        let has_video = video.is_some();
        let has_model3d = model3d.is_some();

        if let Some(ip) = image {
            self.image_providers.write().await.insert(name.to_string(), Arc::from(ip));
        }
        if let Some(vp) = video {
            self.video_providers.write().await.insert(name.to_string(), Arc::from(vp));
        }
        if let Some(mp) = model3d {
            self.model3d_providers.write().await.insert(name.to_string(), Arc::from(mp));
        }
        self.provider_configs.write().await.insert(name.to_string(), config);

        let mut kinds: Vec<&str> = Vec::new();
        if has_image { kinds.push("image"); }
        if has_video { kinds.push("video"); }
        if has_model3d { kinds.push("3d"); }
        info!(provider = %name, modalities = %kinds.join("+"), "Registered provider");
    }
```

Add the accessors next to their video equivalents:
```rust
    /// Get a 3D provider by name.
    pub async fn get_model3d_provider(&self, name: &str) -> Option<Arc<dyn Model3dProvider>> {
        self.model3d_providers.read().await.get(name).cloned()
    }

    /// Get a 3D provider by provider name (capability-registry routing).
    pub async fn model3d_provider_for(&self, name: &str) -> Option<Arc<dyn Model3dProvider>> {
        self.model3d_providers.read().await.get(name).cloned()
    }

    /// Build a per-request 3D provider for `name`. See
    /// [`image_provider_for_request`](Self::image_provider_for_request).
    pub async fn model3d_provider_for_request(
        &self,
        name: &str,
        app_creds: Option<ProviderCredentials>,
    ) -> Option<Arc<dyn Model3dProvider>> {
        match app_creds {
            None => self.model3d_provider_for(name).await,
            Some(creds) => {
                let base = self.provider_configs.read().await.get(name).cloned().unwrap_or_default();
                let cfg = base.with_credentials(creds);
                build_model3d_provider(name, &cfg).map(Arc::from)
            }
        }
    }

    /// Test helper: directly register a pre-built 3D provider.
    #[cfg(test)]
    pub async fn register_mock_model3d(&self, provider: Arc<dyn Model3dProvider>) {
        self.model3d_providers.write().await.insert("mock".to_string(), provider);
    }
```

- [x] DONE 2026-08-27 **Step 5: Extend the catalog**

In `provider_catalog()`, widen the name union and the modality list:
```rust
    let mut names: Vec<&str> = Vec::new();
    for n in IMAGE_PROVIDERS
        .iter()
        .chain(VIDEO_PROVIDERS.iter())
        .chain(MODEL3D_PROVIDERS.iter())
    {
        if !names.contains(n) {
            names.push(n);
        }
    }
```
and inside the `map`:
```rust
            if MODEL3D_PROVIDERS.contains(&name) {
                modalities.push("model3d".to_string());
            }
```
(placed after the `video` push, so ordering stays image → video → model3d).

Also extend the existing `provider_catalog_covers_registry` test's `builds` expression and its final drift loop:
```rust
            let builds = build_image_provider(&entry.name, &default).is_some()
                || build_video_provider(&entry.name, &default).is_some()
                || build_model3d_provider(&entry.name, &default).is_some();
```
```rust
        for name in IMAGE_PROVIDERS
            .iter()
            .chain(VIDEO_PROVIDERS.iter())
            .chain(MODEL3D_PROVIDERS.iter())
        {
            assert!(catalog.contains(*name), "registered provider '{name}' missing from catalog");
        }
```

- [x] DONE 2026-08-27 **Step 6: Run to verify it passes**

Run: `cd litegen-core && cargo test proxy::registry 2>&1 | tail -20`
Expected: PASS.

The pre-existing `assert_eq!(bedrock.modalities, vec!["image".to_string(), "video".to_string()])` still holds (bedrock has no 3D arm). If any catalog assertion now fails, the modality push order is wrong — fix the order, do not relax the assertion.

- [x] DONE 2026-08-27 **Step 7: Commit**

```bash
git add litegen-core/src/proxy/registry.rs
git commit -m "feat(3d): register model3d providers and report the modality in the catalog"
git push origin master
```

---

### Task 7: 3D asset storage — absolute URLs, explicit keys, local fallback

**Status:** ✅ DONE (2026-08-27, b4a44d2) — task review clean

**Files:**
- Modify: `litegen-core/src/proxy/storage.rs`
- Modify: `litegen-core/src/config/mod.rs` (`ServerConfig` gains `public_base_url`)
- Modify: `litegen-core/src/api/handlers/mod.rs` (asset-serving route + handler)
- Test: `litegen-core/src/proxy/storage.rs` (new `mod model3d_storage_tests`)

**Interfaces:**
- Consumes: `ImageStorage`, `S3Storage`, `ImageStorageConfig` (existing).
- Produces: `storage::{build_model3d_store, LocalModel3dStorage, model3d_asset_key, MODEL3D_PATH_PREFIX, local_asset_bytes}`; `ServerConfig::public_base_url`; route `GET /v1/models3d/assets/{*key}`.

**Why a new local store:** `LocalStorage::put` returns `local://{key}` and `S3Store::store` infers `.png` from a content-type switch. Neither can satisfy "every asset URL is absolute and downloads a `.glb`". `LocalModel3dStorage` keeps bytes in-process and returns `{public_base_url}/v1/models3d/assets/{key}`, which the new route serves.

- [x] DONE 2026-08-27 **Step 1: Write the failing tests**

Append to `litegen-core/src/proxy/storage.rs`:

```rust
#[cfg(test)]
mod model3d_storage_tests {
    use super::*;

    #[test]
    fn asset_key_takes_its_extension_from_the_caller_not_the_content_type() {
        // Regression guard for the S3Store bug this path deliberately avoids:
        // `S3Store::store` maps content_type → extension through a hardcoded
        // image switch and would save a mesh as ".png".
        let key = model3d_asset_key(None, "litegen-3d-abc", "model", "glb");
        assert_eq!(key, "litegen/3d/litegen-3d-abc/model.glb");
        assert!(key.ends_with(".glb"));

        let prefixed = model3d_asset_key(Some("tenant-7/meshes"), "litegen-3d-abc", "preview", "png");
        assert_eq!(prefixed, "tenant-7/meshes/litegen-3d-abc/preview.png");
    }

    #[test]
    fn asset_key_default_prefix_is_distinct_from_images() {
        assert_eq!(MODEL3D_PATH_PREFIX, "litegen/3d");
        assert_ne!(MODEL3D_PATH_PREFIX, "litegen/images");
    }

    #[tokio::test]
    async fn local_store_returns_an_absolute_url_and_serves_the_bytes_back() {
        // aipix fetches asset URLs from a worker: a root-relative path fails with
        // "invalid url: relative URL without a base" and is indistinguishable
        // from a provider outage.
        let store = LocalModel3dStorage::new("http://127.0.0.1:8080".into());
        let key = model3d_asset_key(None, "litegen-3d-local", "model", "glb");
        let url = store
            .put(&key, &bytes::Bytes::from_static(b"glTF\x02\x00\x00\x00"), "model/gltf-binary")
            .await
            .unwrap();

        assert_eq!(url, "http://127.0.0.1:8080/v1/models3d/assets/litegen/3d/litegen-3d-local/model.glb");
        assert!(url.starts_with("http://") || url.starts_with("https://"), "must be absolute");
        assert!(!url.starts_with("local://"));

        let (bytes, ct) = local_asset_bytes(&key).await.expect("bytes are retrievable");
        assert_eq!(&bytes[0..4], b"glTF");
        assert_eq!(ct, "model/gltf-binary");
    }

    #[tokio::test]
    async fn local_store_trims_a_trailing_slash_on_the_base_url() {
        let store = LocalModel3dStorage::new("https://litegen.example.com/".into());
        let url = store
            .put("litegen/3d/x/model.glb", &bytes::Bytes::from_static(b"x"), "model/gltf-binary")
            .await
            .unwrap();
        assert_eq!(url, "https://litegen.example.com/v1/models3d/assets/litegen/3d/x/model.glb");
    }

    #[test]
    fn build_model3d_store_falls_back_to_local_when_s3_is_unconfigured() {
        let cfg = crate::config::ImageStorageConfig { backend: "local".into(), path_prefix: None, s3: None };
        // Must not panic and must not be an S3 store; the smoke test is that a
        // put against it yields an absolute URL rather than an error.
        let store = build_model3d_store(&cfg, "http://localhost:8080");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let url = rt
            .block_on(store.put("litegen/3d/y/model.glb", &bytes::Bytes::from_static(b"y"), "model/gltf-binary"))
            .unwrap();
        assert!(url.starts_with("http://localhost:8080/"));
    }
}
```

- [x] DONE 2026-08-27 **Step 2: Run to verify it fails**

Run: `cd litegen-core && cargo test model3d_storage 2>&1 | tail -20`
Expected: FAIL — `cannot find function model3d_asset_key`.

- [x] DONE 2026-08-27 **Step 3: Implement the store**

Append to `litegen-core/src/proxy/storage.rs` (before the test module):

```rust
// ─── 3D asset storage ───────────────────────────────────────────────────────

/// Default key prefix for 3D outputs. Distinct from images' `litegen/images` so
/// a shared bucket keeps the two modalities apart.
pub const MODEL3D_PATH_PREFIX: &str = "litegen/3d";

/// Build the storage key for one file of a 3D generation.
///
/// The extension comes from the CALLER, never from a content-type switch — that
/// is the whole reason this path uses `ImageStorage::put` (explicit key) rather
/// than `ImageStore::store`, whose S3 impl would save a `.glb` as `.png`.
pub fn model3d_asset_key(path_prefix: Option<&str>, generation_id: &str, name: &str, ext: &str) -> String {
    let prefix = path_prefix.unwrap_or(MODEL3D_PATH_PREFIX).trim_matches('/');
    format!("{prefix}/{generation_id}/{name}.{ext}")
}

/// Process-global byte store backing [`LocalModel3dStorage`].
type LocalAssets = tokio::sync::RwLock<std::collections::HashMap<String, (bytes::Bytes, String)>>;
fn local_assets() -> &'static LocalAssets {
    static ASSETS: std::sync::OnceLock<LocalAssets> = std::sync::OnceLock::new();
    ASSETS.get_or_init(|| tokio::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Fetch bytes previously stored by [`LocalModel3dStorage`]. Used by the
/// asset-serving route.
pub async fn local_asset_bytes(key: &str) -> Option<(bytes::Bytes, String)> {
    local_assets().read().await.get(key).cloned()
}

/// Local (no-S3) 3D asset store.
///
/// Unlike [`LocalStorage`], which returns an unusable `local://` URL, this keeps
/// the bytes in-process and hands back an ABSOLUTE URL served by
/// `GET /v1/models3d/assets/{key}`. Clients fetch mesh URLs from their own
/// workers, where a relative path is indistinguishable from an outage.
pub struct LocalModel3dStorage {
    public_base_url: String,
}

impl LocalModel3dStorage {
    pub fn new(public_base_url: String) -> Self {
        Self { public_base_url: public_base_url.trim_end_matches('/').to_string() }
    }
}

#[async_trait::async_trait]
impl ImageStorage for LocalModel3dStorage {
    async fn put(&self, key: &str, bytes: &bytes::Bytes, content_type: &str) -> Result<String, ImageStoreError> {
        local_assets().write().await.insert(key.to_string(), (bytes.clone(), content_type.to_string()));
        Ok(format!("{}/v1/models3d/assets/{}", self.public_base_url, key))
    }
    async fn delete(&self, key: &str) -> Result<(), ImageStoreError> {
        local_assets().write().await.remove(key);
        Ok(())
    }
}

/// Build the 3D asset store from configuration.
///
/// Mirrors [`build_image_store`] but returns the key-explicit [`ImageStorage`]
/// trait, and falls back to [`LocalModel3dStorage`] (absolute URLs) rather than
/// a no-op store, because a 3D generation with no reachable mesh URL is a
/// failed generation, not a degraded one.
pub fn build_model3d_store(config: &ImageStorageConfig, public_base_url: &str) -> Arc<dyn ImageStorage> {
    match config.backend.as_str() {
        "s3" => match S3Storage::from_config(config) {
            Ok(store) => {
                info!("3D asset storage: S3 bucket configured");
                Arc::new(store)
            }
            Err(e) => {
                error!(error = %e, "Failed to configure S3 3D storage, falling back to local");
                Arc::new(LocalModel3dStorage::new(public_base_url.to_string()))
            }
        },
        _ => {
            info!("3D asset storage: local (served from this process)");
            Arc::new(LocalModel3dStorage::new(public_base_url.to_string()))
        }
    }
}
```

- [x] DONE 2026-08-27 **Step 4: Add `public_base_url` to `ServerConfig`**

In `litegen-core/src/config/mod.rs`:

```rust
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_seconds: u64,
    /// Externally reachable origin for URLs litegen mints and hands to clients
    /// (currently 3D asset URLs, which MUST be absolute). Set this behind a
    /// proxy — e.g. `https://app.litegen.ai/api`. Defaults to
    /// `http://{host}:{port}`, which is correct for local dev and CI.
    #[serde(default)]
    pub public_base_url: Option<String>,
}
```
Add `public_base_url: None` to its `Default` impl, and a resolver next to it:
```rust
impl ServerConfig {
    /// The absolute origin to mint client-facing URLs from.
    pub fn public_base_url(&self) -> String {
        self.public_base_url
            .clone()
            .unwrap_or_else(|| format!("http://{}:{}", self.host, self.port))
            .trim_end_matches('/')
            .to_string()
    }
}
```
If `AppConfig` has a manual `Serialize` impl (it does — see `mod.rs:579`), add `public_base_url` to the `ServerConfig` serialization there too if `ServerConfig` is hand-serialized; if only `AppConfig` is, no change is needed. Check before editing.

Add the key to `litegen.example.yaml` under `server:` with a comment.

- [x] DONE 2026-08-27 **Step 5: Add the asset-serving route**

In `litegen-core/src/api/handlers/mod.rs`, next to `get_mock_video_bytes`:

```rust
// ─── 3D asset bytes route ───────────────────────────────────────────────────

/// GET /v1/models3d/assets/{key} — Serve a 3D asset held by the local
/// (no-S3) store. No auth: these are the same unguessable, UUID-keyed URLs
/// handed to clients in the response, and S3 deployments never reach here.
pub async fn get_model3d_asset_bytes(Path(key): Path<String>) -> impl IntoResponse {
    match crate::proxy::storage::local_asset_bytes(&key).await {
        Some((bytes, content_type)) => (
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, content_type),
                // Meshes are immutable once written and keyed by generation id.
                (axum::http::header::CACHE_CONTROL, "public, max-age=31536000, immutable".to_string()),
            ],
            bytes,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
```

Register it in the route table next to `/mock/video/{id}` — note the wildcard capture, since keys contain slashes:
```rust
        .route("/v1/models3d/assets/{*key}", get(get_model3d_asset_bytes))
```

- [x] DONE 2026-08-27 **Step 6: Run to verify it passes**

Run: `cd litegen-core && cargo test model3d_storage 2>&1 | tail -20`
Expected: PASS (5 tests).

Run: `cd litegen-core && cargo build 2>&1 | tail -5`
Expected: clean.

- [x] DONE 2026-08-27 **Step 7: Commit**

```bash
git add litegen-core/src/proxy/storage.rs litegen-core/src/config/mod.rs \
        litegen-core/src/api/handlers/mod.rs litegen.example.yaml
git commit -m "feat(3d): key-explicit asset storage with absolute URLs and a local fallback"
git push origin master
```

---

### Task 8: `validate_model3d` + the `ValidatedModel3d` extractor

**Status:** ✅ DONE (2026-08-27, 9cecdaa) — task review clean

**Files:**
- Modify: `litegen-core/src/api/middleware/validator.rs`
- Test: `litegen-core/tests/unit_tests.rs` (a new `mod model3d_validation`)

**Interfaces:**
- Consumes: `Model3dGenerationRequest` (Task 2), `ModelSchema`, `ParamSpec`, existing `check_prompt`/`check_string`/`check_refs`/`check_extra`.
- Produces: `validator::{Model3dValidationOutput, validate_model3d, ValidatedModel3d}`.

**The behaviour that matters most:** `strict: false` drops an unsupported param and records it in `dropped` (→ the `X-Litegen-Dropped-Params` header); `strict: true` returns `param_unsupported`. aipix relies on this so it does not duplicate litegen's per-model capability table.

- [x] DONE 2026-08-27 **Step 1: Write the failing tests**

Create `litegen-core/tests/model3d_validation.rs`:

```rust
//! Validation surface for the `model3d` family. The lax-drop behaviour here is
//! the single most load-bearing part of the contract aipix codes against: it is
//! what lets a client send a superset of params without knowing which model
//! supports which knob.

use litegen::api::middleware::validator::validate_model3d;
use litegen::capabilities::*;
use litegen::types::*;
use std::collections::HashMap;

fn schema_with(params: HashMap<String, ParamSpec>) -> ModelSchema {
    ModelSchema {
        id: "mock/mesh-3d".into(),
        provider: "mock".into(),
        media_type: MediaType::Model3d,
        display_name: "Mock Mesh 3D".into(),
        description: String::new(),
        pricing: ModelPricing { base_cost_usd: 0.0, variable_pricing: None },
        capabilities: ModelCapabilityFlags { text_to_3d: true, ..Default::default() },
        prompt: PromptSpec { required: true, min_length: None, max_length: None },
        params,
        ref_inputs: None,
        extra_allowlist: vec![],
        tags: vec![],
    }
}

fn req(strict: bool) -> Model3dGenerationRequest {
    Model3dGenerationRequest {
        base: BaseGenerationRequest {
            prompt: "a low-poly fox".into(),
            model: "mock/mesh-3d".into(),
            n: 1,
            negative_prompt: None,
            seed: None,
            reference_images: vec![],
            strict,
            extra: None,
            metadata: None,
        },
        output_format: None, texture: None, pbr: None, target_polycount: None,
        symmetry: None, topology: None, rig: None,
    }
}

fn int_param(min: i64, max: i64) -> ParamSpec {
    ParamSpec::Int(ParamSpecInt { min: Some(min), max: Some(max), default: None, label: None, description: None })
}

fn str_param(values: &[&str]) -> ParamSpec {
    ParamSpec::String(ParamSpecString {
        max_length: None,
        enum_values: values.iter().map(|s| s.to_string()).collect(),
        pattern: None,
        default: None,
        label: None,
        description: None,
    })
}

fn bool_param() -> ParamSpec {
    ParamSpec::Bool(ParamSpecBool { default: None, label: None, description: None })
}

#[test]
fn lax_mode_drops_unsupported_params_instead_of_erroring() {
    let schema = schema_with(HashMap::new()); // model supports nothing
    let mut r = req(false);
    r.target_polycount = Some(5000);
    r.pbr = Some(true);
    r.rig = Some(true);
    r.topology = Some("quad".into());

    let out = validate_model3d(&schema, r).expect("lax must never error on unsupported params");
    assert_eq!(out.request.target_polycount, None, "dropped params are cleared from the request");
    assert_eq!(out.request.pbr, None);
    assert_eq!(out.request.rig, None);
    assert_eq!(out.request.topology, None);
    for p in ["target_polycount", "pbr", "rig", "topology"] {
        assert!(out.dropped.contains(&p.to_string()), "'{p}' must be reported as dropped: {:?}", out.dropped);
    }
}

#[test]
fn strict_mode_rejects_an_unsupported_param() {
    let schema = schema_with(HashMap::new());
    let mut r = req(true);
    r.target_polycount = Some(5000);
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_unsupported");
    assert_eq!(err.param.as_deref(), Some("target_polycount"));
}

#[test]
fn supported_params_pass_through_untouched() {
    let schema = schema_with(HashMap::from([
        ("target_polycount".to_string(), int_param(100, 300_000)),
        ("topology".to_string(), str_param(&["triangle", "quad"])),
        ("output_format".to_string(), str_param(&["glb", "obj"])),
        ("texture".to_string(), bool_param()),
        ("pbr".to_string(), bool_param()),
        ("rig".to_string(), bool_param()),
        ("symmetry".to_string(), str_param(&["off", "auto", "on"])),
    ]));
    let mut r = req(true);
    r.target_polycount = Some(5000);
    r.topology = Some("quad".into());
    r.output_format = Some("obj".into());
    r.texture = Some(true);
    r.pbr = Some(false);
    r.rig = Some(true);
    r.symmetry = Some("auto".into());

    let out = validate_model3d(&schema, r).unwrap();
    assert!(out.dropped.is_empty(), "nothing should drop: {:?}", out.dropped);
    assert_eq!(out.request.target_polycount, Some(5000));
    assert_eq!(out.request.topology.as_deref(), Some("quad"));
    assert_eq!(out.request.symmetry.as_deref(), Some("auto"));
}

#[test]
fn out_of_range_polycount_is_rejected_even_in_lax_mode() {
    // A declared param with a bad VALUE is a client error in both modes — only
    // UNSUPPORTED params get the lax drop.
    let schema = schema_with(HashMap::from([
        ("target_polycount".to_string(), int_param(100, 300_000)),
    ]));
    let mut r = req(false);
    r.target_polycount = Some(9_000_000);
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_out_of_range");
    assert_eq!(err.param.as_deref(), Some("target_polycount"));
}

#[test]
fn enum_mismatch_on_topology_is_rejected() {
    let schema = schema_with(HashMap::from([
        ("topology".to_string(), str_param(&["triangle", "quad"])),
    ]));
    let mut r = req(true);
    r.topology = Some("voxel".into());
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_enum_mismatch");
}

#[test]
fn multiview_roles_are_accepted_when_the_model_declares_them() {
    // Mode is inferred, not passed: `view-*` refs mean multiview-to-3D.
    let mut schema = schema_with(HashMap::new());
    schema.ref_inputs = Some(RefInputSpec {
        max_total: 4,
        default_role: Some("init".into()),
        provider_format: RefProviderFormat::Base64,
        roles: HashMap::from([
            ("init".to_string(), RefRoleSpec { required: false, min_count: 0, max_count: 1 }),
            ("view-front".to_string(), RefRoleSpec { required: false, min_count: 0, max_count: 1 }),
            ("view-back".to_string(), RefRoleSpec { required: false, min_count: 0, max_count: 1 }),
        ]),
    });
    let mut r = req(true);
    r.base.reference_images = vec![
        ReferenceImage { kind: RefImageKind::Url, value: "https://x/f.png".into(), role: Some("view-front".into()) },
        ReferenceImage { kind: RefImageKind::Url, value: "https://x/b.png".into(), role: Some("view-back".into()) },
    ];
    let out = validate_model3d(&schema, r).unwrap();
    assert_eq!(out.request.base.reference_images.len(), 2);
    assert!(out.dropped.is_empty());
}

#[test]
fn prompt_is_still_required() {
    let schema = schema_with(HashMap::new());
    let mut r = req(true);
    r.base.prompt = "   ".into();
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "prompt_required");
}
```

If `validator` is not currently `pub` from the crate root, add `pub use` as needed in `src/api/middleware/mod.rs` (`pub mod validator;`) rather than moving the tests inline — the module is already referenced as `crate::api::middleware::validator` elsewhere.

- [x] DONE 2026-08-27 **Step 2: Run to verify it fails**

Run: `cd litegen-core && cargo test --test model3d_validation 2>&1 | tail -20`
Expected: FAIL — `cannot find function validate_model3d`.

- [x] DONE 2026-08-27 **Step 3: Implement `validate_model3d`**

Add to `litegen-core/src/api/middleware/validator.rs`, after `validate_video`:

```rust
#[derive(Debug)]
pub struct Model3dValidationOutput {
    pub request: Model3dGenerationRequest,
    pub dropped: Vec<String>,
}

#[tracing::instrument(skip(schema, req), fields(model = %schema.id))]
pub fn validate_model3d(
    schema: &ModelSchema,
    mut req: Model3dGenerationRequest,
) -> Result<Model3dValidationOutput, ValidationError> {
    let mut dropped = Vec::new();
    let strict = req.base.strict;

    check_prompt(&schema.prompt, &req.base.prompt)?;

    // seed / negative_prompt — identical treatment to image and video.
    if req.base.seed.is_some() {
        match schema.params.get("seed") {
            Some(ParamSpec::Seed(s)) => {
                let v = req.base.seed.unwrap();
                if v < s.min || v > s.max {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("seed {} outside [{}, {}]", v, s.min, s.max),
                        Some("seed"),
                    ));
                }
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("seed not supported by '{}'", schema.id),
                        Some("seed"),
                    ));
                }
                dropped.push("seed".into());
                req.base.seed = None;
            }
        }
    }

    if req.base.negative_prompt.is_some() {
        match schema.params.get("negative_prompt") {
            Some(ParamSpec::String(s)) => {
                let v = req.base.negative_prompt.as_deref().unwrap();
                check_string(v, &s.enum_values, s.max_length, "negative_prompt")?;
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("negative_prompt not supported by '{}'", schema.id),
                        Some("negative_prompt"),
                    ));
                }
                dropped.push("negative_prompt".into());
                req.base.negative_prompt = None;
            }
        }
    }

    // ─── 3D params ──────────────────────────────────────────────────────────
    //
    // Each follows the same shape: a declared spec validates the VALUE (errors
    // in both modes); an absent or mismatched spec means UNSUPPORTED, which
    // errors in strict mode and drops in lax mode. That split is what lets a
    // client send a superset of params without a capability table of its own.

    macro_rules! string_param {
        ($field:expr, $key:literal) => {
            if $field.is_some() {
                match schema.params.get($key) {
                    Some(ParamSpec::String(s)) => {
                        let v = $field.as_deref().unwrap();
                        check_string(v, &s.enum_values, s.max_length, $key)?;
                    }
                    Some(_) | None => {
                        if strict {
                            return Err(ValidationError::new(
                                "param_unsupported",
                                format!("{} not supported by '{}'", $key, schema.id),
                                Some($key),
                            ));
                        }
                        dropped.push($key.into());
                        $field = None;
                    }
                }
            }
        };
    }

    macro_rules! bool_param {
        ($field:expr, $key:literal) => {
            if $field.is_some() && !matches!(schema.params.get($key), Some(ParamSpec::Bool(_))) {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("{} not supported by '{}'", $key, schema.id),
                        Some($key),
                    ));
                }
                dropped.push($key.into());
                $field = None;
            }
        };
    }

    string_param!(req.output_format, "output_format");
    string_param!(req.symmetry, "symmetry");
    string_param!(req.topology, "topology");
    bool_param!(req.texture, "texture");
    bool_param!(req.pbr, "pbr");
    bool_param!(req.rig, "rig");

    if req.target_polycount.is_some() {
        match schema.params.get("target_polycount") {
            Some(ParamSpec::Int(s)) => {
                let v = req.target_polycount.unwrap() as i64;
                if s.min.map(|m| v < m).unwrap_or(false) || s.max.map(|m| v > m).unwrap_or(false) {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("target_polycount {} out of range", v),
                        Some("target_polycount"),
                    ));
                }
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("target_polycount not supported by '{}'", schema.id),
                        Some("target_polycount"),
                    ));
                }
                dropped.push("target_polycount".into());
                req.target_polycount = None;
            }
        }
    }

    check_refs(schema, &mut req.base.reference_images, strict, &mut dropped)?;
    check_extra(schema, &mut req.base.extra, strict, &mut dropped)?;

    Ok(Model3dValidationOutput { request: req, dropped })
}
```

- [x] DONE 2026-08-27 **Step 4: Add the extractor**

After `impl FromRequest<Arc<AppState>> for ValidatedVideo`:

```rust
pub struct ValidatedModel3d {
    pub schema: Arc<ModelSchema>,
    pub request: Model3dGenerationRequest,
    pub dropped: Vec<String>,
    pub ctx: MaterializeContext,
    pub _permit: tokio::sync::OwnedSemaphorePermit,
}

impl FromRequest<Arc<AppState>> for ValidatedModel3d {
    type Rejection = ValidationRejection;
    async fn from_request(req: Request, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        // Acquire the in-flight slot BEFORE buffering the body (see ValidatedImage).
        let permit = acquire_in_flight(state)?;
        let (parts, body) = req.into_parts();
        let headers = parts.headers;
        let bytes = axum::body::to_bytes(body, 25 * 1024 * 1024).await.map_err(|e| {
            ValidationRejection(StatusCode::PAYLOAD_TOO_LARGE, err_body("body_too_large", &e.to_string(), None, ""))
        })?;
        let (json, ctx) = parse_request_and_context(&headers, bytes).await?;
        let req: Model3dGenerationRequest = serde_json::from_value(json).map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
        })?;
        let schema = state.registry.get(&req.base.model)
            .ok_or_else(|| ValidationRejection(StatusCode::NOT_FOUND, err_body("model_not_found", &format!("model '{}' not found", req.base.model), None, &req.base.model)))?
            .clone();
        let schema = Arc::new(schema);
        match validate_model3d(&schema, req) {
            Ok(out) => Ok(ValidatedModel3d { schema, request: out.request, dropped: out.dropped, ctx, _permit: permit }),
            Err(e) => Err(ValidationRejection(
                StatusCode::BAD_REQUEST,
                err_body(&e.code, &e.message, e.param.as_deref(), &schema.id),
            )),
        }
    }
}
```

- [x] DONE 2026-08-27 **Step 5: Run to verify it passes**

Run: `cd litegen-core && cargo test --test model3d_validation 2>&1 | tail -20`
Expected: PASS (7 tests).

- [x] DONE 2026-08-27 **Step 6: Commit**

```bash
git add litegen-core/src/api/middleware/validator.rs litegen-core/tests/model3d_validation.rs
git commit -m "feat(3d): validate_model3d with lax param drop and the ValidatedModel3d extractor"
git push origin master
```

---

### Task 9: Router — `model3d_jobs`, `generate_model3d`, `get_model3d_status`, re-host helper

**Status:** ✅ DONE (2026-08-27, 3278851) — task review clean

**Files:**
- Modify: `litegen-core/src/proxy/router.rs`
- Test: `litegen-core/src/proxy/router.rs` (new `mod model3d_router_tests`)

**Interfaces:**
- Consumes: `Model3dProvider` + friends (Task 3), `registry::model3d_provider_for_request` (Task 6), `storage::{ImageStorage, model3d_asset_key}` (Task 7).
- Produces: `ProxyRouter::{generate_model3d, get_model3d_status, get_model3d_provider_job_id, has_model3d_provider, model3d_store}`; `pub async fn rehost_model3d_files(store, path_prefix, generation_id, files) -> Result<Vec<Model3dAsset>, ImageStoreError>`.

**Design note:** `rehost_model3d_files` is a free function (not a method) because the poller calls it too, and the poller has no `ProxyRouter`. Uploading happens in the same tick the provider reports completion — several vendors expire download URLs within five minutes, and re-hosting is what makes per-app BYO buckets work for meshes.

- [x] DONE 2026-08-27 **Step 1: Write the failing tests**

Append to `litegen-core/src/proxy/router.rs`:

```rust
#[cfg(test)]
mod model3d_router_tests {
    use super::*;
    use crate::providers::{Model3dAssetKind as _K, Model3dFile};
    use crate::proxy::storage::{model3d_asset_key, LocalModel3dStorage};
    use crate::types::Model3dAssetKind;

    fn mesh_file() -> Model3dFile {
        Model3dFile {
            kind: Model3dAssetKind::Mesh,
            format: "glb".into(),
            content_type: "model/gltf-binary".into(),
            bytes: b"glTF\x02\x00\x00\x00fake".to_vec(),
            polycount: Some(12),
            width: None,
            height: None,
        }
    }

    fn preview_file() -> Model3dFile {
        Model3dFile {
            kind: Model3dAssetKind::Preview,
            format: "png".into(),
            content_type: "image/png".into(),
            bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
            polycount: None,
            width: Some(512),
            height: Some(512),
        }
    }

    #[tokio::test]
    async fn rehost_writes_each_file_under_a_predictable_key_and_returns_absolute_urls() {
        let store: Arc<dyn crate::proxy::storage::ImageStorage> =
            Arc::new(LocalModel3dStorage::new("https://cdn.example.com".into()));

        let assets = rehost_model3d_files(&store, None, "litegen-3d-42", &[mesh_file(), preview_file()])
            .await
            .unwrap();

        assert_eq!(assets.len(), 2);
        let mesh = assets.iter().find(|a| a.kind == Model3dAssetKind::Mesh).unwrap();
        assert_eq!(mesh.url, "https://cdn.example.com/v1/models3d/assets/litegen/3d/litegen-3d-42/model.glb");
        assert_eq!(mesh.format, "glb");
        assert_eq!(mesh.polycount, Some(12));
        assert_eq!(mesh.size_bytes, Some(mesh_file().bytes.len() as u64));

        let preview = assets.iter().find(|a| a.kind == Model3dAssetKind::Preview).unwrap();
        assert!(preview.url.ends_with("/litegen-3d-42/preview.png"), "got {}", preview.url);
        assert_eq!(preview.width, Some(512));
        assert_eq!(preview.polycount, None);

        for a in &assets {
            assert!(a.url.starts_with("https://"), "asset urls must be absolute: {}", a.url);
        }
    }

    #[tokio::test]
    async fn rehost_numbers_multiple_files_of_the_same_kind() {
        let store: Arc<dyn crate::proxy::storage::ImageStorage> =
            Arc::new(LocalModel3dStorage::new("https://cdn.example.com".into()));
        let mut a = preview_file();
        a.format = "png".into();
        let b = preview_file();
        let assets = rehost_model3d_files(&store, None, "litegen-3d-7", &[mesh_file(), a, b]).await.unwrap();
        let urls: Vec<&str> = assets.iter().map(|x| x.url.as_str()).collect();
        assert_eq!(urls.len(), 3);
        assert_eq!(
            urls.iter().collect::<std::collections::HashSet<_>>().len(),
            3,
            "keys must not collide: {urls:?}"
        );
    }

    #[tokio::test]
    async fn rehost_honours_a_per_app_path_prefix() {
        let store: Arc<dyn crate::proxy::storage::ImageStorage> =
            Arc::new(LocalModel3dStorage::new("https://cdn.example.com".into()));
        let assets = rehost_model3d_files(&store, Some("tenant-9/meshes"), "g1", &[mesh_file()]).await.unwrap();
        assert!(assets[0].url.contains("/tenant-9/meshes/g1/model.glb"), "got {}", assets[0].url);
    }
}
```

- [x] DONE 2026-08-27 **Step 2: Run to verify it fails**

Run: `cd litegen-core && cargo test model3d_router 2>&1 | tail -20`
Expected: FAIL — `cannot find function rehost_model3d_files`.

- [x] DONE 2026-08-27 **Step 3: Implement the re-host helper**

Add to `litegen-core/src/proxy/router.rs` (module level, near the bottom before the tests):

```rust
/// Upload every file a 3D provider returned to litegen storage and describe them
/// as `Model3dAsset`s.
///
/// Called from BOTH the router (when a provider completes inline) and the poller
/// (the usual case), which is why it is a free function. It must run in the same
/// tick that observed completion: several vendors expire their download URLs
/// within minutes of task success, and re-hosting is also what makes an app's
/// BYO bucket apply to meshes exactly as it does to images.
///
/// The mesh is always keyed `model.<ext>`, so a client can construct the primary
/// URL without parsing the asset list. Additional files of the same kind get a
/// numeric suffix so keys never collide.
pub async fn rehost_model3d_files(
    store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    path_prefix: Option<&str>,
    generation_id: &str,
    files: &[crate::providers::Model3dFile],
) -> Result<Vec<Model3dAsset>, crate::proxy::storage::ImageStoreError> {
    use crate::proxy::storage::model3d_asset_key;

    let mut seen: HashMap<&'static str, u32> = HashMap::new();
    let mut assets = Vec::with_capacity(files.len());

    for f in files {
        let stem: &'static str = match f.kind {
            Model3dAssetKind::Mesh => "model",
            Model3dAssetKind::Texture => "texture",
            Model3dAssetKind::Preview => "preview",
        };
        let n = seen.entry(stem).or_insert(0);
        let name = if *n == 0 { stem.to_string() } else { format!("{stem}_{n}") };
        *n += 1;

        let key = model3d_asset_key(path_prefix, generation_id, &name, &f.format);
        let url = store
            .put(&key, &bytes::Bytes::from(f.bytes.clone()), &f.content_type)
            .await?;

        assets.push(Model3dAsset {
            kind: f.kind,
            url,
            format: f.format.clone(),
            size_bytes: Some(f.bytes.len() as u64),
            polycount: f.polycount,
            width: f.width,
            height: f.height,
        });
    }
    Ok(assets)
}
```

- [x] DONE 2026-08-27 **Step 4: Add the router field and methods**

In `ProxyRouter`, next to `video_jobs`:
```rust
    /// In-flight 3D generation jobs, keyed by the local `litegen-3d-...` ID.
    model3d_jobs: Arc<tokio::sync::RwLock<HashMap<String, Model3dGenerationHandle>>>,
    /// Storage for re-hosted 3D assets (S3 when configured, local otherwise).
    pub model3d_store: Arc<dyn crate::proxy::storage::ImageStorage>,
```

`ProxyRouter::new` gains a `model3d_store` parameter (added after `image_store`) and initialises `model3d_jobs` to an empty map. Update every construction site — find them with:
```bash
cd litegen-core && grep -rn "ProxyRouter::new" src/ tests/
```
At each site pass `crate::proxy::storage::build_model3d_store(&config.image_storage, &config.server.public_base_url())`. In test harnesses that build a config inline (`api/middleware/e2e_tests.rs`, `proxy/router.rs` tests, `tests/multitenant_api.rs`), pass `Arc::new(LocalModel3dStorage::new("http://test.local".into()))`.

Add the methods after `get_video_status`:

```rust
    // ─── 3D Model Generation ────────────────────────────────────────────

    /// Submit a 3D generation. Always async: the response is `pending` and the
    /// poller (or `get_model3d_status`) drives it to a terminal state.
    #[tracing::instrument(
        skip(self, schema, base, extras, materialized),
        fields(model = %schema.id, provider = %schema.provider)
    )]
    pub async fn generate_model3d(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        materialized: &MaterializedRequest,
        app_creds: Option<ProviderCredentials>,
    ) -> Result<Model3dGenerationResponse, ProxyError> {
        let provider = self
            .registry
            .model3d_provider_for_request(&schema.provider, app_creds)
            .await
            .ok_or_else(|| ProxyError::ProviderNotConfigured(schema.provider.clone()))?;

        let max_retries = 2u32;
        let mut last_error: Option<ProviderError> = None;
        let mut handle: Option<Model3dGenerationHandle> = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                info!(provider = %schema.provider, attempt, "Retrying 3D generation");
            }
            let attempt_start = Instant::now();
            match tokio::time::timeout(
                std::time::Duration::from_secs(120),
                provider.generate(schema, base, extras, materialized),
            )
            .await
            {
                Ok(Ok(h)) => {
                    self.record_latency(&schema.provider, attempt_start.elapsed().as_millis() as u64).await;
                    handle = Some(h);
                    break;
                }
                Ok(Err(e)) => {
                    let retryable = e.is_retryable();
                    last_error = Some(e);
                    if !retryable { break; }
                }
                Err(_) => last_error = Some(ProviderError::Timeout { timeout_ms: 120_000 }),
            }
        }

        let handle = handle.ok_or_else(|| ProxyError::AllDeploymentsFailed {
            model: schema.id.clone(),
            last_error: last_error.map(|e| e.to_string()),
        })?;

        // One submitted job produces one mesh, so billing is flat per generation
        // — `n` is meaningless here, exactly as it is for video.
        let base_cost = schema.pricing.base_cost_usd;
        let (_, total) = apply_markup(base_cost, self.config.cost_markup_percent);
        let usage_info = Some(UsageInfo {
            cost_usd: total,
            tokens: crate::providers::usd_to_tokens(total, 0.001),
            cost_source: CostSource::Estimated,
        });

        let local_id = format!("litegen-3d-{}", uuid::Uuid::new_v4());
        self.model3d_jobs.write().await.insert(local_id.clone(), handle);

        Ok(Model3dGenerationResponse {
            id: local_id,
            status: GenerationStatus::Pending,
            model: schema.id.clone(),
            provider: schema.provider.clone(),
            assets: Vec::new(),
            progress: 0,
            error: None,
            usage: usage_info,
            created: chrono::Utc::now().timestamp(),
        })
    }

    /// Poll an in-flight 3D generation by local ID, re-hosting its files if the
    /// provider reports completion on this call.
    #[tracing::instrument(skip(self), fields(id = %id))]
    pub async fn get_model3d_status(&self, id: &str) -> Result<Model3dGenerationResponse, ProxyError> {
        let handle = {
            let jobs = self.model3d_jobs.read().await;
            jobs.get(id).cloned()
        };
        let handle = handle.ok_or_else(|| ProxyError::NotFound(format!("3d job '{}' not found", id)))?;

        let provider = self
            .registry
            .model3d_provider_for(&handle.provider)
            .await
            .ok_or_else(|| ProxyError::ProviderNotConfigured(handle.provider.clone()))?;

        let poll = provider.poll_status(&handle).await.map_err(|e| ProxyError::ProviderError {
            provider: handle.provider.clone(),
            error: e.to_string(),
            retryable: e.is_retryable(),
        })?;

        let is_terminal = matches!(
            poll.status,
            GenerationStatus::Completed | GenerationStatus::Failed | GenerationStatus::Cancelled
        );

        // Re-host in the same call that saw completion — provider URLs expire.
        let assets = if poll.status == GenerationStatus::Completed && !poll.files.is_empty() {
            rehost_model3d_files(&self.model3d_store, None, id, &poll.files)
                .await
                .map_err(|e| ProxyError::ProviderError {
                    provider: handle.provider.clone(),
                    error: format!("failed to store 3d assets: {e}"),
                    retryable: true,
                })?
        } else {
            Vec::new()
        };

        if is_terminal {
            self.model3d_jobs.write().await.remove(id);
        }

        Ok(Model3dGenerationResponse {
            id: id.to_string(),
            status: poll.status,
            model: handle.model.clone(),
            provider: handle.provider.clone(),
            assets,
            progress: poll.progress,
            error: poll.error,
            usage: None,
            created: chrono::Utc::now().timestamp(),
        })
    }

    /// Provider job id for a submitted 3D generation (for the DB row).
    pub async fn get_model3d_provider_job_id(&self, local_id: &str) -> Option<String> {
        self.model3d_jobs.read().await.get(local_id).map(|h| h.provider_job_id.clone())
    }

    pub async fn has_model3d_provider(&self, name: &str) -> bool {
        self.registry.model3d_provider_for(name).await.is_some()
    }

    /// Estimate the cost of a 3D generation without dispatching it.
    pub async fn estimate_model3d_cost(
        &self,
        schema: &ModelSchema,
        request: &Model3dGenerationRequest,
    ) -> Result<CostEstimate, ProxyError> {
        let provider = self
            .registry
            .model3d_provider_for(&schema.provider)
            .await
            .ok_or_else(|| ProxyError::ProviderNotConfigured(schema.provider.clone()))?;
        provider.estimate_cost(schema, request).await.map_err(|e| ProxyError::ProviderError {
            provider: schema.provider.clone(),
            error: e.to_string(),
            retryable: e.is_retryable(),
        })
    }
```

Add the needed imports to the `use crate::providers::{...}` line: `Model3dExtras, Model3dGenerationHandle`.

Note: 3D deliberately has **no** `execute_route_model3d` / routing-strategy path in this phase — there is one 3D provider. Add the route dispatch alongside the real vendors in phase 2.

- [x] DONE 2026-08-27 **Step 5: Run to verify it passes**

Run: `cd litegen-core && cargo test model3d_router 2>&1 | tail -20`
Expected: PASS (3 tests).

Run: `cd litegen-core && cargo test 2>&1 | tail -20`
Expected: no regressions (the `ProxyRouter::new` signature change is fixed at every site).

- [x] DONE 2026-08-27 **Step 6: Commit**

```bash
git add litegen-core/src/proxy/router.rs litegen-core/src/api/middleware/e2e_tests.rs \
        litegen-core/tests/multitenant_api.rs litegen-core/src/main.rs litegen-core/src/lib.rs
git commit -m "feat(3d): router job tracking, dispatch, and same-tick asset re-hosting"
git push origin master
```
(Adjust the staged paths to whatever `grep -rn "ProxyRouter::new"` actually turned up.)

---

### Task 10: API handlers, routes, per-app storage, and metadata persistence

**Status:** ✅ DONE (2026-08-27, 85641e5 + af005d0) — task review clean

**Files:**
- Modify: `litegen-core/src/api/handlers/mod.rs` — `generate_3d`, `estimate_3d_cost`, `get_3d_status`, `resolve_app_model3d_store`, route table, `list_models` media filter
- Modify: `litegen-core/src/api/openapi.rs` — register the three paths
- Modify: `litegen-core/src/db/trait_def.rs`, `db/sqlite.rs`, `db/postgres.rs` — `update_generation_metadata`
- Modify: every `DatabaseStore` test stub (`api/middleware/e2e_tests.rs`, others found by the compiler)

**Interfaces:**
- Consumes: `ValidatedModel3d` (Task 8), `ProxyRouter::{generate_model3d, get_model3d_status, estimate_model3d_cost}` (Task 9), `resolve_org_provider_credential` / `reserve_quota` / `settle_quota` (existing).
- Produces: handlers `generate_3d`, `estimate_3d_cost`, `get_3d_status`; `resolve_app_model3d_store(state, key_ctx) -> Option<(Arc<dyn ImageStorage>, Option<String>)>`; `DatabaseStore::update_generation_metadata(id, &serde_json::Value)`.

**Why a metadata writer:** `generations.metadata` is already a nullable TEXT column in both migrations (`20240101000003_generations.sql`) — no migration is needed — but `update_generation_status` cannot write it. The asset list lives there so `GET /v1/generations/{id}` and the dashboard gallery can render a mesh without a second lookup.

- [x] DONE 2026-08-27 **Step 1: Add `update_generation_metadata` to the DB layer**

`litegen-core/src/db/trait_def.rs`, in `trait DatabaseStore`:
```rust
    /// Replace a generation's `metadata` JSON blob. Used by the 3D path to
    /// persist the re-hosted asset list; `metadata` is already a nullable TEXT
    /// column in both backends, so this needs no migration.
    async fn update_generation_metadata(
        &self,
        id: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), sqlx::Error>;
```

`db/sqlite.rs`:
```rust
    async fn update_generation_metadata(
        &self,
        id: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE generations SET metadata = ? WHERE id = ?")
            .bind(serde_json::to_string(metadata).unwrap_or_else(|_| "null".into()))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
```

`db/postgres.rs` — same body with `$1`/`$2` placeholders.

Then `cargo build` and add `async fn update_generation_metadata(&self, _id: &str, _metadata: &serde_json::Value) -> Result<(), sqlx::Error> { Ok(()) }` to every stub the compiler names.

- [x] DONE 2026-08-27 **Step 2: Write the failing integration test**

Create `litegen-core/tests/model3d_api.rs`. Model the harness on `src/api/middleware/e2e_tests.rs` (`AppState` + `tower::ServiceExt::oneshot`); read that file and copy its `NoopDb`/state construction rather than inventing one. Use an in-memory `SqliteDatabase` so generation rows persist.

```rust
//! End-to-end HTTP coverage for the `model3d` family, against the mock provider.
//! Mirrors the aipix acceptance checklist item-for-item.

mod harness; // see Step 3 — extracted from e2e_tests.rs

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn submit_poll_and_complete_yields_exactly_one_absolute_mesh_asset() {
    let (app, _db) = harness::app_with_mock_3d().await;

    // Submit.
    let resp = app.clone().oneshot(
        Request::post("/v1/models3d/generations")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"model":"mock/mesh-3d","prompt":"a low-poly fox"}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let submitted: serde_json::Value = harness::json_body(resp).await;
    assert_eq!(submitted["status"], "pending");
    assert_eq!(submitted["progress"], 0);
    assert!(submitted.get("assets").is_none(), "no assets before completion");
    let id = submitted["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("litegen-3d-"));

    // Poll to terminal.
    let mut last = serde_json::Value::Null;
    for _ in 0..10 {
        let resp = app.clone().oneshot(
            Request::get(format!("/v1/models3d/{id}")).body(Body::empty()).unwrap(),
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        last = harness::json_body(resp).await;
        if last["status"] == "completed" || last["status"] == "failed" { break; }
    }

    assert_eq!(last["status"], "completed");
    assert_eq!(last["progress"], 100);
    let assets = last["assets"].as_array().expect("completed carries assets");
    let meshes: Vec<_> = assets.iter().filter(|a| a["kind"] == "mesh").collect();
    assert_eq!(meshes.len(), 1, "exactly one mesh asset, always");
    let url = meshes[0]["url"].as_str().unwrap();
    assert!(url.starts_with("http://") || url.starts_with("https://"), "absolute url required, got {url}");
    assert!(url.ends_with(".glb"));
    assert_eq!(meshes[0]["format"], "glb");
}

#[tokio::test]
async fn the_mesh_url_actually_downloads_a_glb() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let url = harness::run_to_completion(&app, "mock/mesh-3d", "a fox").await;

    // Strip the origin: the asset route is mounted on this same app.
    let path = url.splitn(4, '/').nth(3).map(|p| format!("/{p}")).unwrap();
    let resp = app.oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "model/gltf-binary");
    let bytes = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024).await.unwrap();
    assert_eq!(&bytes[0..4], b"glTF", "the served bytes are a real GLB");
}

#[tokio::test]
async fn lax_mode_drops_unsupported_params_and_reports_them_in_the_header() {
    // The single most important behaviour in the aipix contract.
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.oneshot(
        Request::post("/v1/models3d/generations")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"model":"mock/mesh-3d","prompt":"a fox","strict":false,"rig":true,"symmetry":"on"}"#,
            ))
            .unwrap(),
    ).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK, "lax must not error on unsupported params");
    let dropped = resp.headers().get("x-litegen-dropped-params")
        .expect("X-Litegen-Dropped-Params must be set")
        .to_str().unwrap().to_string();
    assert!(dropped.contains("rig"), "got {dropped}");
    assert!(dropped.contains("symmetry"), "got {dropped}");
}

#[tokio::test]
async fn strict_mode_rejects_the_same_request() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.oneshot(
        Request::post("/v1/models3d/generations")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"model":"mock/mesh-3d","prompt":"a fox","rig":true}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = harness::json_body(resp).await;
    assert_eq!(body["error"]["code"], "param_unsupported");
}

#[tokio::test]
async fn cost_endpoint_returns_a_cost_estimate() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.oneshot(
        Request::post("/v1/models3d/cost")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"model":"mock/mesh-3d","prompt":"a fox"}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = harness::json_body(resp).await;
    for key in ["base_cost_usd", "markup_usd", "total_cost_usd", "tokens_required", "cost_source"] {
        assert!(body.get(key).is_some(), "CostEstimate missing '{key}': {body}");
    }
}

#[tokio::test]
async fn generation_row_records_media_type_model3d_and_the_asset_metadata() {
    let (app, db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/mesh-3d", "a fox").await;
    harness::poll_until_terminal(&app, &id).await;

    let row = db.get_generation(&id).await.unwrap().unwrap();
    assert_eq!(row.media_type, "model3d", "aipix discriminates on this exact string");
    assert!(row.result_url.as_deref().is_some_and(|u| u.ends_with(".glb")),
        "result_url is the primary mesh: {:?}", row.result_url);
    let meta = row.metadata.expect("assets are persisted for the gallery");
    let assets = meta["assets"].as_array().expect("metadata.assets");
    assert!(assets.iter().any(|a| a["kind"] == "mesh"));
}

#[tokio::test]
async fn list_models_exposes_the_3d_model_and_filters_by_media_type() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(
        Request::get("/v1/models?media_type=model3d").body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = harness::json_body(resp).await;
    let models = body["data"].as_array().unwrap();
    assert!(!models.is_empty(), "at least one model3d model must be listed");
    assert!(models.iter().all(|m| m["media_type"] == "model3d"), "filter must exclude other families");
    let mesh = models.iter().find(|m| m["id"] == "mock/mesh-3d").unwrap();
    assert_eq!(mesh["capabilities"]["supports_text_to_3d"], true);
    assert_eq!(mesh["capabilities"]["output_formats"][0], "glb");
}

#[tokio::test]
async fn model_schema_endpoint_serves_a_populated_params_map_for_3d() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.oneshot(
        Request::get("/v1/models/mock/all-params-3d").body(Body::empty()).unwrap(),
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let schema: serde_json::Value = harness::json_body(resp).await;
    assert_eq!(schema["media_type"], "model3d");
    let params = schema["params"].as_object().expect("params map");
    for key in ["output_format", "texture", "pbr", "rig", "symmetry", "topology", "target_polycount"] {
        assert!(params.contains_key(key), "3D schema must expose '{key}': {:?}", params.keys());
    }
    assert_eq!(params["target_polycount"]["label"], "Target polycount");
}

#[tokio::test]
async fn failing_model_terminates_failed_with_an_error_and_no_assets() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let id = harness::submit(&app, "mock/fail-3d", "anything").await;
    let last = harness::poll_until_terminal(&app, &id).await;
    assert_eq!(last["status"], "failed");
    assert!(last["error"].as_str().is_some_and(|e| !e.is_empty()));
    assert!(last.get("assets").is_none(), "a failed generation carries no assets");
}
```

- [x] DONE 2026-08-27 **Step 3: Build the shared test harness**

Create `litegen-core/tests/harness/mod.rs`. Read `src/api/middleware/e2e_tests.rs` first and lift its `AppState` construction verbatim, changing only: use `SqliteDatabase::connect("sqlite::memory:")` instead of `NoopDb`; register `MockModel3dProvider` alongside the image/video mocks; build the router from `crate::api::handlers::routes(...)` so the real route table (not a hand-picked subset) is under test.

```rust
use axum::body::Body;
use axum::http::Request;
use axum::Router;
use std::sync::Arc;
use tower::ServiceExt;

pub async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Full app with the mock image/video/3d providers and an in-memory sqlite DB.
pub async fn app_with_mock_3d() -> (Router, Arc<litegen::db::sqlite::SqliteDatabase>) {
    // … lift from src/api/middleware/e2e_tests.rs …
    todo!("copy the AppState wiring from e2e_tests.rs, add MockModel3dProvider")
}

pub async fn submit(app: &Router, model: &str, prompt: &str) -> String {
    let body = serde_json::json!({ "model": model, "prompt": prompt }).to_string();
    let resp = app.clone().oneshot(
        Request::post("/v1/models3d/generations")
            .header("content-type", "application/json")
            .body(Body::from(body)).unwrap(),
    ).await.unwrap();
    json_body(resp).await["id"].as_str().unwrap().to_string()
}

pub async fn poll_until_terminal(app: &Router, id: &str) -> serde_json::Value {
    for _ in 0..10 {
        let resp = app.clone().oneshot(
            Request::get(format!("/v1/models3d/{id}")).body(Body::empty()).unwrap(),
        ).await.unwrap();
        let v = json_body(resp).await;
        if v["status"] == "completed" || v["status"] == "failed" || v["status"] == "cancelled" {
            return v;
        }
    }
    panic!("3d job {id} never reached a terminal status");
}

pub async fn run_to_completion(app: &Router, model: &str, prompt: &str) -> String {
    let id = submit(app, model, prompt).await;
    let last = poll_until_terminal(app, &id).await;
    last["assets"].as_array().unwrap().iter()
        .find(|a| a["kind"] == "mesh").unwrap()["url"].as_str().unwrap().to_string()
}
```

Replace the `todo!` with the real wiring before running. If the handler route table requires auth for these endpoints, either construct a `KeyContext` the way `e2e_tests.rs` does, or mount the subset of routes those tests already mount — match whatever `e2e_tests.rs` does rather than weakening auth.

- [x] DONE 2026-08-27 **Step 4: Run to verify it fails**

Run: `cd litegen-core && cargo test --test model3d_api 2>&1 | tail -20`
Expected: FAIL — no `/v1/models3d/*` routes; 404s.

- [x] DONE 2026-08-27 **Step 5: Implement the handlers**

Add to `litegen-core/src/api/handlers/mod.rs`, after `estimate_video_cost`:

```rust
// ─── 3D Model Generation ────────────────────────────────────────────────────

/// POST /v1/models3d/generations — Start a 3D (mesh) generation.
#[utoipa::path(
    post,
    path = "/v1/models3d/generations",
    request_body = Model3dGenerationRequest,
    responses(
        (status = 200, description = "3D generation started", body = Model3dGenerationResponse),
        (status = 400, description = "Bad request", body = ErrorResponse),
    ),
    tag = "Models3D"
)]
pub async fn generate_3d(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    validated: ValidatedModel3d,
) -> impl IntoResponse {
    let start = std::time::Instant::now();

    let materialized = match state.materializer.materialize(
        &validated.schema,
        validated.request.base.reference_images.clone(),
        &validated.ctx,
    ).await {
        Ok(m) => m,
        Err(e) => return validation_rejection_response(&e.to_string(), 400, &validated.schema.id),
    };

    let extras = Model3dExtras {
        output_format: validated.request.output_format.clone(),
        texture: validated.request.texture,
        pbr: validated.request.pbr,
        target_polycount: validated.request.target_polycount,
        symmetry: validated.request.symmetry.clone(),
        topology: validated.request.topology.clone(),
        rig: validated.request.rig,
        extra: validated.request.base.extra.clone(),
    };

    let app_creds = match resolve_org_provider_credential(&state, &key_ctx, &validated.schema.provider).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if app_creds.is_none() && !state.router.has_model3d_provider(&validated.schema.provider).await {
        return provider_not_configured_response(&validated.schema.provider);
    }

    let charge_key = key_ctx.as_ref().and_then(|c| c.key_id);
    let reserved = match reserve_quota(&state, charge_key, validated.schema.pricing.base_cost_usd).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    match state.router.generate_model3d(
        &validated.schema, &validated.request.base, &extras, &materialized, app_creds,
    ).await {
        Ok(response) => {
            let latency = start.elapsed().as_millis() as i64;
            let cost = response.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);
            settle_quota(&state, charge_key, reserved, cost).await;

            let artifact = RequestArtifact {
                request_id: response.id.clone(),
                media_type: "model3d".to_string(),
                prompt: Some(validated.request.base.prompt.clone()),
                negative_prompt: validated.request.base.negative_prompt.clone(),
                params_json: serde_json::to_value(&extras).ok(),
                refs_meta_json: build_refs_meta(&validated.request.base.reference_images),
                output_kind: "url".to_string(),
                output_value: None, // async — the mesh URL is not known yet
                output_mime: None,
                output_truncated: false,
                error_message: None,
                created_at: chrono::Utc::now(),
                org_id: key_ctx.as_ref().and_then(|c| c.org_id.clone()),
                app_id: key_ctx.as_ref().and_then(|c| c.app_id.clone()),
            };

            let db = state.db.clone();
            let id = response.id.clone();
            let model = response.model.clone();
            let provider = response.provider.clone();
            let provider_job_id = state.router.get_model3d_provider_job_id(&id).await;
            let key_id = key_ctx.as_ref().and_then(|c| c.key_id);
            let org_id = key_ctx.as_ref().and_then(|c| c.org_id.clone());
            let app_id = key_ctx.as_ref().and_then(|c| c.app_id.clone());
            tokio::spawn(async move {
                let _ = db.log_request(&id, &model, &provider, "pending", "model3d", cost, latency, None, None, org_id.as_deref(), app_id.as_deref()).await;
                let _ = db.insert_generation(
                    &id, key_id.as_ref(), &model, &provider, "model3d",
                    provider_job_id.as_deref(), cost, org_id.as_deref(), app_id.as_deref(),
                ).await;
                if let Err(e) = db.insert_request_artifact(&artifact).await {
                    tracing::warn!(error = %e, request_id = %artifact.request_id, "Failed to store 3d artifact");
                }
            });

            let mut resp = (StatusCode::OK, Json(serde_json::to_value(response).unwrap())).into_response();
            if let Some((k, v)) = dropped_header(&validated.dropped) {
                resp.headers_mut().insert(k, v);
            }
            resp
        }
        Err(e) => {
            settle_quota(&state, charge_key, reserved, 0.0).await;
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            error!(error = %e, "3D generation failed");
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

/// GET /v1/models3d/{id} — Poll the status of an in-flight 3D generation.
#[utoipa::path(
    get,
    path = "/v1/models3d/{id}",
    params(("id" = String, Path, description = "3D generation ID")),
    responses(
        (status = 200, description = "Current status", body = Model3dGenerationResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
    ),
    tag = "Models3D"
)]
pub async fn get_3d_status(
    State(state): State<Arc<AppState>>,
    OptionalKeyContext(key_ctx): OptionalKeyContext,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Same tenant scoping as get_video_status: a persisted row must belong to
    // the caller's org; router-tracked in-flight jobs pass through.
    let ctx_org = match key_ctx.as_ref().and_then(|c| c.org_id.as_deref()) {
        Some(o) => o,
        None => return forbidden_no_org(),
    };
    if let Ok(Some(gen)) = state.db.get_generation(&id).await {
        if gen.org_id.as_deref() != Some(ctx_org) {
            return (StatusCode::NOT_FOUND, Json(error_response("Not found", 404))).into_response();
        }
        if let Err(resp) = authorize_generation_for_session(
            &state, key_ctx.as_ref(), &gen,
            crate::auth::permissions::Permission::GenerationReadAny,
            crate::auth::permissions::Permission::GenerationReadOwn,
            "generation:read:own",
        ).await {
            return resp;
        }
    }

    match state.router.get_model3d_status(&id).await {
        Ok(resp) => {
            // Persist the terminal result so `GET /v1/generations/{id}`, the
            // gallery, and any webhook see the same assets the poller would have
            // written — whichever path observed completion first.
            if resp.status == GenerationStatus::Completed && !resp.assets.is_empty() {
                persist_model3d_result(&state, &resp).await;
            } else if resp.status == GenerationStatus::Failed {
                let _ = state.db.update_generation_status(
                    &id, "failed", resp.progress as i32, None,
                    resp.error.as_deref(), Some(chrono::Utc::now()),
                ).await;
            }
            (StatusCode::OK, Json(serde_json::to_value(resp).unwrap())).into_response()
        }
        Err(e) => {
            // Fall through to the DB row: once the poller has terminalised a
            // job the router no longer tracks it.
            if let Ok(Some(gen)) = state.db.get_generation(&id).await {
                return (StatusCode::OK, Json(serde_json::to_value(model3d_response_from_row(&gen)).unwrap())).into_response();
            }
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

/// POST /v1/models3d/cost — Estimate cost for a 3D generation.
#[utoipa::path(
    post,
    path = "/v1/models3d/cost",
    request_body = Model3dGenerationRequest,
    responses(
        (status = 200, description = "Cost estimate", body = CostEstimate),
        (status = 400, description = "Bad request", body = ErrorResponse),
    ),
    tag = "Models3D"
)]
pub async fn estimate_3d_cost(
    State(state): State<Arc<AppState>>,
    validated: ValidatedModel3d,
) -> impl IntoResponse {
    match state.router.estimate_model3d_cost(&validated.schema, &validated.request).await {
        Ok(est) => (StatusCode::OK, Json(serde_json::to_value(est).unwrap())).into_response(),
        Err(e) => {
            let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(error_response(&e.to_string(), status.as_u16()))).into_response()
        }
    }
}

/// Write a completed 3D result to its generation row: `result_url` is the
/// primary mesh (so every existing cross-modal consumer keeps working), and the
/// full asset list rides `metadata.assets`.
pub(crate) async fn persist_model3d_result(state: &AppState, resp: &Model3dGenerationResponse) {
    let mesh_url = resp.mesh().map(|m| m.url.clone());
    if let Err(e) = state.db.update_generation_status(
        &resp.id, "completed", resp.progress as i32,
        mesh_url.as_deref(), None, Some(chrono::Utc::now()),
    ).await {
        tracing::warn!(generation_id = %resp.id, error = %e, "failed to persist 3d status");
        return;
    }
    let meta = serde_json::json!({ "assets": resp.assets });
    if let Err(e) = state.db.update_generation_metadata(&resp.id, &meta).await {
        tracing::warn!(generation_id = %resp.id, error = %e, "failed to persist 3d assets");
    }
}

/// Rebuild a `Model3dGenerationResponse` from a persisted row (used once the
/// poller has terminalised a job and the router no longer tracks it).
pub(crate) fn model3d_response_from_row(gen: &crate::types::Generation) -> Model3dGenerationResponse {
    let assets = gen.metadata.as_ref()
        .and_then(|m| m.get("assets"))
        .and_then(|a| serde_json::from_value::<Vec<Model3dAsset>>(a.clone()).ok())
        .unwrap_or_default();
    Model3dGenerationResponse {
        id: gen.id.clone(),
        status: gen.status,
        model: gen.model.clone(),
        provider: gen.provider.clone(),
        assets,
        progress: gen.progress.clamp(0, 100) as u8,
        error: gen.error_message.clone(),
        usage: None,
        created: gen.created_at.timestamp(),
    }
}
```

Add `use crate::api::middleware::validator::ValidatedModel3d;` and `use crate::providers::Model3dExtras;` to the handler imports.

- [x] DONE 2026-08-27 **Step 6: Register the routes and the `media_type` filter**

In the route table, next to the video routes:
```rust
        .route("/v1/models3d/generations", post(generate_3d))
        .route("/v1/models3d/cost", post(estimate_3d_cost))
        .route("/v1/models3d/{id}", get(get_3d_status))
```
Register `/v1/models3d/{id}` **after** `/v1/models3d/generations` and `/v1/models3d/cost` so the literal segments win over the capture. Verify with the `submit_poll_and_complete_*` test — a 405/404 on POST means the ordering is wrong.

Extend `list_models` with the optional filter aipix asked for:
```rust
#[derive(serde::Deserialize)]
pub struct ListModelsQuery {
    /// Optional `image` | `video` | `model3d` filter.
    #[serde(default)]
    pub media_type: Option<String>,
}
```
and in the handler:
```rust
    let models: Vec<ModelInfo> = state.registry.all()
        .map(project_model_info)
        .filter(|m| match q.media_type.as_deref() {
            None | Some("") => true,
            Some(want) => serde_json::to_value(m.media_type)
                .ok()
                .and_then(|v| v.as_str().map(|s| s == want))
                .unwrap_or(false),
        })
        .collect();
```
Add the `params(("media_type" = Option<String>, Query, description = "Filter by media type"))` line to its `#[utoipa::path]`.

- [x] DONE 2026-08-27 **Step 7: Add `resolve_app_model3d_store`**

Next to `resolve_app_image_store` — same `app_storage_credentials` row, no new table, no new dashboard form, so an app that configured BYO storage for images gets it for meshes for free:

```rust
/// Resolve the calling app's BYO 3D asset store, if configured & usable.
///
/// Reads the SAME `app_storage_credentials` row as `resolve_app_image_store`,
/// but builds an `S3Storage` (key-explicit `put`) rather than an `S3Store`
/// (content-type→extension inference, which would save a `.glb` as `.png`).
/// Returns the store and the app's configured path prefix. Fails open to the
/// global store on any error — a bad BYO row must never break a generation.
pub(crate) async fn resolve_app_model3d_store(
    state: &AppState,
    app_id: &str,
) -> Option<(std::sync::Arc<dyn crate::proxy::storage::ImageStorage>, Option<String>)> {
    let secrets_key = state.secrets_key?;
    let row = match state.db.get_app_storage(app_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(app_id, error = %e, "byo 3d storage: lookup failed, using global store");
            return None;
        }
    };

    #[derive(serde::Deserialize)]
    struct StorageSecret { access_key_id: String, secret_access_key: String }

    let plaintext = crate::auth::secrets::decrypt(&secrets_key, &row.secret_ciphertext, &row.secret_nonce)
        .map_err(|e| tracing::warn!(app_id, error = %e, "byo 3d storage: decrypt failed"))
        .ok()?;
    let secret: StorageSecret = serde_json::from_slice(&plaintext)
        .map_err(|e| tracing::warn!(app_id, error = %e, "byo 3d storage: corrupt secret"))
        .ok()?;

    let cfg = crate::config::ImageStorageConfig {
        backend: row.backend.clone(),
        path_prefix: row.path_prefix.clone(),
        s3: Some(crate::config::S3StorageConfig {
            bucket_name: row.bucket_name.clone(),
            region: row.region.clone(),
            access_key_id: Some(secret.access_key_id),
            secret_access_key: Some(secret.secret_access_key),
            endpoint_url: row.endpoint_url.clone(),
            custom_public_url: row.custom_public_url.clone(),
        }),
    };
    match crate::proxy::storage::S3Storage::from_config(&cfg) {
        Ok(store) => Some((std::sync::Arc::new(store), row.path_prefix.clone())),
        Err(e) => {
            tracing::warn!(app_id, error = %e, "byo 3d storage: build failed, using global store");
            None
        }
    }
}
```

The poller (Task 11) is the caller — it resolves per-app storage per row. Nothing in `generate_3d` needs it, because 3D never completes inline.

- [x] DONE 2026-08-27 **Step 8: Register the paths with utoipa**

In `src/api/openapi.rs` `paths(...)`, after the video entries:
```rust
        crate::api::handlers::generate_3d,
        crate::api::handlers::estimate_3d_cost,
        crate::api::handlers::get_3d_status,
```

- [x] DONE 2026-08-27 **Step 9: Run to verify it passes**

Run: `cd litegen-core && cargo test --test model3d_api 2>&1 | tail -30`
Expected: PASS (9 tests).

Run: `cd litegen-core && cargo test 2>&1 | tail -20`
Expected: no regressions.

- [x] DONE 2026-08-27 **Step 10: Commit**

```bash
git add litegen-core/src/api/handlers/mod.rs litegen-core/src/api/openapi.rs \
        litegen-core/src/db/ litegen-core/src/api/middleware/e2e_tests.rs \
        litegen-core/tests/model3d_api.rs litegen-core/tests/harness/
git commit -m "feat(3d): /v1/models3d endpoints, media_type filter, and asset persistence"
git push origin master
```

---

### Task 11: Poller — media-type dispatch and same-tick re-hosting

**Status:** ✅ DONE (2026-08-27, a1e1821 + 812f94f) — task review clean

**Files:**
- Modify: `litegen-core/src/proxy/poller.rs`
- Test: `litegen-core/src/proxy/poller.rs` (`mod poller_tests`)

**Interfaces:**
- Consumes: `registry::model3d_provider_for_request` (Task 6), `router::rehost_model3d_files` (Task 9), `handlers::resolve_app_model3d_store` (Task 10), `DatabaseStore::update_generation_metadata` (Task 10), `storage::build_model3d_store` (Task 7).
- Produces: `poll_once` handling `media_type == "model3d"`; a new `poll_once` parameter carrying the global 3D store.

**The rule that drives the shape:** `poll_status()` returning `Completed` comes back with bytes already in hand, and the upload must happen **in the same loop iteration**. Video never had this pressure because litegen passes provider URLs straight through; several 3D vendors expire download URLs within five minutes.

- [x] DONE 2026-08-27 **Step 1: Write the failing tests**

Append to `mod poller_tests` in `litegen-core/src/proxy/poller.rs`:

```rust
    use crate::providers::model3d::mock::MockModel3dProvider;
    use crate::providers::Model3dProvider;
    use crate::proxy::storage::{ImageStorage, LocalModel3dStorage};

    async fn make_3d_registry() -> Arc<ProviderRegistry> {
        let reg = Arc::new(ProviderRegistry::new());
        let mut p = MockModel3dProvider::new();
        p.configure(ProviderInstanceConfig::default());
        reg.register_mock_model3d(Arc::new(p)).await;
        reg
    }

    fn test_3d_store() -> Arc<dyn ImageStorage> {
        Arc::new(LocalModel3dStorage::new("https://assets.test".into()))
    }

    /// Submit through the mock provider so the row's provider_job_id refers to a
    /// job the provider actually knows about.
    async fn submit_mock_3d(model: &str, prompt: &str) -> String {
        use crate::capabilities::{MediaType, ModelCapabilityFlags, ModelPricing, PromptSpec};
        let mut p = MockModel3dProvider::new();
        p.configure(ProviderInstanceConfig::default());
        let schema = crate::capabilities::ModelSchema {
            id: model.into(), provider: "mock".into(), media_type: MediaType::Model3d,
            display_name: model.into(), description: String::new(),
            pricing: ModelPricing { base_cost_usd: 0.0, variable_pricing: None },
            capabilities: ModelCapabilityFlags { text_to_3d: true, ..Default::default() },
            prompt: PromptSpec { required: true, min_length: None, max_length: None },
            params: Default::default(), ref_inputs: None, extra_allowlist: vec![], tags: vec![],
        };
        let base = crate::types::BaseGenerationRequest {
            prompt: prompt.into(), model: model.into(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        };
        let extras = crate::providers::Model3dExtras {
            output_format: None, texture: None, pbr: None, target_polycount: None,
            symmetry: None, topology: None, rig: None, extra: None,
        };
        p.generate(&schema, &base, &extras, &Default::default()).await.unwrap().provider_job_id
    }

    #[tokio::test]
    async fn poll_once_drives_a_model3d_row_to_completed_with_a_rehosted_mesh() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_3d_registry().await;
        let store = test_3d_store();
        let job_id = submit_mock_3d("mock/mesh-3d", "a fox").await;

        db.insert_generation(
            "litegen-3d-poll-1", None, "mock/mesh-3d", "mock", "model3d",
            Some(&job_id), 0.0, None, None,
        ).await.unwrap();

        // The mock ramps over three polls, so drive several ticks.
        for _ in 0..5 {
            poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
            let row = db.get_generation("litegen-3d-poll-1").await.unwrap().unwrap();
            if row.status == crate::types::GenerationStatus::Completed { break; }
        }

        let row = db.get_generation("litegen-3d-poll-1").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed);
        assert_eq!(row.progress, 100);

        // result_url is the primary mesh, absolute, .glb — so every existing
        // cross-modal consumer keeps working without knowing about assets[].
        let url = row.result_url.expect("mesh url");
        assert!(url.starts_with("https://assets.test/"), "must be re-hosted, got {url}");
        assert!(url.ends_with("/model.glb"), "got {url}");

        let meta = row.metadata.expect("asset list persisted");
        let assets = meta["assets"].as_array().unwrap();
        assert_eq!(assets.iter().filter(|a| a["kind"] == "mesh").count(), 1);
        assert!(assets.iter().any(|a| a["kind"] == "preview"));
    }

    #[tokio::test]
    async fn poll_once_still_drives_video_rows_through_the_video_provider() {
        // Regression guard for the media_type branch: adding 3D must not send
        // video rows down the `_ => skip` arm.
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_registry().await; // video mock
        let store = test_3d_store();

        db.insert_generation(
            "litegen-vid-branch-1", None, "mock/video-gen", "mock", "video",
            Some("mock-video-job-1"), 0.0, None, None,
        ).await.unwrap();

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        let row = db.get_generation("litegen-vid-branch-1").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Completed);
        assert!(row.result_url.is_some());
    }

    #[tokio::test]
    async fn poll_once_marks_a_failed_3d_row_without_assets() {
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_3d_registry().await;
        let store = test_3d_store();
        let job_id = submit_mock_3d("mock/fail-3d", "anything").await;

        db.insert_generation(
            "litegen-3d-fail-1", None, "mock/fail-3d", "mock", "model3d",
            Some(&job_id), 0.0, None, None,
        ).await.unwrap();

        for _ in 0..5 {
            poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;
            let row = db.get_generation("litegen-3d-fail-1").await.unwrap().unwrap();
            if row.status == crate::types::GenerationStatus::Failed { break; }
        }

        let row = db.get_generation("litegen-3d-fail-1").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Failed);
        assert!(row.error_message.is_some_and(|e| !e.is_empty()));
        assert!(row.result_url.is_none(), "a failed generation must not carry a mesh url");
    }

    #[tokio::test]
    async fn poll_once_skips_a_row_whose_media_type_has_no_async_provider() {
        // `image` rows are synchronous and never land in the active window; if
        // one ever does, it must be skipped, not crash the tick.
        let db: Arc<dyn DatabaseStore> = in_memory_db().await;
        let registry = make_3d_registry().await;
        let store = test_3d_store();

        db.insert_generation(
            "litegen-img-stray-1", None, "mock/image-gen", "mock", "image",
            Some("job-x"), 0.0, None, None,
        ).await.unwrap();

        poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        let row = db.get_generation("litegen-img-stray-1").await.unwrap().unwrap();
        assert_eq!(row.status, crate::types::GenerationStatus::Pending, "skipped, not failed");
    }
```

Every existing `poll_once(...)` call in this module gains the `&store` argument.

- [x] DONE 2026-08-27 **Step 2: Run to verify it fails**

Run: `cd litegen-core && cargo test poller_tests 2>&1 | tail -20`
Expected: FAIL — `poll_once` takes 4 arguments, not 5.

- [x] DONE 2026-08-27 **Step 3: Restructure `poll_once`**

Change the signature and hoist the per-row work into a media-type branch:

```rust
pub(crate) async fn poll_once(
    db: &Arc<dyn DatabaseStore>,
    registry: &Arc<ProviderRegistry>,
    secrets_key: Option<[u8; 32]>,
    mode: crate::config::Mode,
    model3d_store: &Arc<dyn crate::proxy::storage::ImageStorage>,
) {
```

Inside the `for gen in rows` loop, replace the unconditional `registry.video_provider_for_request(...)` block with a branch that produces a common `(status, progress, result_url, error, assets)` tuple:

```rust
        // Result of polling one row, normalised across modalities so the
        // reaper, DB update, and webhook dispatch below stay modality-agnostic.
        struct PollOutcome {
            status: GenerationStatus,
            progress: u8,
            result_url: Option<String>,
            error: Option<String>,
            /// 3D only: the full re-hosted asset list, persisted to metadata.
            assets: Option<serde_json::Value>,
        }

        let outcome: Option<PollOutcome> = match gen.media_type.as_str() {
            "video" => {
                // … existing video path, unchanged: resolve provider, require
                // provider_job_id, build VideoGenerationHandle, poll_status …
                // then:
                Some(PollOutcome {
                    status: poll.status,
                    progress: poll.progress,
                    result_url: poll.video_url,
                    error: poll.error,
                    assets: None,
                })
            }
            "model3d" => {
                let Some(provider) = registry
                    .model3d_provider_for_request(&gen.provider, app_creds)
                    .await
                else {
                    if over_age {
                        warn!(generation_id = %gen.id, provider = %gen.provider, "poller: 3d provider not found past max age, reaping");
                        reap_generation(db, &gen.id, "provider no longer configured").await;
                    } else {
                        warn!(generation_id = %gen.id, provider = %gen.provider, "poller: 3d provider not found, skipping");
                    }
                    continue;
                };
                let Some(provider_job_id) = gen.provider_job_id.clone() else {
                    if over_age {
                        reap_generation(db, &gen.id, "no provider job id was recorded").await;
                    }
                    continue;
                };
                let handle = crate::providers::Model3dGenerationHandle {
                    provider_job_id,
                    provider: gen.provider.clone(),
                    model: gen.model.clone(),
                    stage_context: gen
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("stage_context").cloned()),
                };
                let poll = match provider.poll_status(&handle).await {
                    Ok(p) => p,
                    Err(e) => {
                        handle_poll_error(db, &gen, &e, over_age).await;
                        continue;
                    }
                };

                // Re-host NOW, in the same tick that observed completion:
                // several vendors expire their download URLs within minutes.
                let assets = if poll.status == GenerationStatus::Completed && !poll.files.is_empty() {
                    // Per-app BYO bucket if the app configured one, else global.
                    let (store, prefix) = match gen.app_id.as_deref() {
                        Some(app_id) => crate::api::handlers::resolve_app_model3d_store(
                            /* AppState is not available here — see note */ db, secrets_key, app_id,
                        )
                        .await
                        .map(|(s, p)| (s, p))
                        .unwrap_or_else(|| (model3d_store.clone(), None)),
                        None => (model3d_store.clone(), None),
                    };
                    match crate::proxy::router::rehost_model3d_files(
                        &store, prefix.as_deref(), &gen.id, &poll.files,
                    ).await {
                        Ok(a) => Some(a),
                        Err(e) => {
                            // Storage failure is retryable — leave the row
                            // non-terminal so the next tick tries again rather
                            // than reporting a completed generation with no mesh.
                            warn!(generation_id = %gen.id, error = %e, "poller: 3d asset upload failed, will retry");
                            continue;
                        }
                    }
                } else {
                    None
                };

                // Contract: a completed generation ALWAYS carries a mesh. If the
                // provider reported success without one, that is a failure, not
                // a degraded success — a paid generation the caller cannot use.
                if poll.status == GenerationStatus::Completed
                    && !assets.as_ref().is_some_and(|a| a.iter().any(|x| x.kind == crate::types::Model3dAssetKind::Mesh))
                {
                    warn!(generation_id = %gen.id, "poller: 3d completed without a mesh asset, failing the row");
                    reap_generation(db, &gen.id, "provider reported success without a mesh asset").await;
                    continue;
                }

                let mesh_url = assets.as_ref().and_then(|a| {
                    a.iter().find(|x| x.kind == crate::types::Model3dAssetKind::Mesh).map(|m| m.url.clone())
                });
                Some(PollOutcome {
                    status: poll.status,
                    progress: poll.progress,
                    result_url: mesh_url,
                    error: poll.error,
                    assets: assets.map(|a| serde_json::json!({ "assets": a })),
                })
            }
            other => {
                // Only video and 3D are async today. An `image` row here means
                // something upstream is wrong; log and skip rather than reap.
                warn!(generation_id = %gen.id, media_type = %other, "poller: no async provider for media type, skipping");
                continue;
            }
        };
        let Some(outcome) = outcome else { continue };
```

Then the existing terminal/reap/update/webhook tail operates on `outcome` instead of `poll`, with one addition after `update_generation_status` succeeds:

```rust
        if let Some(meta) = outcome.assets {
            if let Err(e) = db.update_generation_metadata(&gen.id, &meta).await {
                warn!(generation_id = %gen.id, error = %e, "poller: failed to persist 3d assets");
            }
        }
```

Extract the repeated error handling into a helper so both branches share it:
```rust
/// Shared poll-error policy: a non-retryable upstream error fails the row now
/// (the job is gone and retrying cannot recover it); a retryable one is left
/// alone unless the row is past MAX_ACTIVE_AGE.
async fn handle_poll_error(
    db: &Arc<dyn DatabaseStore>,
    gen: &crate::types::Generation,
    e: &crate::providers::ProviderError,
    over_age: bool,
) {
    if !e.is_retryable() {
        warn!(generation_id = %gen.id, error = %e, "poller: non-retryable poll error, marking failed");
        reap_generation(db, &gen.id, &e.to_string()).await;
    } else if over_age {
        warn!(generation_id = %gen.id, error = %e, "poller: poll failing past max age, reaping");
        reap_generation(db, &gen.id, "timed out awaiting provider").await;
    } else {
        warn!(generation_id = %gen.id, error = %e, "poller: poll_status failed, will retry");
    }
}
```

**On `resolve_app_model3d_store` in the poller:** that function as written in Task 10 takes `&AppState`, which `poll_once` does not have. Refactor it to take `(db: &Arc<dyn DatabaseStore>, secrets_key: Option<[u8; 32]>, app_id: &str)` — those are the only two things it reads from state — and have any handler-side caller pass `&state.db, state.secrets_key`. Do that refactor here rather than duplicating the decrypt logic.

- [x] DONE 2026-08-27 **Step 4: Update the poller's spawn site**

Find where the poll loop is spawned (`grep -rn "poll_once" src/ | grep -v tests`) and thread the store through, built once at startup:
```rust
let model3d_store = crate::proxy::storage::build_model3d_store(
    &config.image_storage,
    &config.server.public_base_url(),
);
```
Pass the same `Arc` the router holds if one is already available there — `state.router.model3d_store.clone()` avoids building it twice and keeps the local in-process store consistent between the router and poller paths. Prefer that.

- [x] DONE 2026-08-27 **Step 5: Run to verify it passes**

Run: `cd litegen-core && cargo test poller 2>&1 | tail -20`
Expected: PASS (existing video tests + 4 new).

Run: `cd litegen-core && cargo test 2>&1 | tail -20`
Expected: no regressions.

- [x] DONE 2026-08-27 **Step 6: Commit**

```bash
git add litegen-core/src/proxy/poller.rs litegen-core/src/api/handlers/mod.rs litegen-core/src/main.rs
git commit -m "feat(3d): poller dispatches by media type and re-hosts assets in the same tick"
git push origin master
```

---

### Task 12: TypeScript SDK — generic `pollJob`, `Model3dJob`, `client.models3d`

**Status:** ✅ DONE (2026-08-27, 12ef80c + 11d4d22) — task review clean

**Files:**
- Modify: `sdks/typescript/src/polling.ts`, `src/client.ts`, `src/index.ts`
- Regenerate: `sdks/typescript/src/generated/schema.ts`, `apps/landing/public/openapi.json`
- Test: `sdks/typescript/test/models3d.test.ts` (match the existing test file layout — check `sdks/typescript/package.json`'s `test` script first)

**Interfaces:**
- Consumes: the regenerated `components["schemas"]["Model3dGenerationRequest" | "Model3dGenerationResponse" | "Model3dAsset"]`.
- Produces: `pollJob<T>`, `poll3d`, `Model3dJob`, `client.models3d.{generate,estimateCost,getStatus,waitForCompletion,poll}`, exported types `Model3dGenerationRequest`, `Model3dGenerationResponse`, `Model3dAsset`, `Model3dJob`, and `MediaType.Model3d`.

- [x] DONE 2026-08-27 **Step 1: Regenerate the OpenAPI document and SDK schema**

```bash
cd litegen-core && cargo run --bin litegen -- --dump-openapi > ../apps/landing/public/openapi.json 2>/dev/null \
  || cargo test openapi 2>&1 | tail -5
```
If there is no dump flag, find how `apps/landing/public/openapi.json` is currently produced (`grep -rn "openapi.json" scripts/ sdks/scripts/ package.json`) and use that path. Then:
```bash
cd sdks && ./scripts/regen-all.sh
```
Confirm the new schemas landed:
```bash
grep -c "Model3dGenerationResponse" sdks/typescript/src/generated/schema.ts   # expect >= 1
grep -o '"/v1/models3d/[^"]*"' apps/landing/public/openapi.json | sort -u     # expect 3 paths
```

- [x] DONE 2026-08-27 **Step 2: Write the failing test**

Create `sdks/typescript/test/models3d.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import { LiteGenClient } from "../src/client";
import { pollJob } from "../src/polling";

type Resp = { id: string; status: string; progress: number; assets?: Array<{ kind: string; url: string }> };

function clientWith(fetchImpl: typeof fetch) {
  return new LiteGenClient({ baseUrl: "https://litegen.test", apiKey: "sk_test", fetch: fetchImpl });
}

function jsonResponse(body: unknown) {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

describe("client.models3d", () => {
  it("submits to /v1/models3d/generations", async () => {
    const fetchMock = vi.fn(async (url: string) => {
      expect(String(url)).toContain("/v1/models3d/generations");
      return jsonResponse({ id: "litegen-3d-1", status: "pending", progress: 0 });
    });
    const c = clientWith(fetchMock as unknown as typeof fetch);
    const job = c.models3d.generate({ model: "mock/mesh-3d", prompt: "a fox" } as never);
    const submitted = await job.submitted;
    expect(submitted.id).toBe("litegen-3d-1");
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("polls GET /v1/models3d/{id} until terminal and returns the mesh", async () => {
    const states: Resp[] = [
      { id: "litegen-3d-1", status: "pending", progress: 0 },
      { id: "litegen-3d-1", status: "processing", progress: 66 },
      {
        id: "litegen-3d-1",
        status: "completed",
        progress: 100,
        assets: [{ kind: "mesh", url: "https://cdn.test/litegen/3d/litegen-3d-1/model.glb" }],
      },
    ];
    let n = 0;
    const fetchMock = vi.fn(async (url: string) => {
      if (String(url).endsWith("/generations")) return jsonResponse(states[0]);
      expect(String(url)).toContain("/v1/models3d/litegen-3d-1");
      return jsonResponse(states[Math.min(++n, states.length - 1)]);
    });
    const c = clientWith(fetchMock as unknown as typeof fetch);

    const final = await c.models3d.generate({ model: "mock/mesh-3d", prompt: "a fox" } as never, { intervalMs: 1 });
    expect(final.status).toBe("completed");
    expect(final.assets?.find((a) => a.kind === "mesh")?.url).toMatch(/^https:\/\/.*\.glb$/);
  });

  it("streams progress updates via for-await", async () => {
    const states: Resp[] = [
      { id: "j", status: "processing", progress: 33 },
      { id: "j", status: "completed", progress: 100, assets: [{ kind: "mesh", url: "https://cdn.test/m.glb" }] },
    ];
    let n = -1;
    const c = clientWith((async () => jsonResponse(states[Math.min(++n, 1)])) as unknown as typeof fetch);

    const seen: number[] = [];
    for await (const u of c.models3d.poll("j", { intervalMs: 1 })) seen.push(u.progress);
    expect(seen).toEqual([33, 100]);
  });

  it("estimateCost posts to /v1/models3d/cost", async () => {
    const fetchMock = vi.fn(async (url: string) => {
      expect(String(url)).toContain("/v1/models3d/cost");
      return jsonResponse({ base_cost_usd: 0.1, markup_usd: 0, total_cost_usd: 0.1, tokens_required: 100, cost_source: "estimated" });
    });
    const c = clientWith(fetchMock as unknown as typeof fetch);
    const est = await c.models3d.estimateCost({ model: "mock/mesh-3d", prompt: "a fox" } as never);
    expect(est.total_cost_usd).toBe(0.1);
  });
});

describe("pollJob", () => {
  it("is generic over any { status } response", async () => {
    // The point of the refactor: poll3d is a two-line wrapper, not a copy.
    const states = [{ status: "processing" }, { status: "completed" }];
    let n = -1;
    const seen: string[] = [];
    for await (const u of pollJob("x", async () => states[Math.min(++n, 1)], { intervalMs: 1 })) {
      seen.push(u.status);
    }
    expect(seen).toEqual(["processing", "completed"]);
  });
});
```

- [x] DONE 2026-08-27 **Step 3: Run to verify it fails**

Run: `cd sdks/typescript && npm test 2>&1 | tail -20`
Expected: FAIL — `client.models3d` is undefined, `pollJob` is not exported.

- [x] DONE 2026-08-27 **Step 4: Generalise `polling.ts`**

Replace the body of `sdks/typescript/src/polling.ts`'s poll functions with the generic version (types and `TERMINAL_STATUSES` stay as they are):

```ts
import type { components } from "./generated/schema";
import { LiteGenPollingTimeoutError } from "./errors";

type VideoResponse = components["schemas"]["VideoGenerationResponse"];
type Model3dResponse = components["schemas"]["Model3dGenerationResponse"];

/** Any pollable job response. */
type WithStatus = { status: string };

export interface WaitForCompletionOptions {
  /** Milliseconds between polls. Default 2000. */
  intervalMs?: number;
  /** Total timeout in milliseconds. Default 5 minutes. */
  timeoutMs?: number;
  /** Optional AbortSignal to cancel polling. */
  signal?: AbortSignal;
}

const TERMINAL_STATUSES = new Set<string>(["completed", "failed", "cancelled"]);

/**
 * Poll any async job, yielding every status update — including the terminal one
 * — as an async iterable. `progress` is LiteGen's unified 0–100 value across
 * every provider and modality, so this loop behaves identically for video and
 * 3D. Iteration stops after the first terminal status, which is the final value
 * yielded.
 */
export async function* pollJob<T extends WithStatus>(
  id: string,
  getStatus: (id: string) => Promise<T>,
  opts: WaitForCompletionOptions = {},
): AsyncGenerator<T, T, void> {
  const intervalMs = opts.intervalMs ?? 2000;
  const timeoutMs = opts.timeoutMs ?? 5 * 60_000;
  const deadline = Date.now() + timeoutMs;

  let last: T | undefined;
  while (true) {
    if (opts.signal?.aborted) throw new DOMException("Polling aborted", "AbortError");
    if (Date.now() > deadline) throw new LiteGenPollingTimeoutError(id, last?.status);
    last = await getStatus(id);
    yield last;
    if (TERMINAL_STATUSES.has(last.status)) return last;
    await sleep(intervalMs, opts.signal);
  }
}

/** Resolve once any job reaches a terminal status, returning the final state. */
export async function waitForJob<T extends WithStatus>(
  id: string,
  getStatus: (id: string) => Promise<T>,
  opts: WaitForCompletionOptions = {},
): Promise<T> {
  let last: T | undefined;
  for await (const update of pollJob(id, getStatus, opts)) last = update;
  // pollJob always yields at least once before completing.
  return last as T;
}

export const pollVideo = (
  id: string,
  getStatus: (id: string) => Promise<VideoResponse>,
  opts?: WaitForCompletionOptions,
) => pollJob<VideoResponse>(id, getStatus, opts);

export const poll3d = (
  id: string,
  getStatus: (id: string) => Promise<Model3dResponse>,
  opts?: WaitForCompletionOptions,
) => pollJob<Model3dResponse>(id, getStatus, opts);

/** Back-compat alias — `waitForCompletion` predates the generic refactor. */
export const waitForCompletion = waitForJob;

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => resolve(), ms);
    signal?.addEventListener("abort", () => { clearTimeout(t); reject(new DOMException("Polling aborted", "AbortError")); }, { once: true });
  });
}
```

- [x] DONE 2026-08-27 **Step 5: Add `Model3dJob` and the namespace to `client.ts`**

After `VideosNamespace`:

```ts
type Model3dRequest = Schemas["Model3dGenerationRequest"];
type Model3dResponse = Schemas["Model3dGenerationResponse"];

/**
 * Handle returned by `models3d.generate`. 3D generation is always async, so
 * this is both awaitable and async-iterable — the same shape as {@link VideoJob}:
 *
 * ```ts
 * const mesh = await client.models3d.generate({ ... });
 * const url = mesh.assets?.find(a => a.kind === "mesh")?.url;
 *
 * for await (const update of client.models3d.generate({ ... })) {
 *   console.log(`${update.status} — ${update.progress}%`);
 * }
 * ```
 *
 * A completed job always carries exactly one `kind: "mesh"` asset with an
 * absolute URL; its absence means the generation failed.
 */
export class Model3dJob implements PromiseLike<Model3dResponse>, AsyncIterable<Model3dResponse> {
  private submittedPromise?: Promise<Model3dResponse>;

  constructor(
    private readonly submitFn: () => Promise<Model3dResponse>,
    private readonly pollFn: (id: string) => AsyncGenerator<Model3dResponse, Model3dResponse, void>,
  ) {}

  /** The initial submit response (id, status) — does not wait for completion. */
  get submitted(): Promise<Model3dResponse> {
    if (!this.submittedPromise) this.submittedPromise = this.submitFn();
    return this.submittedPromise;
  }

  then<TResult1 = Model3dResponse, TResult2 = never>(
    onfulfilled?: ((value: Model3dResponse) => TResult1 | PromiseLike<TResult1>) | null,
    onrejected?: ((reason: unknown) => TResult2 | PromiseLike<TResult2>) | null,
  ): Promise<TResult1 | TResult2> {
    return this.runToCompletion().then(onfulfilled, onrejected);
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<Model3dResponse, void, void> {
    const job = await this.submitted;
    yield* this.pollFn(job.id);
  }

  private async runToCompletion(): Promise<Model3dResponse> {
    const job = await this.submitted;
    let last: Model3dResponse = job;
    for await (const update of this.pollFn(job.id)) last = update;
    return last;
  }
}

class Models3dNamespace {
  constructor(private readonly client: LiteGenClient) {}
  /** Submit a 3D job. Awaitable (final state) and async-iterable (progress). */
  generate(req: Model3dRequest, opts?: WaitForCompletionOptions): Model3dJob {
    return new Model3dJob(
      () => this.client.request<Model3dResponse>("POST", "/v1/models3d/generations", req, opts?.signal),
      (id) => poll3d(id, (jid) => this.getStatus(jid, opts?.signal), opts),
    );
  }
  estimateCost(req: Model3dRequest, signal?: AbortSignal): Promise<CostEstimate> {
    return this.client.request("POST", "/v1/models3d/cost", req, signal);
  }
  getStatus(id: string, signal?: AbortSignal): Promise<Model3dResponse> {
    return this.client.request("GET", `/v1/models3d/${encodeURIComponent(id)}`, undefined, signal);
  }
  waitForCompletion(id: string, opts?: WaitForCompletionOptions): Promise<Model3dResponse> {
    return waitForJob(id, (jid) => this.getStatus(jid, opts?.signal), opts);
  }
  /** Stream progress updates: `for await (const u of client.models3d.poll(id))`. */
  poll(id: string, opts?: WaitForCompletionOptions): AsyncGenerator<Model3dResponse, Model3dResponse, void> {
    return poll3d(id, (jid) => this.getStatus(jid, opts?.signal), opts);
  }
}
```

Add `readonly models3d: Models3dNamespace;` to `LiteGenClient` (next to `videos`), initialise it in the constructor, and add `poll3d, waitForJob` to the `./polling` import.

- [x] DONE 2026-08-27 **Step 6: Export from `index.ts`**

Add `Model3dJob` to the `from "./client"` export list; add `poll3d, waitForJob` to the `from "./polling"` export; add `Model3d: "model3d",` to the `MediaType` const; and add the type aliases next to the video ones:
```ts
export type Model3dGenerationRequest = components["schemas"]["Model3dGenerationRequest"];
export type Model3dGenerationResponse = components["schemas"]["Model3dGenerationResponse"];
export type Model3dAsset = components["schemas"]["Model3dAsset"];
```

- [x] DONE 2026-08-27 **Step 7: Run to verify it passes**

Run: `cd sdks/typescript && npm run build && npm test 2>&1 | tail -20`
Expected: build clean, all tests PASS.

- [x] DONE 2026-08-27 **Step 8: Commit**

```bash
git add sdks/typescript/ apps/landing/public/openapi.json
git commit -m "feat(sdk): client.models3d and a generic pollJob backing every async family"
git push origin master
```

---

### Task 13: Dashboard — `<model-viewer>` component and the three-way media surfaces

**Status:** ✅ DONE (2026-08-27, 44d3c7b + f92e775) — task review clean

**Files:**
- Create: `dashboard/src/components/ModelViewer.tsx`
- Modify: `dashboard/src/pages/Generations.tsx:19-43` (`MediaPreview`), `dashboard/src/pages/Models.tsx:88` (filter), `dashboard/src/components/ModelDetail.tsx:7-8` (curl builder), `dashboard/src/components/TracePanel.tsx:220-235` (artifact drill-down)

**Interfaces:**
- Consumes: `Generation.media_type === 'model3d'`, `Generation.metadata.assets` (Task 10), the SDK's `Model3dAsset` type (Task 12).
- Produces: `<ModelViewer src={...} poster={...} />`; helper `meshUrl(g)` / `previewUrl(g)` exported from `ModelViewer.tsx`.

- [x] DONE 2026-08-27 **Step 1: Add the `<model-viewer>` dependency**

`<model-viewer>` ships as a self-registering custom element with no bundler-level three.js dependency:
```bash
cd dashboard && npm install @google/model-viewer
```
Confirm it is a runtime dependency, not a devDependency, in `dashboard/package.json`.

- [x] DONE 2026-08-27 **Step 2: Write the component**

`dashboard/src/components/ModelViewer.tsx`:

```tsx
import React, { useEffect, useState } from 'react';
import type { Generation } from '@litegen/sdk';

/** One entry of a 3D generation's re-hosted asset list (metadata.assets). */
interface Asset {
  kind: 'mesh' | 'texture' | 'preview';
  url: string;
  format: string;
}

function assets(g: Generation): Asset[] {
  const raw = (g.metadata as { assets?: unknown } | null | undefined)?.assets;
  return Array.isArray(raw) ? (raw as Asset[]) : [];
}

/** Primary mesh URL. Falls back to `result_url`, which the API also sets to the
 *  mesh, so a row written before assets[] existed still renders. */
export function meshUrl(g: Generation): string | null {
  return assets(g).find(a => a.kind === 'mesh')?.url ?? g.result_url ?? null;
}

/** 2D preview for thumbnail/grid contexts, where a live GL canvas per row would
 *  be wasteful. Null when the provider returned no preview. */
export function previewUrl(g: Generation): string | null {
  return assets(g).find(a => a.kind === 'preview')?.url ?? null;
}

/**
 * Lazily loads the `<model-viewer>` custom element the first time a 3D result is
 * actually shown, so the ~1MB WebGL bundle never lands on a dashboard session
 * that only looks at images.
 */
export default function ModelViewer({
  src,
  poster,
  testId,
  style,
}: {
  src: string;
  poster?: string | null;
  testId?: string;
  style?: React.CSSProperties;
}) {
  const [ready, setReady] = useState(() => !!customElements.get('model-viewer'));
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (ready) return;
    let cancelled = false;
    import('@google/model-viewer')
      .then(() => { if (!cancelled) setReady(true); })
      .catch(() => { if (!cancelled) setFailed(true); });
    return () => { cancelled = true; };
  }, [ready]);

  if (failed) {
    // A GL/bundle failure must not swallow the result — the mesh is still
    // downloadable, so degrade to a link rather than an empty box.
    return (
      <a href={src} data-testid={testId ? `${testId}-fallback` : undefined} style={{ color: '#58a6ff' }}>
        Download mesh
      </a>
    );
  }
  if (!ready) {
    return <span data-testid={testId ? `${testId}-loading` : undefined} style={{ color: '#8b949e' }}>loading viewer…</span>;
  }

  return React.createElement('model-viewer', {
    src,
    poster: poster ?? undefined,
    'camera-controls': true,
    'auto-rotate': true,
    'shadow-intensity': '1',
    'data-testid': testId,
    style: { width: '100%', height: 360, background: '#0d1117', borderRadius: 6, ...style },
  });
}
```

If the project's TS config rejects the unknown `model-viewer` intrinsic element, `React.createElement` (used above) sidesteps the JSX intrinsic check — no global type augmentation needed.

- [x] DONE 2026-08-27 **Step 3: Extend `MediaPreview` in `Generations.tsx`**

Replace the two-way `isVideo` switch (`dashboard/src/pages/Generations.tsx:19-43`) with a three-way one:

```tsx
import ModelViewer, { meshUrl, previewUrl } from '../components/ModelViewer';

/** Inline media for a generation: <img> for images, <video> for video,
 *  <model-viewer> for meshes. `thumb` renders the small in-row preview —
 *  always a flat 2D image, never a GL canvas per row. */
function MediaPreview({ g, thumb }: { g: Generation; thumb?: boolean }) {
  if (g.status !== 'completed' || !g.result_url) {
    return thumb ? <span style={{ color: '#8b949e' }}>—</span> : null;
  }
  const style = { maxWidth: '100%', maxHeight: 480, marginBottom: 12, display: 'block', borderRadius: 6 } as const;

  if (g.media_type === 'model3d') {
    const mesh = meshUrl(g);
    const poster = previewUrl(g);
    if (thumb) {
      return poster
        ? <img src={poster} alt="" loading="lazy" data-testid={`gen-thumb-${g.id}`}
               style={{ width: 48, height: 48, objectFit: 'cover', borderRadius: 4, display: 'block' }} />
        : <span title="3D result" aria-label="3d">🧊</span>;
    }
    return mesh
      ? <ModelViewer src={mesh} poster={poster} testId={`gen-media-${g.id}`} style={style} />
      : null;
  }

  const isVideo = g.media_type === 'video';
  if (thumb) {
    if (isVideo) return <span title="video result" aria-label="video">🎬</span>;
    return (
      <img src={g.result_url} alt="" loading="lazy" data-testid={`gen-thumb-${g.id}`}
           style={{ width: 48, height: 48, objectFit: 'cover', borderRadius: 4, display: 'block' }} />
    );
  }
  return isVideo
    ? <video controls src={g.result_url} data-testid={`gen-media-${g.id}`} style={style} />
    : <img src={g.result_url} alt={g.model} loading="lazy" data-testid={`gen-media-${g.id}`} style={style} />;
}
```

- [x] DONE 2026-08-27 **Step 4: Extend the remaining three surfaces**

`Models.tsx:88` — widen the filter and give `model3d` a readable label:
```tsx
            {['', 'image', 'video', 'model3d'].map(val => (
              <label key={val} …>
                <input type="radio" name="media_type" value={val}
                       checked={filterMediaType === val}
                       onChange={() => setFilterMediaType(val)} />
                {val === '' ? 'All' : val === 'model3d' ? '3D' : val}
              </label>
            ))}
```

`ModelDetail.tsx:7-8` — the curl builder's endpoint switch becomes three-way:
```tsx
const endpoint =
  model.media_type === 'model3d' ? '/v1/models3d/generations'
  : model.media_type === 'video' ? '/v1/videos/generations'
  : '/v1/images/generations';
```
Read the surrounding lines before editing — if the file uses an `isVideo` boolean threaded through several places, replace it with this single `endpoint` const rather than adding a second boolean.

`TracePanel.tsx:220-235` — add a 3D branch before the video branch in the `output_kind === 'url'` block:
```tsx
    if (artifact.media_type === 'model3d') {
      return (
        <ModelViewer
          src={artifact.output_value}
          testId="trace-visual-3d"
          style={{ maxHeight: 360, border: '1px solid #30363d' }}
        />
      );
    }
```

- [x] DONE 2026-08-27 **Step 5: Verify the build and typecheck**

Run: `cd dashboard && npm run build 2>&1 | tail -20`
Expected: clean. A TS error about the `model-viewer` element means Step 2's `React.createElement` was replaced with JSX — revert to `createElement`.

- [x] DONE 2026-08-27 **Step 6: Commit**

```bash
git add dashboard/src/components/ModelViewer.tsx dashboard/src/pages/Generations.tsx \
        dashboard/src/pages/Models.tsx dashboard/src/components/ModelDetail.tsx \
        dashboard/src/components/TracePanel.tsx dashboard/package.json dashboard/package-lock.json
git commit -m "feat(dashboard): model-viewer previews across the gallery, models, and traces"
git push origin master
```

---

### Task 14: Playground — async job state machine and `ResultTile3D`

**Status:** ✅ DONE (2026-08-27, bd49c9a + f92e775) — task review clean

**Files:**
- Modify: `dashboard/src/playground/types.ts`, `useFanOut.ts`, `SingleMode.tsx`, `ResultGrid.tsx`
- Create: `dashboard/src/playground/ResultTile3D.tsx`

**Interfaces:**
- Consumes: `client.models3d` (Task 12), `ModelViewer` (Task 13).
- Produces: `ResultTileState.mediaType`, `TileStatus` gains `'polling'`, `ResultTileState.assets`.

**This is the one genuinely new piece.** The Playground has no polling machinery at all — `TileStatus` is `'queued' | 'running' | 'done' | 'error'`, all client-side, because image generation returns its result from the same request. 3D needs a real pending → poll → done path. Reuse the SDK's `client.models3d.poll` as the polling primitive so it is not hand-rolled a second time.

- [x] DONE 2026-08-27 **Step 1: Extend the tile types**

`dashboard/src/playground/types.ts`:
```ts
import type { ParamSpec, ImageGenerationRequest, Model3dGenerationRequest, Model3dAsset } from '@litegen/sdk';

// … MergedParam, SharedFormState unchanged …

/** `polling` is distinct from `running`: the request has been ACCEPTED and we
 *  are now waiting on a server-side job, so the tile can show real progress. */
export type TileStatus = 'queued' | 'running' | 'polling' | 'done' | 'error';

/** One result cell (model × index). */
export interface ResultTileState {
  key: string;                         // `${modelId}#${index}`
  modelId: string;
  index: number;
  status: TileStatus;
  /** Which family this tile is rendering — decides the tile component. */
  mediaType: 'image' | 'model3d';
  request: ImageGenerationRequest | Model3dGenerationRequest;
  b64_json?: string | null;
  url?: string | null;
  /** 3D only: the re-hosted asset list from the completed job. */
  assets?: Model3dAsset[];
  /** 0–100, populated while `polling`. */
  progress?: number;
  costUsd?: number;
  latencyMs?: number;
  error?: string;
}
```

Every existing `ResultTileState` literal now needs `mediaType: 'image'` — the compiler will name them.

- [x] DONE 2026-08-27 **Step 2: Add the 3D branch to `useFanOut`**

In `dashboard/src/playground/useFanOut.ts`, split the worker body on media type. The image path is unchanged; the 3D path submits then polls:

```ts
        if (mediaType === 'model3d') {
          keys.forEach(k => patch(k, { status: 'running' }));
          const started = performance.now();
          try {
            const job = client.models3d.generate(request as Model3dGenerationRequest, {
              signal: ctrl.signal,
              intervalMs: 1500,
            });
            const submitted = await job.submitted;
            keys.forEach(k => patch(k, { status: 'polling', progress: submitted.progress ?? 0 }));

            let final = submitted;
            for await (const update of client.models3d.poll(submitted.id, {
              signal: ctrl.signal,
              intervalMs: 1500,
            })) {
              final = update;
              keys.forEach(k => patch(k, { progress: update.progress ?? 0 }));
            }

            const latency = Math.round(performance.now() - started);
            if (final.status !== 'completed') {
              keys.forEach(k => patch(k, {
                status: 'error',
                error: final.error ?? `job ${final.status}`,
                latencyMs: latency,
              }));
            } else {
              const mesh = final.assets?.find(a => a.kind === 'mesh');
              keys.forEach(k => patch(k, {
                status: 'done',
                latencyMs: latency,
                costUsd: final.usage?.cost_usd,
                progress: 100,
                assets: final.assets ?? [],
                // A completed 3D job always carries a mesh; its absence is a failure.
                url: mesh?.url ?? null,
                ...(mesh ? {} : { status: 'error' as const, error: 'completed without a mesh asset' }),
              }));
            }
          } catch (e) {
            if (ctrl.signal.aborted) return;
            keys.forEach(k => patch(k, { status: 'error', error: (e as Error).message }));
          }
          continue;
        }
```

Derive `mediaType` from the request list — change `run`'s parameter to `Array<{ modelId: string; mediaType: 'image' | 'model3d'; request: ImageGenerationRequest | Model3dGenerationRequest }>` and set `mediaType` on each initial tile.

- [x] DONE 2026-08-27 **Step 3: Write `ResultTile3D`**

`dashboard/src/playground/ResultTile3D.tsx` — same head/meta/rerun chrome as `ResultTile.tsx`, mesh body:

```tsx
import ModelViewer from '../components/ModelViewer';
import type { ResultTileState } from './types';

interface Props { tile: ResultTileState; onRerun: (modelId: string) => void; }

export default function ResultTile3D({ tile, onRerun }: Props) {
  const mesh = tile.assets?.find(a => a.kind === 'mesh')?.url ?? tile.url ?? undefined;
  const poster = tile.assets?.find(a => a.kind === 'preview')?.url ?? undefined;
  return (
    <div className="pg-tile" data-testid={`pg-tile-${tile.modelId}`}>
      <div className="pg-tile-head">
        <code className="pg-tile-model">{tile.modelId}</code>
        <button className="btn btn-secondary pg-tile-rerun" title="Rerun this model"
          data-testid={`pg-tile-rerun-${tile.modelId}`} onClick={() => onRerun(tile.modelId)}>↻</button>
      </div>
      <div className="pg-tile-image">
        {tile.status === 'queued' || tile.status === 'running' ? (
          <span className="pg-tile-status" data-testid={`pg-tile-spinner-${tile.modelId}`}>⟳ submitting…</span>
        ) : tile.status === 'polling' ? (
          <span className="pg-tile-status" data-testid={`pg-tile-progress-${tile.modelId}`}>
            ⟳ generating… {tile.progress ?? 0}%
          </span>
        ) : tile.status === 'error' ? (
          <span className="pg-tile-error" data-testid={`pg-tile-error-${tile.modelId}`}>⚠ {tile.error}</span>
        ) : mesh ? (
          <ModelViewer src={mesh} poster={poster} testId={`pg-tile-mesh-${tile.modelId}`} style={{ height: 240 }} />
        ) : (
          <span className="pg-tile-status">no mesh</span>
        )}
      </div>
      <div className="pg-tile-meta">
        {tile.costUsd != null && <span>${tile.costUsd.toFixed(3)}</span>}
        {tile.latencyMs != null && <span>{tile.latencyMs}ms</span>}
      </div>
    </div>
  );
}
```

In `ResultGrid.tsx`, pick the component per tile:
```tsx
{tiles.map(t => t.mediaType === 'model3d'
  ? <ResultTile3D key={t.key} tile={t} onRerun={onRerun} />
  : <ResultTile key={t.key} tile={t} onRerun={onRerun} />)}
```

- [x] DONE 2026-08-27 **Step 4: Widen the Playground's model filter**

`SingleMode.tsx:50-51` currently pins the Playground to mock image models — a deliberate free, deterministic sandbox. Keep that constraint and just admit the 3D mocks:
```tsx
        const playable = allModels.filter(
          m => m.provider === 'mock' && (m.media_type === 'image' || m.media_type === 'model3d'),
        );
```
Leave the default-model preference (`mock/visual-image-gen`) as it is, so the Playground's first paint does not change. Apply the same filter change in `CompareMode.tsx` if it duplicates the predicate — `grep -n "media_type === 'image'" dashboard/src/playground/*.tsx`.

- [x] DONE 2026-08-27 **Step 5: Verify the build**

Run: `cd dashboard && npm run build 2>&1 | tail -20`
Expected: clean.

- [x] DONE 2026-08-27 **Step 6: Commit**

```bash
git add dashboard/src/playground/
git commit -m "feat(playground): async job polling and a mesh result tile"
git push origin master
```

---

### Task 15: Playwright coverage for the 3D surfaces

**Files:**
- Create: `dashboard/tests/model3d.spec.ts` (match the existing spec directory — check `dashboard/playwright.config.ts` for `testDir`)

**Interfaces:**
- Consumes: everything above, running against a live litegen with the mock providers.

**Why this is the only gate on the TS `MediaType` copies:** the two Rust `MediaType` enums are compiler-enforced, but `apps/landing/src/config/models.generated.ts` and the dashboard's string comparisons compile fine with a missed spot. This spec is what catches that, and it is also what proves the mock GLB actually loads in three.js (`<model-viewer>` uses `GLTFLoader` internally).

- [ ] **Step 1: Write the spec**

Read an existing spec first (`ls dashboard/tests/`) and copy its login/setup fixture verbatim — do not invent a new auth path.

```ts
import { expect, test } from '@playwright/test';

test.describe('3D model generation', () => {
  test('Playground runs a mock 3D model through pending → polling → done', async ({ page }) => {
    await page.goto('/playground');

    // The 3D mock must be offerable — this is also the guard on the model list
    // exposing media_type "model3d" correctly through the whole chain.
    await page.getByTestId('pg-model-picker').click();
    await page.getByText('mock/mesh-3d').click();
    await page.getByTestId('pg-prompt').fill('a low-poly fox');
    await page.getByTestId('pg-run').click();

    // The tile must pass through a real polling state — the Playground had no
    // async path at all before 3D, so this is the assertion that it exists.
    await expect(page.getByTestId('pg-tile-progress-mock/mesh-3d')).toBeVisible({ timeout: 15_000 });

    // …and then land on a rendered mesh.
    const viewer = page.getByTestId('pg-tile-mesh-mock/mesh-3d');
    await expect(viewer).toBeVisible({ timeout: 30_000 });

    const src = await viewer.getAttribute('src');
    expect(src, 'mesh src must be absolute — a relative URL is a contract violation').toMatch(/^https?:\/\//);
    expect(src).toMatch(/\.glb$/);

    // model-viewer only fires `load` once GLTFLoader has parsed the file, so
    // this is the real proof the mock GLB is valid, not just well-formed bytes.
    await expect
      .poll(async () => viewer.evaluate((el: HTMLElement & { loaded?: boolean }) => el.loaded === true), {
        timeout: 30_000,
      })
      .toBe(true);
  });

  test('Generations gallery renders a mesh row with a viewer and a 2D thumb', async ({ page }) => {
    await page.goto('/generations');

    const row = page.locator('tr', { hasText: 'mock/mesh-3d' }).first();
    await expect(row).toBeVisible({ timeout: 30_000 });

    // Thumb context stays a flat image — never a GL canvas per row.
    await expect(row.locator('img')).toBeVisible();

    // Expanding the row shows the interactive viewer.
    await row.click();
    const media = page.locator('[data-testid^="gen-media-"]').first();
    await expect(media).toBeVisible({ timeout: 15_000 });
    expect(await media.evaluate(el => el.tagName.toLowerCase())).toBe('model-viewer');
  });

  test('Models page filters to the 3D family', async ({ page }) => {
    await page.goto('/models');
    await page.getByTestId('models-filter-media-type').getByRole('radio', { name: '3D' }).check();
    await expect(page.getByText('mock/mesh-3d')).toBeVisible();
    await expect(page.getByText('mock/visual-image-gen')).toHaveCount(0);
  });

  test('a failing 3D model surfaces its error rather than a blank tile', async ({ page }) => {
    await page.goto('/playground');
    await page.getByTestId('pg-model-picker').click();
    await page.getByText('mock/fail-3d').click();
    await page.getByTestId('pg-prompt').fill('anything');
    await page.getByTestId('pg-run').click();

    const err = page.getByTestId('pg-tile-error-mock/fail-3d');
    await expect(err).toBeVisible({ timeout: 30_000 });
    await expect(err).toContainText('fail');
  });
});
```

Adjust every `data-testid` to the ones the Playground actually uses — read `SingleMode.tsx` / `CompareMode.tsx` and use the real ids rather than the ones guessed here. If a needed id is missing, add it in the component (that is a legitimate part of this task).

- [ ] **Step 2: Run to verify it fails, then passes**

Run: `cd dashboard && npx playwright test model3d 2>&1 | tail -30`

Expected on first run: failures pointing at missing test ids or a not-yet-running backend. Start the stack the way the other specs do (check `dashboard/playwright.config.ts`'s `webServer`), fix the ids, and re-run until green.

- [ ] **Step 3: Run the whole suite**

Run: `cd dashboard && npx playwright test 2>&1 | tail -20`
Expected: no regressions in the existing specs.

- [ ] **Step 4: Commit**

```bash
git add dashboard/tests/model3d.spec.ts dashboard/src/playground/
git commit -m "test(e2e): 3D playground polling, gallery viewer, filter, and failure paths"
git push origin master
```

---

### Task 16: Landing generated config

**Status:** ✅ DONE (2026-08-27, 6b927b8) — task review clean 2026-09-11 (reviewed on the diff alone; the implementer's session ended before it wrote a report)

**Files:**
- Modify: `apps/landing/scripts/derive-models.mjs`, `apps/landing/scripts/derive-capabilities.mjs`
- Regenerate: `apps/landing/src/config/models.generated.ts`, `apps/landing/src/components/infra-flow/capabilities.generated.ts`

**Interfaces:**
- Consumes: `GET /v1/models` with `media_type: "model3d"` (Task 10).
- Produces: `mediaType: 'image' | 'video' | 'model3d'`, `outputs: { image, video, model3d }`.

- [x] DONE 2026-08-27 **Step 1: Extend `derive-models.mjs`**

At `:84-91`, add the 3D capability booleans alongside the existing ones:
```js
    mediaType: m.media_type,
    output: m.media_type,
    capabilities: {
      textToImage: caps.text_to_image === true,
      imageToImage: caps.image_to_image === true,
      // …
      textToVideo: caps.text_to_video === true,
      imageToVideo: caps.image_to_video === true,
      textTo3d: caps.supports_text_to_3d === true,
      imageTo3d: caps.supports_image_to_3d === true,
      multiviewTo3d: caps.supports_multiview_to_3d === true,
    },
```
Widen the emitted TS types at `:141-148`:
```js
  mediaType: 'image' | 'video' | 'model3d';
  output: 'image' | 'video' | 'model3d';
```
and add `textTo3d: boolean; imageTo3d: boolean; multiviewTo3d: boolean;` to the emitted `capabilities` interface.

At `:179-182`, widen the per-provider bucketing:
```js
    const p = (out[m.provider] ??= { image: [], video: [], model3d: [] });
    const name = /* unchanged */;
    if (m.output === 'video') p.video.push(name);
    else if (m.output === 'model3d') p.model3d.push(name);
    else p.image.push(name);
```
and the emitted type at `:195-196` gains `model3d: string[];`.

Read the file before editing — the line numbers above are from the current revision and the surrounding code may differ.

- [x] DONE 2026-08-27 **Step 2: Extend `derive-capabilities.mjs`**

At `:18-46`, carry a third output flag through the accumulator:
```js
  const v = acc.get(m.provider) ?? { text: false, image: false, multi: false, hasImage: false, hasVideo: false, has3d: false };
  if (cap.supports_text_to_image || cap.supports_text_to_video || cap.supports_text_to_3d) v.text = true;
  // …
  if (m.media_type === 'video') v.hasVideo = true;
  if (m.media_type === 'image') v.hasImage = true;
  if (m.media_type === 'model3d') v.has3d = true;
```
and in the emit:
```js
      outputs: { image: v.hasImage || !anyOut, video: v.hasVideo, model3d: v.has3d },
```
Update `anyOut` to consider `has3d`, the emitted `OutputModalities` typedef (`:13`), the emitted TS interface (`:67-69`), the `serialize` template (`:59`), and the hand-written fallback table (`:86-93`) — every literal there needs `model3d: false` so the committed snapshot stays valid if the gateway is unreachable at build time. Fail-open behaviour is otherwise unchanged.

- [x] DONE 2026-08-27 **Step 3: Regenerate and inspect the diff**

Start a local litegen with the mock providers, then:
```bash
cd apps/landing && node scripts/derive-models.mjs && node scripts/derive-capabilities.mjs
git diff --stat apps/landing/src/config/models.generated.ts apps/landing/src/components/infra-flow/capabilities.generated.ts
```
Expected: the mock 3D models appear with `mediaType: 'model3d'`, and every provider entry gains `model3d: false` except `mock`.

Note: `models.generated.ts` and `capabilities.generated.ts` already show as modified in the working tree from earlier work — inspect `git diff` and keep those changes; do not let a regeneration silently revert them.

- [x] DONE 2026-08-27 **Step 4: Verify the landing build**

Run: `cd apps/landing && npm run build 2>&1 | tail -20`
Expected: clean.

- [x] DONE 2026-08-27 **Step 5: Commit**

```bash
git add apps/landing/scripts/ apps/landing/src/config/models.generated.ts \
        apps/landing/src/components/infra-flow/capabilities.generated.ts
git commit -m "feat(landing): carry the model3d family through the generated config"
git push origin master
```

---

### Task 17: Polish, cross-workspace verification, and doc reconciliation

**Files:**
- Modify: `docs/superpowers/specs/2026-08-19-3d-model-generation-design.md` (record the contract reconciliation)
- Modify: `docs/superpowers/plans/2026-08-20-3d-model-generation.md` (this file — mark every task done)
- Modify: `README.md`, `litegen.example.yaml` (if either enumerates supported modalities)

- [ ] **Step 1: Run every gate**

```bash
cd litegen-core && cargo test 2>&1 | tail -20
cd litegen-core && cargo clippy --all-targets -- -D warnings 2>&1 | tail -30
cd sdks/typescript && npm run build && npm test 2>&1 | tail -20
cd dashboard && npm run build && npx playwright test 2>&1 | tail -20
cd apps/landing && npm run build 2>&1 | tail -10
```
All five must be clean. Do not proceed past a failure — fix it.

- [ ] **Step 2: Reconcile the design doc with what shipped**

The design doc is still marked `draft (awaiting review)` and specifies a contract that was superseded. Edit its header and add a reconciliation note — do NOT rewrite the file wholesale:

```markdown
**Status:** phase 1 implemented 2026-08-20 (foundation + mock). Phase 2 (Meshy, Tripo3D, Stability, Rodin) open.

> **Contract reconciliation (2026-08-20).** aipix was already coded against
> `docs/litegen-3d-specs.md` when this was implemented, so that document won on
> every wire-facing decision and this one won on internal architecture. Three
> changes from §3, §6, and §8 below:
>
> - Wire media type is **`model3d`**, not `model_3d`. (§3.1's claim that
>   `Model3d` serializes as `"model_3d"` is simply wrong — serde's snake_case
>   does not insert an underscore before a digit.)
> - Routes are **`/v1/models3d/…`**, not `/v1/3d/…` (§6.1).
> - The response carries **`assets: Model3dAsset[]`**, not
>   `model_url`/`thumbnail_url` with extras in `metadata` (§3.2). One asset is
>   always `kind: "mesh"`; texture and preview files are siblings rather than a
>   separate map.
> - 3D params are **first-class `ParamSpec` entries**, reversing decision 6 —
>   aipix renders one control per `params` entry, so `extra_allowlist`
>   passthrough would have left every 3D knob unrenderable.
>
> Everything else — the `Model3dProvider` trait shape, byte-carrying poll
> results, `ImageStorage`-not-`ImageStore` (§5.2), same-tick re-hosting, the
> poller's media-type branch — shipped as designed.
```

- [ ] **Step 3: Verify the aipix acceptance checklist end to end**

Against a running local litegen with the mock providers:
```bash
BASE=http://localhost:8080
KEY=sk_...   # a local key

curl -s "$BASE/v1/models?media_type=model3d" -H "Authorization: Bearer $KEY" | jq '.data[].id'
curl -s "$BASE/v1/models/mock/all-params-3d" -H "Authorization: Bearer $KEY" | jq '.params | keys'
curl -s -X POST "$BASE/v1/models3d/cost" -H "Authorization: Bearer $KEY" \
  -H 'content-type: application/json' -d '{"model":"mock/mesh-3d","prompt":"a fox"}' | jq
ID=$(curl -s -X POST "$BASE/v1/models3d/generations" -H "Authorization: Bearer $KEY" \
  -H 'content-type: application/json' -d '{"model":"mock/mesh-3d","prompt":"a fox"}' | jq -r .id)
sleep 8
curl -s "$BASE/v1/models3d/$ID" -H "Authorization: Bearer $KEY" | jq '{status, assets}'
MESH=$(curl -s "$BASE/v1/models3d/$ID" -H "Authorization: Bearer $KEY" | jq -r '.assets[] | select(.kind=="mesh") | .url')
curl -sI "$MESH" | head -3
curl -s "$MESH" | head -c 4 | xxd   # expect: 676c 5446  ("glTF")

# strict:false must drop, not error, and report the header:
curl -si -X POST "$BASE/v1/models3d/generations" -H "Authorization: Bearer $KEY" \
  -H 'content-type: application/json' \
  -d '{"model":"mock/mesh-3d","prompt":"a fox","strict":false,"rig":true}' \
  | grep -i 'x-litegen-dropped-params\|HTTP/'

# cancellation on the shared endpoint:
ID2=$(curl -s -X POST "$BASE/v1/models3d/generations" -H "Authorization: Bearer $KEY" \
  -H 'content-type: application/json' -d '{"model":"mock/mesh-3d","prompt":"x"}' | jq -r .id)
curl -s -X PATCH "$BASE/v1/generations/$ID2" -H "Authorization: Bearer $KEY" \
  -H 'content-type: application/json' -d '{"status":"cancelled"}' | jq '.status'
```
Tick each box in `~/source/repos/aipix/docs/litegen-3d-specs.md` §7 as it is verified, and record any item that does NOT pass rather than quietly moving on.

See also `docs/superpowers/param-mapping-matrix.md` (Task 18) for the per-param
request-flow checklist — the tickable record of which mappings are actually
proven by a named test, for every family including this one.

- [ ] **Step 4: Mark this plan complete**

Tick every `- [ ]` in this file as `- [x] DONE 2026-08-20` with the shipping commit subject, in the same commit that ships Step 5. A stale open item that actually shipped is as bad as an unmarked one.

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/
git commit -m "docs(3d): reconcile the design with the shipped contract and close out phase 1"
git push origin master
git status -sb   # must show no "ahead"
```

- [ ] **Step 6: Tell aipix its short-circuit can go**

Report to the user (do not edit the aipix repo as part of this plan):
- `mock/mesh-3d` now exists in litegen with the same id aipix seeds locally, returning a real GLB over the real API.
- `apps/api/src/modules/litegen/litegen-model3d.generator.ts`'s `mock/*` short-circuit and the bundled `apps/api/mock-gen-mesh.glb` can be deleted.
- `Litegen3dHttpClient` can be replaced with `client.models3d` from `litegen-sdk` once the SDK version is bumped and published.

---

## Self-Review

**Spec coverage.** Every section of the aipix contract maps to a task: §1 types → Tasks 1–2; §2 endpoints → Task 10; §3 request/response and the `strict:false` requirement → Tasks 2, 8, 10; §4 `ModelSchema` + `label`/`description` → Tasks 1, 5; §5 SDK surface → Task 12; §6 mock provider → Tasks 4–5; §7 acceptance checklist → Task 17 Step 3. From the litegen design doc: §3 data model → Tasks 1–2; §4 provider abstraction → Tasks 3, 6; §5 storage → Task 7; §6 async/poller → Tasks 9–11; §7 dashboard → Tasks 13–15; §8 SDK + landing → Tasks 12, 16.

**Deliberately out of scope**, per the answered scope question: `providers/model3d/{meshy,tripo3d,stability,rodin}.rs`, their `models/*.yaml`, their wiremock and `#[ignore]` live tests, and any routing-strategy dispatch for 3D. `MODEL3D_PROVIDERS` and `build_model3d_provider` are shaped so each is an additive match arm.

**Known deviations from the design doc**, all recorded in Task 17 Step 2: wire media type, route paths, response shape, and first-class params.

**Two things an executor must not skip:**
1. `ProxyRouter::new` and `poll_once` both gain a parameter. Find every call site with `grep -rn` before editing — several live in test harnesses.
2. `resolve_app_model3d_store` is written against `&AppState` in Task 10 but called from the poller in Task 11, which has no `AppState`. Task 11 Step 3 refactors it to take `(db, secrets_key, app_id)`; do that refactor rather than duplicating the decrypt logic.

---

# Plan Amendment — 2026-08-20

Added after the user reviewed progress at Task 4 and asked three questions the
original plan did not answer. Their decisions:

1. **3D vendor adapters stay out of this pass.** Meshy / Tripo3D / Stability /
   Rodin remain a second wave on the proven trait, once live keys can settle the
   field-name caveats recorded in spec §9. Tasks 1–17 are unchanged in scope.
2. **Add a param-mapping matrix** (Task 18) — the tickable request-flow /
   param-mapping artifact that did not exist.
3. **Extend `catalog_conformance` to 3D** (Task 19).
4. **Add Storybook to the dashboard** (Task 20) — net-new tooling.

## Correction to Task 15 (binding)

Task 15 says to create `dashboard/tests/model3d.spec.ts` and to check
`playwright.config.ts` for `testDir`. That directory does not exist. Verified
actual layout:

- `dashboard/playwright.config.ts` → `testDir: './e2e'`, `baseURL:
  http://127.0.0.1:5174/`, and a `webServer` array that boots the **release**
  binary at `../litegen-core/target/release/litegen` plus Vite.
- Existing specs: `dashboard/e2e/{compare-playground,god-test}.spec.ts`,
  `dashboard/e2e-live/{authgate,live}.spec.ts`,
  `dashboard/e2e-mt/multitenant.spec.ts`.
- Scripts: `npm run test:e2e` (default config), `test:e2e:mt` (multitenant).

**Task 15's spec file is `dashboard/e2e/model3d.spec.ts`.** It must be runnable
by the existing `npm run test:e2e`, which requires a release build of the
gateway (`cargo build --release -p litegen`) before the run.

---

### Task 18: Param-mapping matrix — the request-flow validation checklist

**Status:** ✅ DONE (2026-08-27, 02715ec) — task review clean

**Files:**
- Create: `docs/superpowers/param-mapping-matrix.md`
- Modify: `docs/superpowers/plans/2026-08-20-3d-model-generation.md` (link it from Task 17's checklist)

**Interfaces:**
- Consumes: `models/*.yaml` (the advertised surface), `src/api/middleware/validator.rs` (the validation rules), each `src/providers/**/*.rs` (the outbound field names).
- Produces: a living document with one tickable row per (param, model family) pair.

**Why:** nothing in this repo answers "where does `target_polycount` go, what
validates it, and what proves the mapping is right?" without reading three files.
The matrix is that answer, and it is where a reviewer ticks off a verified flow.

- [x] DONE 2026-08-27 **Step 1: Generate the raw inventory (do not hand-transcribe)**

Hand-written tables rot. Derive the starting data:

```bash
cd /Users/joeviscardi/source/repos/litegen
# Every param every model advertises, with its kind:
python3 - <<'PY'
import glob, yaml, collections
rows = collections.defaultdict(set)
for f in sorted(glob.glob('models/*.yaml')):
    doc = yaml.safe_load(open(f))
    for m in doc.get('models') or []:
        for name, spec in (m.get('params') or {}).items():
            rows[(m['media_type'], name, spec.get('kind'))].add(m['id'])
for (media, name, kind), ids in sorted(rows.items()):
    print(f"{media}\t{name}\t{kind}\t{len(ids)}\t{sorted(ids)[0]}")
PY
```

Run it and keep the output — it is the matrix's spine.

- [x] DONE 2026-08-27 **Step 2: Write the document**

`docs/superpowers/param-mapping-matrix.md`. Structure:

```markdown
# Param mapping matrix

**What this is:** for every parameter litegen advertises, where it comes from,
what validates it, what it becomes on the wire to each provider, and what test
proves that. Tick a row only when you have SEEN the mapping work — a green unit
test on the validator is not evidence the provider sends the right field name.

**Legend:** ✅ verified (test named) · 🟡 implemented, unverified against a live
vendor · ⬜ not implemented · — not applicable to this family

## How a request flows

  POST /v1/{family}/generations
    → Validated{Image,Video,Model3d} extractor        (validator.rs)
        · deserialize into the family's request struct
        · check_prompt / per-param spec check
        · strict=true  → 4xx `param_unsupported`
          strict=false → drop + record in X-Litegen-Dropped-Params
        · check_refs   (roles vs. ref_inputs)
        · check_extra  (keys vs. extra_allowlist)
    → materializer   (ref images → url | base64 | multipart per provider_format)
    → {Image,Video,Model3d}Extras                     (handlers/mod.rs)
    → router dispatch → provider.generate(schema, base, extras, materialized)
    → provider maps extras → the vendor's own field names   ← THE MAPPING
    → async families: handle stored, poller drives to terminal

## 3D (`model3d`)

| litegen param | ParamSpec | Validator rule | Meshy | Tripo3D | Stability | Rodin | Proof |
|---|---|---|---|---|---|---|---|
| `prompt` | PromptSpec | required, length | `prompt` | `prompt` | — (image only) | — | ⬜ |
| `output_format` | String enum | enum_values | `model_urls.<fmt>` (response-side) | `quad`→FBX else GLB | fixed GLB | `format` | ⬜ |
| `texture` | Bool | supported-or-drop | `should_texture` | `texture` | — | `material` | ⬜ |
| `pbr` | Bool | supported-or-drop | (texture_* maps) | `pbr` | — | `material=PBR` | ⬜ |
| `target_polycount` | Int 100–300000 | range | `target_polycount` | `face_limit` | `vertex_count` | `quality` | ⬜ |
| `symmetry` | String enum | enum off/auto/on | `symmetry_mode` | — | — | — | ⬜ |
| `topology` | String enum | enum triangle/quad | `topology` | `quad` (bool) | `remesh` | `mesh_mode` | ⬜ |
| `rig` | Bool | supported-or-drop | (separate endpoint) | `rig` | — | — | ⬜ |
| `seed` | Seed | range | — | `model_seed` | `seed` | — | ⬜ |
| ref role `init` | RefInputSpec | role declared | `image_url` (b64 ok) | `file_token`\|url | multipart `image` | multipart `images` | ⬜ |
| ref roles `view-*` | RefInputSpec | role declared | multi-image-to-3d | multiview-to-model | — | `condition_mode` | ⬜ |

Vendor columns are transcribed from the design spec §9 and are **unverified**
against live APIs — that is exactly why every 3D row starts ⬜. Do not upgrade a
cell to ✅ on documentation alone.

## Image / Video

[One row per param from Step 1's inventory. Columns: param, ParamSpec kind,
validator rule, how many models declare it, which test asserts it. Existing
coverage: `tests/catalog_conformance.rs` asserts advertised enum values against
vendor docs; `src/api/middleware/validator_tests.rs` asserts the validation
rules; per-provider `#[cfg(test)] mod tests` assert outbound request shape.]

## Verification recipe

For ONE (param, provider) cell, ✅ requires all four:
1. The model YAML advertises it with the right `ParamSpec` kind.
2. A validator test asserts both strict-reject and lax-drop for it.
3. A provider test (wiremock or unit) asserts the OUTBOUND field name and value.
4. `catalog_conformance` asserts any enum values against the vendor's documented set.

Name the test in the cell, e.g. `✅ meshy::tests::maps_topology`.
```

Fill the Image/Video section from Step 1's real output — every row, not a sample.

- [x] DONE 2026-08-27 **Step 3: Tick what today's code already earns**

Walk the existing validator tests and per-provider test modules and mark the
image/video rows they genuinely cover, naming each test. Leave everything
unproven as ⬜ or 🟡. Resist the urge to mark a row ✅ because the code "looks
right" — the whole value of this document is that its ticks mean something.

- [x] DONE 2026-08-27 **Step 4: Commit**

```bash
git add docs/superpowers/param-mapping-matrix.md docs/superpowers/plans/2026-08-20-3d-model-generation.md
git commit -m "docs: param mapping matrix — request flow and per-provider field mapping"
```

---

### Task 19: Extend `catalog_conformance` to the 3D family

**Status:** ✅ DONE (2026-08-27, 89a65e1) — task review clean

**Files:**
- Modify: `litegen-core/tests/catalog_conformance.rs`

**Interfaces:**
- Consumes: `CapabilityRegistry::from_dir(models/)`, `capabilities::MediaType::Model3d`, the 3D `ParamSpec` entries from Task 1, the mock 3D models from Task 5.
- Produces: structural conformance assertions covering every `media_type: model3d` model.

**Scope honesty:** with no vendor adapters shipped, there are no vendor model ids
to assert against vendor docs. What this task CAN assert — and what genuinely
guards the surface — is that the catalog never advertises a 3D shape the family's
own contract forbids, and that the enum vocabularies match the values recorded in
design spec §9. Those assertions keep working unchanged when real vendor rows
land; they simply gain more subjects.

- [x] DONE 2026-08-27 **Step 1: Write the failing tests**

Append to `litegen-core/tests/catalog_conformance.rs`:

```rust
// ─── 3D family conformance ──────────────────────────────────────────────────
//
// No vendor 3D adapters ship yet, so these assert the family's structural
// contract across every advertised model3d row rather than vendor-specific ids.
// They gain subjects, not new code, when Meshy/Tripo3D land.

use litegen::capabilities::MediaType;

fn model3d_models(reg: &CapabilityRegistry) -> Vec<&litegen::capabilities::ModelSchema> {
    reg.all().filter(|m| m.media_type == MediaType::Model3d).collect()
}

#[test]
fn every_3d_model_declares_at_least_one_generation_mode() {
    let reg = registry();
    let models = model3d_models(&reg);
    assert!(!models.is_empty(), "the catalog advertises no model3d models");
    for m in models {
        let c = &m.capabilities;
        assert!(
            c.text_to_3d || c.image_to_3d || c.multiview_to_3d,
            "{} is media_type model3d but advertises no 3D generation mode — \
             a client filtering on capabilities would never offer it",
            m.id
        );
    }
}

#[test]
fn every_3d_model_advertising_image_modes_declares_the_matching_ref_roles() {
    // Mode is INFERRED from reference roles, never passed explicitly, so a model
    // claiming image_to_3d with no `init` role can never actually be driven in
    // that mode.
    let reg = registry();
    for m in model3d_models(&reg) {
        if m.capabilities.image_to_3d {
            let roles = m.ref_inputs.as_ref().map(|r| &r.roles);
            assert!(
                roles.is_some_and(|r| r.contains_key("init")),
                "{} advertises image_to_3d but declares no `init` ref role",
                m.id
            );
        }
        if m.capabilities.multiview_to_3d {
            let roles = m.ref_inputs.as_ref().expect("multiview model needs ref_inputs");
            let views = roles.roles.keys().filter(|k| k.starts_with("view-")).count();
            assert!(
                views >= 2,
                "{} advertises multiview_to_3d but declares {} view-* roles; \
                 multiview needs at least two",
                m.id, views
            );
        }
    }
}

#[test]
fn 3d_enum_vocabularies_match_the_documented_vendor_sets() {
    // Values transcribed from the design spec §9 vendor tables. If a future
    // catalog row advertises something outside these, either the vendor moved
    // or the row is wrong — either way, re-verify before widening this list.
    let reg = registry();
    for m in model3d_models(&reg) {
        if let Some(ParamSpec::String(s)) = m.params.get("topology") {
            for v in &s.enum_values {
                assert!(
                    matches!(v.as_str(), "triangle" | "quad"),
                    "{}: topology '{}' is outside the documented set {{triangle, quad}}",
                    m.id, v
                );
            }
        }
        if let Some(ParamSpec::String(s)) = m.params.get("symmetry") {
            for v in &s.enum_values {
                assert!(
                    matches!(v.as_str(), "off" | "auto" | "on"),
                    "{}: symmetry '{}' is outside the documented set {{off, auto, on}}",
                    m.id, v
                );
            }
        }
        if let Some(ParamSpec::String(s)) = m.params.get("output_format") {
            assert!(!s.enum_values.is_empty(), "{}: output_format declares no values", m.id);
            for v in &s.enum_values {
                assert!(
                    matches!(v.as_str(), "glb" | "obj" | "fbx" | "usdz" | "stl" | "3mf"),
                    "{}: output_format '{}' is not a mesh container litegen supports",
                    m.id, v
                );
            }
            assert!(
                s.enum_values.iter().any(|v| v == "glb"),
                "{}: every 3D model must be able to emit glb — it is the only \
                 self-contained format the dashboard viewer and the SDK assume",
                m.id
            );
        }
    }
}

#[test]
fn no_3d_model_advertises_image_or_video_only_params() {
    // A 3D row carrying `size`/`fps`/`duration_seconds` would render nonsense
    // controls in the Playground and in any schema-driven client panel.
    let reg = registry();
    for m in model3d_models(&reg) {
        for forbidden in ["size", "fps", "duration_seconds", "aspect_ratio"] {
            assert!(
                !m.params.contains_key(forbidden),
                "{} is a 3D model but advertises the {} param",
                m.id, forbidden
            );
        }
    }
}

#[test]
fn target_polycount_bounds_are_sane_where_declared() {
    let reg = registry();
    for m in model3d_models(&reg) {
        if let Some(ParamSpec::Int(i)) = m.params.get("target_polycount") {
            let (min, max) = (i.min.unwrap_or(0), i.max.unwrap_or(i64::MAX));
            assert!(min > 0, "{}: target_polycount min must be positive, got {}", m.id, min);
            assert!(max > min, "{}: target_polycount max {} <= min {}", m.id, max, min);
            assert!(
                max <= 10_000_000,
                "{}: target_polycount max {} is implausible; no phase-1 vendor \
                 accepts anything near it (Meshy tops out at 300k)",
                m.id, max
            );
        }
    }
}
```

Rust identifiers cannot begin with a digit — rename `3d_enum_vocabularies_...`
to `model3d_enum_vocabularies_match_the_documented_vendor_sets`.

- [x] DONE 2026-08-27 **Step 2: Run to verify they fail, then pass**

Run: `cd litegen-core && cargo test --test catalog_conformance 2>&1 | tail -20`

Before Task 5 lands its mock 3D models, `every_3d_model_declares_at_least_one_generation_mode`
fails on the empty-catalog assertion — that is correct and expected. **Task 19
must therefore run AFTER Task 5.** If you are executing in order this is already
true; if not, sequence it accordingly.

- [x] DONE 2026-08-27 **Step 3: Commit**

```bash
git add litegen-core/tests/catalog_conformance.rs
git commit -m "test(3d): structural catalog conformance for the model3d family"
```

---

### Task 20: Storybook for the dashboard

**Files:**
- Create: `dashboard/.storybook/main.ts`, `dashboard/.storybook/preview.ts`
- Create: `dashboard/src/components/ModelViewer.stories.tsx`, `dashboard/src/playground/ResultTile3D.stories.tsx`, `dashboard/src/playground/ParamField.stories.tsx`, `dashboard/src/pages/Generations.MediaPreview.stories.tsx`
- Modify: `dashboard/package.json`, `dashboard/.gitignore`

**Interfaces:**
- Consumes: `ModelViewer` (Task 13), `ResultTile3D` + `ResultTileState` (Task 14), `ParamField`, `MediaPreview`.
- Produces: `npm run storybook`, `npm run build-storybook`.

**Why:** the 3D surfaces have states that are expensive to reach through the live
app — a job mid-poll at 66%, a mesh that failed to load, a completed tile with no
mesh asset. Stories make each one a single click, and they are the only practical
way to eyeball the viewer against both dashboard themes.

**Must run AFTER Tasks 13 and 14** — it has nothing to render otherwise.

- [ ] **Step 1: Install**

The dashboard is React 19 + Vite 8 + TypeScript 6. Use Storybook's Vite-React
framework and let its initializer match the versions rather than pinning by hand:

```bash
cd dashboard && npx storybook@latest init --builder vite --yes
```

Then inspect what it generated: it may add example stories under
`src/stories/` — delete those, they are noise. Confirm `package.json` gained
`storybook` and `build-storybook` scripts and that `storybook-static/` is
git-ignored.

If the initializer fails on the React 19 / Vite 8 combination, report it as
DONE_WITH_CONCERNS with the error rather than downgrading either dependency —
the dashboard's runtime versions are not negotiable for a tooling addition.

- [ ] **Step 2: Configure**

`dashboard/.storybook/main.ts` — restrict story discovery to co-located files so
no example scaffolding creeps back in:

```ts
import type { StorybookConfig } from '@storybook/react-vite';

const config: StorybookConfig = {
  stories: ['../src/**/*.stories.@(ts|tsx)'],
  addons: ['@storybook/addon-essentials'],
  framework: { name: '@storybook/react-vite', options: {} },
};
export default config;
```

`dashboard/.storybook/preview.ts` — the dashboard renders on a dark ground
(`#0d1117` throughout `Generations.tsx`), so stories must too or every component
will look wrong:

```ts
import type { Preview } from '@storybook/react';

const preview: Preview = {
  parameters: {
    backgrounds: {
      default: 'dashboard',
      values: [
        { name: 'dashboard', value: '#0d1117' },
        { name: 'light', value: '#ffffff' },
      ],
    },
  },
};
export default preview;
```

If the dashboard has a global stylesheet (check `src/main.tsx` for a CSS import),
import it in `preview.ts` too — otherwise `.pg-tile`, `.btn` and friends render
unstyled.

- [ ] **Step 3: Write the stories — one per state that is hard to reach live**

`ModelViewer.stories.tsx`:

```tsx
import type { Meta, StoryObj } from '@storybook/react';
import ModelViewer from './ModelViewer';

const meta: Meta<typeof ModelViewer> = {
  title: 'Media/ModelViewer',
  component: ModelViewer,
};
export default meta;
type Story = StoryObj<typeof ModelViewer>;

/** The happy path: a real mesh served by the gateway. Point `src` at a running
 *  litegen's mock asset URL to see genuine geometry. */
export const WithMesh: Story = {
  args: { src: 'http://127.0.0.1:5099/v1/models3d/assets/litegen/3d/demo/model.glb' },
};

/** A poster is shown while the mesh streams — verify it is not stretched. */
export const WithPoster: Story = {
  args: {
    src: 'http://127.0.0.1:5099/v1/models3d/assets/litegen/3d/demo/model.glb',
    poster: 'https://placehold.co/512x512/0d1117/58a6ff/png',
  },
};

/** An unreachable src must degrade to the download link, never an empty box.
 *  This is the state the live app almost never shows you. */
export const LoadFailure: Story = {
  args: { src: 'http://127.0.0.1:1/nonexistent.glb' },
};
```

`ResultTile3D.stories.tsx` — cover every `TileStatus`, because the polling states
are transient in the real app:

```tsx
import type { Meta, StoryObj } from '@storybook/react';
import ResultTile3D from './ResultTile3D';
import type { ResultTileState } from './types';

const base: ResultTileState = {
  key: 'mock/mesh-3d#0',
  modelId: 'mock/mesh-3d',
  index: 0,
  status: 'queued',
  mediaType: 'model3d',
  request: { model: 'mock/mesh-3d', prompt: 'a low-poly fox' } as never,
};

const meta: Meta<typeof ResultTile3D> = {
  title: 'Playground/ResultTile3D',
  component: ResultTile3D,
  args: { onRerun: () => {} },
};
export default meta;
type Story = StoryObj<typeof ResultTile3D>;

export const Queued: Story = { args: { tile: base } };
export const Submitting: Story = { args: { tile: { ...base, status: 'running' } } };
/** Mid-poll — the state 3D introduced to a Playground that had no async path. */
export const Polling: Story = { args: { tile: { ...base, status: 'polling', progress: 66 } } };
export const Failed: Story = {
  args: { tile: { ...base, status: 'error', error: 'mock 3d generation failed' } },
};
/** Completed but mesh-less: the contract violation the tile must surface as an
 *  error rather than rendering an empty viewer. */
export const CompletedWithoutMesh: Story = {
  args: { tile: { ...base, status: 'done', assets: [], latencyMs: 4200, costUsd: 0 } },
};
export const Done: Story = {
  args: {
    tile: {
      ...base,
      status: 'done',
      latencyMs: 4200,
      costUsd: 0,
      assets: [{ kind: 'mesh', url: 'http://127.0.0.1:5099/v1/models3d/assets/litegen/3d/demo/model.glb', format: 'glb' }],
    },
  },
};
```

`ParamField.stories.tsx` — one story per `ParamSpec` kind (`bool`, `int`,
`float`, `string` with and without `enum_values`, `seed`, `size`,
`aspect_ratio`), including one with `label` and `description` set and one
without, so the Task 1 fallback-to-key-name behaviour is visible side by side.
Read `ParamField.tsx` for its real prop names before writing these.

`Generations.MediaPreview.stories.tsx` — the three-way media switch in both
`thumb` and full modes: image, video, and `model3d` with and without a preview
asset. `MediaPreview` is currently module-private in `Generations.tsx`; export it
so it can be storied. That export is the minimum change — do not restructure the
page.

- [ ] **Step 4: Verify**

```bash
cd dashboard && npm run build-storybook 2>&1 | tail -20
```
Expected: a clean static build. Then `npm run storybook` and click through every
story listed above; a story that throws is a failing test.

Also confirm the dashboard's own gates still pass, since this task touched
`package.json`:
```bash
cd dashboard && npm run build && npm run lint 2>&1 | tail -20
```

- [ ] **Step 5: Commit**

```bash
git add dashboard/.storybook dashboard/package.json dashboard/package-lock.json \
        dashboard/.gitignore dashboard/src/**/*.stories.tsx dashboard/src/pages/Generations.tsx
git commit -m "feat(dashboard): Storybook, with stories for the 3D and param surfaces"
```

---

## Amended acceptance (supersedes Task 17 Step 1)

```bash
cd litegen-core && cargo test
cd litegen-core && cargo clippy --all-targets -- -D warnings
cd sdks/typescript && npm run build && npm test
cd dashboard && npm run build && npm run lint
cd dashboard && npm run build-storybook
cargo build --release -p litegen && cd dashboard && npm run test:e2e
cd apps/landing && npm run build && npm run test:scripts
```

Note the last two additions: the Playwright run needs a **release** binary
(`playwright.config.ts` boots `../litegen-core/target/release/litegen`), and the
landing app has a script test suite (`npm run test:scripts`) that Task 16's
generator changes must keep green.

---

# Plan Amendment 2 — 2026-09-11: thorough 3D preview + observability

The user asked for "thorough 3D model previewing capabilities" and Storybook
coverage of both the Playground and the generation-viewing (observability)
surfaces. A survey of the dashboard found three concrete gaps that make this
more than polish:

1. **The Logs → trace panel can never show a 3D mesh (or a video).** The
   request artifact is written at submit time with `output_value: None`
   (async — the URL is unknown), and **nothing ever updates it**
   (`DatabaseStore` has `insert_request_artifact` only). So
   `TracePanel`'s `model3d` branch is dead code: every 3D row shows "Output URL
   not yet available (async generation pending)" forever. Video has the same bug.
2. **A mesh that fails to load renders an empty canvas.** `ModelViewer` only
   falls back when the *bundle import* fails; `<model-viewer>`'s own `error`
   event (404, corrupt GLB, CORS) is never observed.
3. **The TypeScript SDK has no `generations.get(id)`** even though
   `GET /v1/generations/{id}` exists — the trace panel needs it to resolve an
   async artifact to its generation row.

Decisions (recorded here and in the implementing commits):

- **Resolve async artifacts in the frontend from the generation row; do NOT add
  an artifact-update path in the DB.** The generation row already holds
  `status`, `result_url` and `metadata.assets`; the artifact's `request_id` IS
  the generation id (`handlers/mod.rs`, `request_id: response.id.clone()`).
  Writing the same data a second time into the artifact would create two
  sources of truth that can disagree. This fixes video too, for free.
- **Presentational / container split** so every state is storyable without a
  network: `ModelPreview` and `GenerationOutput` take data as props; only
  `TracePanel` fetches.
- **Keep `ModelViewer` the low-level element wrapper; add `ModelPreview` as the
  full inspector.** Compact contexts (Playground grid tiles, gallery thumbs)
  keep the light viewer; full contexts (Generations detail row, Playground
  Single Mode, trace panel) get the inspector.
- **Add vitest to the dashboard** for the pure asset helpers. The dashboard has
  no unit runner today (Playwright only); vitest is the boring choice — it is
  already the SDK's runner and shares the Vite toolchain.
- **No wireframe mode.** `<model-viewer>` has no wireframe API; faking one
  means dropping to raw three.js scene access, which is a maintenance cost the
  user did not ask for.

Sequencing: **21 → 22 → 20 (expanded) → clean + release build → 15 → 17.**

---

### Task 21: `ModelPreview` — the full 3D inspector

**Status:** ✅ DONE (2026-09-11, e7c8943 + fixes 0575ffb, 4ced9f4) — task review clean after 2 fix rounds (import/no-WebGL stuck states, malformed-asset crash, stale-load race, non-glTF message, public-API retry)

**Files:**
- Create: `dashboard/src/components/model3d-assets.ts` (pure helpers)
- Create: `dashboard/src/components/model3d-assets.test.ts`
- Create: `dashboard/src/components/ModelPreview.tsx`
- Modify: `dashboard/src/components/ModelViewer.tsx`
- Modify: `dashboard/src/pages/Generations.tsx` (expanded row), `dashboard/src/playground/SingleMode.tsx` (result panel)
- Modify: `dashboard/package.json` (vitest devDependency + `"test": "vitest run"`)

**Interfaces:**
- Produces: `assetsOf(g: Generation): Model3dAsset[]`, `meshOf(assets)`,
  `previewOf(assets)`, `texturesOf(assets)`, `formatBytes(n)`;
  `<ModelPreview assets={Model3dAsset[]} error?={string} testId?={string} />`;
  `<ModelViewer … onLoad? onError? autoRotate? exposure? background? />`.
- Consumes: SDK type `Model3dAsset`.

- [x] DONE 2026-09-11 **Step 1: Move the helpers out of `ModelViewer.tsx`.** `meshUrl`/`previewUrl`
  currently live in the component file behind two `eslint-disable
  react-refresh/only-export-components` comments. Move them into
  `model3d-assets.ts` (typed against the SDK's `Model3dAsset`, not a local
  interface) and delete the disables. Update the two importers.
- [x] DONE 2026-09-11 **Step 2: Unit-test the helpers with vitest** — `assetsOf` tolerates
  missing/`null`/non-array `metadata.assets`; `meshOf` falls back to
  `result_url`; `formatBytes` boundary values (0, 1023, 1024, 1.5 MB);
  `texturesOf` excludes mesh and preview. Watch them fail first.
- [x] DONE 2026-09-11 **Step 3: Make `ModelViewer` observe `<model-viewer>`'s own events.**
  Attach listeners via a ref for `load`, `error`, and `progress`. On `error`,
  render the same download-link fallback the import-failure path uses — a
  404 or corrupt GLB must never leave an empty canvas. Show a thin progress bar
  driven by `event.detail.totalProgress` while loading. Accept `autoRotate`,
  `exposure`, and `background` props. Forward `onLoad` with the model's
  bounding-box dimensions from `getDimensions()`.
- [x] DONE 2026-09-11 **Step 4: Build `ModelPreview`.** A viewer stage plus:
  - toolbar: auto-rotate toggle, reset camera, exposure slider (0.25–2.0),
    background (dark / light / checker), fullscreen (Fullscreen API on the
    container), copy mesh URL;
  - stats strip: format, size (`formatBytes`), polycount, bounding-box
    dimensions (from `onLoad`);
  - asset list: every asset with a kind badge, format, size, polycount or
    WxH, and open/download links — download via `<a download>` on the
    absolute URL;
  - texture/preview thumbnail strip;
  - an explicit error state when `error` is set or no mesh exists (a
    completed generation without a mesh is a contract violation — say so).
  Every control gets a stable `data-testid` (Task 15 drives them).
- [x] DONE 2026-09-11 **Step 5: Wire it in.** Generations expanded row and Playground Single
  Mode result panel render `ModelPreview`; Playground grid tiles and gallery
  thumbs keep the compact `ModelViewer` / flat image.
- [x] DONE 2026-09-11 **Step 6: Gates.** `cd dashboard && npm test && npm run build && npm run lint`
  (lint must not exceed the 13-problem pre-existing baseline). Commit, and tick
  this task's boxes in the same commit.

---

### Task 22: Observability — resolve async artifacts to their generation

**Status:** ✅ DONE (2026-09-11, c13e652) — contract-test regex fixed (SDK suite fully green); every 3D artifact now resolves via its generation; 404 retry capped at 20 polls

**Files:**
- Modify: `sdks/typescript/src/client.ts` (`generations.get`), `sdks/typescript/test/client.test.ts`
- Create: `dashboard/src/components/GenerationOutput.tsx`
- Modify: `dashboard/src/components/TracePanel.tsx`

**Interfaces:**
- Consumes: `ModelPreview`, `assetsOf` (Task 21).
- Produces: `client.generations.get(id, signal?) : Promise<Generation>`;
  `<GenerationOutput generation={Generation | null} loading={boolean} error?={string} />`.

- [x] DONE 2026-09-11 **Step 1: SDK `generations.get`.** `GET /v1/generations/{id}` exists in
  the API and the OpenAPI document but the client never exposed it. Add it next
  to `list`/`cancel`, with a test asserting method + URL. Gates:
  `npm run typecheck && npm test && npm run build` in `sdks/typescript`
  (typecheck is mandatory — vitest does not type-check, and skipping it let a
  regression through on Task 12). The pre-existing `contract.test.ts`
  allowed-models failure stays out of scope.
- [x] DONE 2026-09-11 **Step 2: `GenerationOutput` (presentational).** Renders a generation row
  by status: pending/processing → status + progress bar; failed/cancelled →
  the error message; completed `model3d` → `ModelPreview` from `assetsOf`;
  completed `video` → `<video>` (or `<img>` for an `image/*` GIF result, as
  `VisualTab` already special-cases); completed image → `<img>`.
- [x] DONE 2026-09-11 **Step 3: `TracePanel` resolution.** In `VisualTab`, when
  `output_kind === 'url'` and `output_value` is empty and `media_type` is
  `video` or `model3d`, fetch `client.generations.get(artifact.request_id)`
  and render `GenerationOutput`. While the generation is non-terminal, re-poll
  every 3s and stop on terminal or unmount. A 404 (row not yet persisted — the
  insert is spawned after the response) shows "pending" and keeps polling
  rather than erroring. Remove the now-dead `model3d` `output_value` branch
  only if nothing else can reach it; say which you did in the commit.
- [x] DONE 2026-09-11 **Step 4: Gates.** Dashboard `npm test && npm run build && npm run lint`;
  SDK gates from Step 1. Commit, ticking this task's boxes.

---

## Expansion of Task 20 (Storybook) — binding

Task 20 runs **after** Tasks 21 and 22 and its story list grows to cover both
surfaces the user named:

**Playground / testing:** `ResultTile3D` (queued, running, polling 33% / 66%,
done, failed, done-without-mesh, cancelled); `ResultTile` (image, for
contrast); `ParamField` (every `ParamSpec` kind, with and without
`label`/`description`, including the 3D enums — topology, symmetry,
output_format — and `target_polycount`).

**Viewing / observability:** `ModelViewer` (loading, loaded, mesh 404,
corrupt GLB, bundle-import failure); `ModelPreview` (full asset set with
textures + preview, mesh-only, no-mesh contract violation, high-polycount
mesh, light and checker backgrounds); `MediaPreview` (thumb and full ×
image / video / 3D-with-preview / 3D-without-preview); `GenerationOutput`
(pending 0%, processing 66%, completed 3D, completed video, failed,
cancelled).

**Fixtures — real bytes, not placeholders.** Stories must render actual
meshes, so commit fixtures under `dashboard/.storybook/fixtures/` served via
`staticDirs`: a textured cube `cube.glb`, a higher-poly `icosphere.glb` (so
the polycount stat shows something non-trivial), a checker `texture.png`, a
`preview.png`, and a deliberately corrupt `corrupt.glb`. Generate them with a
committed script (`make_fixtures.py`, stdlib only) rather than copying binaries
from elsewhere, so they are reproducible. The mesh-404 story points at a path
that does not exist.

Stories co-locate as `*.stories.tsx` beside their components. Add
`npm run build-storybook` to the amended acceptance gates.

## Expansion of Task 15 (Playwright) — binding

Add coverage for the inspector and the observability fix: in the Generations
expanded row, the `ModelPreview` stats strip shows the mock's polycount (12)
and the asset list lists the mesh and the preview; and in Logs, opening the
trace panel for a completed 3D request renders the mesh (the `model-viewer`
element reaches `loaded`) rather than "Output URL not yet available".

---

### Task 17A: Carried review follow-ups (no dependency install, no Rust build)

Split out of Task 17 on 2026-09-11 because free disk (~0.4 GB, driven by system
swap from many concurrent sessions) cannot fit the Storybook install or the
multi-GB Rust build that Task 17's final gates need. Everything here is a small,
review-identified defect in files already touched by this plan, and each is
verifiable with the dashboard's vitest/build/lint and the landing script tests.

- [ ] **Param-mapping matrix:** annotate the `rig` row's Tripo3D cell
  `(unconfirmed)` — it names a literal `rig` field that appears nowhere in the
  design spec §9, contradicting the doc's claim that every vendor column is
  transcribed from §9 (Task 18/19 review).
- [ ] **`apps/landing/scripts/derive-capabilities.mjs`:** update the top
  docstring and the fallback-table comment, which still describe outputs as
  image/video only (Task 16 review). Keep `npm run test:scripts` green.
- [ ] **`dashboard/src/components/generation-output.ts`:** correct the 404-retry
  rationale (comment at :19-24 and the test name at `generation-output.test.ts:34`).
  The artifact is inserted only AFTER `insert_generation` is awaited in the same
  spawned task (verified at `handlers/mod.rs` generate_video :437-455 and
  generate_3d :683-692), so a lasting 404 means a failed insert; the retry is a
  safety margin, not "the insert races the response" (Task 22 review).
- [ ] **Transient-error tolerance in the trace-panel poll:** a single 5xx /
  timeout / network error during a minutes-long 3D job currently ends the live
  view and discards the last known progress. Tolerate a short streak (3
  consecutive) of transient errors — keep showing the last generation with a
  subtle "reconnecting" note — and only end on the streak or a non-transient
  error. Extend `shouldKeepPolling`'s unit tests (Task 22 review).
- [ ] **`GenerationOutput` video fallback:** `GenerationOutput.tsx:66` turns ANY
  `<video>` load failure into a broken `<img>`. Only fall back to `<img>` when
  the result is plausibly an image (GIF/`image/*`); otherwise show the failure
  and an open link (Task 22 review).
- [ ] **Playground Single Mode:** disable the model `<select>` while a
  generation is in flight. The exposure grew from a sub-second image request to
  a multi-second 3D poll (Task 13/14 review).
- [ ] **Gates:** `cd dashboard && npm test && npm run build && npm run lint`;
  `cd apps/landing && npm run test:scripts`. Commit, ticking this section.
